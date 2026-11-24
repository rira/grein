use std::collections::BTreeMap;
use std::sync::Arc;

use grein_core::blackboard::{BlackboardEntry, BlackboardEntryKind};
use grein_core::dag::DagStorage;
use grein_core::model::{
    Anchor, AnchorId, AuthorRole, ConversationNode, NodeId, Thread, ThreadId, ThreadKind,
    ThreadStatus,
};
use grein_core::port::dto::{
    AgentConfigChoice, AgentConfigOption, AgentMetaEntry, AgentOptionsSnapshot, AgentProviderInfo,
    AgentSlashCommand, EngineControlEvent, EngineStreamEvent, HydratedSession, HydratedThread,
    HydratedTurn, PermissionDecisionPayload, PtyCell, PtyColor, PtyMouseEncoding, PtyMouseMode,
    PtyScreenSnapshot, SessionSnapshot, ShellCommandResult, SubagentSnapshot, SubagentStatus,
    ThreadMeta, ThreadSummary, TranscriptEntry, TuiEngineEvent, TurnDispatchResult,
    UserPreferencesSnapshot, WorktreeEntry,
};
use grein_core::port::outbound::{PreferenceRepository, SessionRepository, SettingsCodec};
use grein_core::prefs::codec_for;
use grein_core::storage::SqliteStorage;
use grein_core::sync::ContextCompiler;
use grein_core::{
    CoreError, JsonSettingsCodec, SectionPatch, SettingsFormat, SettingsPatch, SettingsSnapshot,
    TomlSettingsCodec, UserPrefsStore,
};
use serde_json::{json, Value};

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn outbound_ports_and_concrete_adapters_are_send_sync() {
    assert_send_sync::<SqliteStorage>();
    assert_send_sync::<UserPrefsStore>();
    assert_send_sync::<TomlSettingsCodec>();
    assert_send_sync::<JsonSettingsCodec>();
    assert_send_sync::<Arc<dyn DagStorage>>();
    assert_send_sync::<Arc<dyn SessionRepository>>();
    assert_send_sync::<Arc<dyn PreferenceRepository>>();
    assert_send_sync::<Arc<dyn SettingsCodec>>();
}

#[test]
fn session_repository_trait_object_covers_full_lifecycle() -> grein_core::Result<()> {
    let storage = SqliteStorage::in_memory()?;
    assert!(storage.is_initialized());
    assert!(storage.table_exists("nodes")?);
    assert!(storage.list_tables()?.iter().any(|t| t == "threads"));

    let repo: Arc<dyn SessionRepository> = Arc::new(storage);

    let root = ConversationNode::new_root(AuthorRole::System, "init");
    let user_1 = ConversationNode::new(
        NodeId::new_v7(),
        Some(root.id),
        AuthorRole::User,
        "  first prompt  ".to_string(),
    );
    let asst_1 = ConversationNode::new(
        NodeId::new_v7(),
        Some(user_1.id),
        AuthorRole::Assistant,
        "reply".to_string(),
    );
    repo.insert_node(&root)?;
    repo.insert_node(&user_1)?;
    repo.insert_node(&asst_1)?;

    assert_eq!(repo.get_node(&user_1.id.0)?, Some(user_1.clone()));
    assert_eq!(repo.get_children(&root.id.0)?, vec![user_1.clone()]);
    assert_eq!(repo.list_root_nodes()?, vec![root.clone()]);
    assert_eq!(repo.list_nodes()?.len(), 3);
    assert_eq!(repo.list_recent_user_prompts(5)?, vec!["first prompt"]);
    assert_eq!(
        repo.get_trunk_path(&asst_1.id.0)?,
        vec![root.clone(), user_1.clone(), asst_1.clone()]
    );

    let anchor_id = AnchorId::new_v7();
    let anchor = Anchor {
        level: 2,
        title: "Ports".to_string(),
        slug: "ports".to_string(),
        line_number: 10,
    };
    repo.insert_anchor(&anchor_id.0, &anchor)?;
    assert_eq!(repo.get_anchor(&anchor_id.0)?, Some(anchor.clone()));
    assert_eq!(repo.list_anchors()?, vec![(anchor_id, anchor.clone())]);
    repo.link_node_anchor(&asst_1.id.0, &anchor_id.0)?;
    assert_eq!(repo.list_anchors_for_node(&asst_1.id.0)?, vec![anchor]);

    let main_tid = repo.create_thread_with_title(root.id, anchor_id, "Main")?;
    let branch_tid =
        repo.create_thread_with_parent(asst_1.id, anchor_id, Some("Branch"), Some(main_tid))?;
    let term_tid = repo.create_thread_with_kind(
        asst_1.id,
        anchor_id,
        Some("Shell"),
        Some(branch_tid),
        ThreadKind::Terminal,
        Some("/tmp/wt"),
    )?;
    let dag_tid = ThreadId(repo.create_thread(&root.id.0, &anchor_id.0)?);
    let direct_tid = ThreadId::new_v7();
    let direct = Thread::new(
        direct_tid,
        root.id,
        anchor_id,
        asst_1.id,
        ThreadStatus::AwaitingInput,
    )
    .with_title("Direct");
    repo.insert_thread(&direct)?;

    assert_eq!(repo.list_threads()?.len(), 5);
    assert_eq!(repo.get_thread_leaf(&direct_tid)?, asst_1.id);
    assert_eq!(
        repo.get_thread_state(&direct_tid)?,
        ThreadStatus::AwaitingInput
    );

    repo.update_thread_leaf(&main_tid.0, &asst_1.id.0)?;
    repo.update_thread_status(&main_tid.0, ThreadStatus::Streaming)?;
    repo.update_thread_title(&main_tid, "Main Updated")?;
    repo.update_thread_custom_name(&main_tid, Some("Primary"))?;
    repo.update_thread_agent_provider(&main_tid, Some("claude"))?;
    repo.update_thread_agent_provider(&direct_tid, Some("codex"))?;
    repo.update_thread_agent_provider(&direct_tid, None)?;
    repo.update_thread_cwd(&term_tid, Some("/tmp/wt2"))?;
    repo.update_thread_acp_session_id(&branch_tid, Some("acp-1"))?;
    repo.mark_thread_merged(&branch_tid, true)?;

    let main_loaded = repo.get_thread(&main_tid.0)?.expect("main thread");
    assert_eq!(main_loaded.title, "Main Updated");
    assert_eq!(main_loaded.custom_name.as_deref(), Some("Primary"));
    assert_eq!(main_loaded.agent_provider.as_deref(), Some("claude"));
    assert_eq!(main_loaded.status, ThreadStatus::Streaming);
    assert_eq!(main_loaded.active_leaf_id, asst_1.id);

    let direct_loaded = repo.get_thread(&direct_tid.0)?.expect("direct thread");
    assert_eq!(direct_loaded.agent_provider, None);

    let branch_loaded = repo.get_thread(&branch_tid.0)?.expect("branch thread");
    assert_eq!(branch_loaded.acp_session_id.as_deref(), Some("acp-1"));
    assert!(branch_loaded.merged);

    let payload = ContextCompiler::new("Sys").compile_thread_context(repo.as_ref(), branch_tid)?;
    assert_eq!(payload.anchor_header.as_deref(), Some("## Ports"));
    assert_eq!(payload.trunk_path.len(), 3);

    assert_eq!(
        repo.reparent_child_threads(&branch_tid, Some(&main_tid))?,
        1
    );
    let term_loaded = repo.get_thread(&term_tid.0)?.expect("term thread");
    assert_eq!(term_loaded.parent_thread_id, Some(main_tid));
    assert_eq!(term_loaded.cwd.as_deref(), Some("/tmp/wt2"));

    assert!(repo.delete_thread(&dag_tid)?);
    assert!(!repo.delete_thread(&dag_tid)?);

    let entry = BlackboardEntry::new("arch", "hexagonal", BlackboardEntryKind::Decision);
    repo.insert_blackboard_entry(&entry)?;
    assert_eq!(
        repo.get_blackboard_entry("arch")?.map(|e| e.value),
        Some("hexagonal".to_string())
    );
    assert_eq!(repo.list_blackboard_entries()?.len(), 1);
    assert!(repo.delete_blackboard_entry("arch")?);
    assert!(repo.list_blackboard_entries()?.is_empty());

    Ok(())
}

#[test]
fn preference_repository_and_settings_codec_round_trip() -> grein_core::Result<()> {
    let repo: Arc<dyn PreferenceRepository> = Arc::new(UserPrefsStore::in_memory()?);

    repo.set("custom", "k1", "v1")?;
    assert_eq!(repo.get("custom", "k1")?.as_deref(), Some("v1"));
    assert_eq!(
        repo.list_namespace("custom")?,
        BTreeMap::from([("k1".to_string(), "v1".to_string())])
    );
    assert!(repo.delete("custom", "k1")?);

    repo.set_keybinding("quit", "q")?;
    assert_eq!(
        repo.keybindings()?,
        BTreeMap::from([("quit".to_string(), "q".to_string())])
    );
    assert!(repo.remove_keybinding("quit")?);
    repo.set_keybinding("branch", "b")?;
    assert_eq!(repo.clear_keybindings()?, 1);

    repo.set_theme("nord")?;
    repo.set_theme_pref("accent", "#88c0d0")?;
    assert_eq!(repo.theme()?.as_deref(), Some("nord"));
    assert_eq!(repo.theme_prefs()?.len(), 2);
    assert_eq!(repo.clear_theme_prefs()?, 2);

    repo.set_ui_pref("density", "compact")?;
    assert_eq!(repo.ui_prefs()?.len(), 1);
    assert_eq!(repo.clear_ui_prefs()?, 1);

    let patch = SettingsPatch {
        version: 1,
        preferences: BTreeMap::from([(
            "theme".to_string(),
            SectionPatch::Entries(BTreeMap::from([(
                "name".to_string(),
                Some("dracula".into()),
            )])),
        )]),
    };
    let report = repo.import_patch(&patch, None, false)?;
    assert_eq!(report.updated, 1);

    let toml_out = repo.export_string(SettingsFormat::Toml, None)?;
    let json_out = repo.export_string(SettingsFormat::Json, None)?;
    let snap = repo.export_snapshot(None)?;

    let toml_codec: Arc<dyn SettingsCodec> = Arc::new(TomlSettingsCodec);
    let json_codec: Arc<dyn SettingsCodec> = Arc::new(JsonSettingsCodec);
    assert_eq!(toml_codec.format(), SettingsFormat::Toml);
    assert_eq!(toml_codec.format_name(), "toml");
    assert_eq!(json_codec.format(), SettingsFormat::Json);
    assert_eq!(json_codec.format_name(), "json");
    assert_eq!(codec_for(SettingsFormat::Toml).format_name(), "toml");
    assert_eq!(codec_for(SettingsFormat::Json).format_name(), "json");

    assert_eq!(toml_codec.encode(&snap)?, toml_out);
    assert_eq!(json_codec.encode(&snap)?, json_out);
    assert_eq!(toml_codec.decode(&toml_out)?, patch);
    assert_eq!(json_codec.decode(&json_out)?, patch);
    assert_eq!(toml_codec.decode_theme("name = \"dracula\"\n")?, patch);
    assert_eq!(json_codec.decode_theme(r#"{"name":"dracula"}"#)?, patch);

    let section_entries = BTreeMap::from([("name".to_string(), "nord".to_string())]);
    assert_eq!(
        toml_codec.encode_section(" theme ", &section_entries)?,
        "[theme]\nname = \"nord\"\n"
    );
    let encoded_json_sec: SettingsSnapshot =
        serde_json::from_str(&json_codec.encode_section("theme", &section_entries)?)?;
    assert_eq!(
        encoded_json_sec.preferences.get("theme"),
        Some(&section_entries)
    );
    assert!(matches!(
        toml_codec.encode_section("   ", &section_entries),
        Err(CoreError::SyncError(_))
    ));

    let imported_str = repo.import_str(&json_out, Some(SettingsFormat::Json), None, true)?;
    assert_eq!(imported_str.updated, 1);
    let imported_theme = repo.import_theme_str("name = \"nord\"\n", None, true)?;
    assert_eq!(imported_theme.updated, 1);
    assert_eq!(repo.theme()?.as_deref(), Some("nord"));

    Ok(())
}

#[test]
fn port_dto_wire_format_matches_existing_contracts() -> Result<(), Box<dyn std::error::Error>> {
    // 1. EngineControlEvent preserves internally-tagged snake_case wire format
    let ctrl_events = [
        (
            EngineControlEvent::TurnRecorded {
                node_id: "n-1".into(),
            },
            json!({"type": "turn_recorded", "node_id": "n-1"}),
        ),
        (
            EngineControlEvent::ThreadForked {
                thread_id: "t-1".into(),
            },
            json!({"type": "thread_forked", "thread_id": "t-1"}),
        ),
        (
            EngineControlEvent::ThreadMerged {
                thread_id: "t-2".into(),
            },
            json!({"type": "thread_merged", "thread_id": "t-2"}),
        ),
        (
            EngineControlEvent::PermissionResolved {
                request_id: "r-1".into(),
                allow: true,
            },
            json!({"type": "permission_resolved", "request_id": "r-1", "allow": true}),
        ),
    ];
    for (ev, expected_val) in ctrl_events {
        let actual: Value = serde_json::to_value(&ev)?;
        assert_eq!(actual, expected_val);
        let decoded: EngineControlEvent = serde_json::from_value(expected_val)?;
        assert_eq!(decoded, ev);
    }

    // 2. PtyScreenSnapshot legacy defaults + new mouse/wrap fields + plain_text()
    let legacy_pty = r#"{
        "rows": 24,
        "cols": 80,
        "cells": [],
        "cursor_row": 0,
        "cursor_col": 0,
        "hide_cursor": false,
        "application_cursor": false,
        "bracketed_paste": false,
        "alternate_screen": false,
        "exited": false,
        "exit_code": null
    }"#;
    let legacy_snap: PtyScreenSnapshot = serde_json::from_str(legacy_pty)?;
    assert_eq!(legacy_snap.scrollback_offset, 0);
    assert_eq!(legacy_snap.cwd, None);
    assert!(legacy_snap.row_wrapped.is_empty());
    assert_eq!(legacy_snap.mouse_protocol_mode, PtyMouseMode::None);
    assert_eq!(
        legacy_snap.mouse_protocol_encoding,
        PtyMouseEncoding::Default
    );

    let full_snap = PtyScreenSnapshot {
        rows: 3,
        cols: 4,
        cells: vec![
            vec![
                PtyCell {
                    contents: "🦀".into(),
                    fg: PtyColor::Rgb(10, 20, 30),
                    bg: PtyColor::Indexed(5),
                    bold: true,
                    italic: false,
                    underline: true,
                    inverse: false,
                    is_wide_continuation: false,
                },
                PtyCell {
                    contents: String::new(),
                    is_wide_continuation: true,
                    ..PtyCell::default()
                },
                PtyCell {
                    contents: "!".into(),
                    ..PtyCell::default()
                },
                PtyCell::default(),
            ],
            vec![PtyCell::default(); 4],
        ],
        cursor_row: 0,
        cursor_col: 3,
        hide_cursor: false,
        application_cursor: true,
        bracketed_paste: true,
        alternate_screen: false,
        exited: true,
        exit_code: Some(0),
        scrollback_offset: 12,
        cwd: Some("/tmp".into()),
        row_wrapped: vec![true, false, false],
        mouse_protocol_mode: PtyMouseMode::ButtonMotion,
        mouse_protocol_encoding: PtyMouseEncoding::Sgr,
    };
    assert_eq!(full_snap.plain_text(), "🦀!");
    let snap_json = serde_json::to_string(&full_snap)?;
    let snap_roundtrip: PtyScreenSnapshot = serde_json::from_str(&snap_json)?;
    assert_eq!(snap_roundtrip, full_snap);

    // 3. EngineStreamEvent / TuiEngineEvent preserves externally-tagged wire format across all 23 variants
    let config_opt = AgentConfigOption {
        id: "model".into(),
        name: "Model".into(),
        category: Some("model".into()),
        current_value: "m1".into(),
        options: vec![AgentConfigChoice {
            value: "m1".into(),
            name: "Model 1".into(),
            description: Some("fast".into()),
        }],
    };
    let slash_cmd = AgentSlashCommand {
        name: "/plan".into(),
        description: "plan mode".into(),
    };
    let subagent = SubagentSnapshot {
        id: "sa-1".into(),
        role: "Explorer".into(),
        agent_type: "research".into(),
        model: Some("flash".into()),
        activity: Some("reading".into()),
        status: SubagentStatus::Running,
        started_at_ms: 42,
    };
    let thread_meta = ThreadMeta {
        thread_id: "t-1".into(),
        parent_thread_id: None,
        inherited_turn_count: 2,
        merged: false,
        kind: ThreadKind::Terminal,
        cwd: Some("/work".into()),
        git_branch: Some("main".into()),
        git_dirty: 1,
        custom_name: Some("Shell".into()),
        agent_provider: Some("claude".into()),
    };
    let hydrated_thread = HydratedThread {
        id: "t-1".into(),
        title: "Main".into(),
        custom_name: None,
        agent_provider: Some("claude".into()),
        turns: vec![HydratedTurn {
            id: "n-1".into(),
            role: "user".into(),
            content: "hi".into(),
        }],
    };

    let stream_events: Vec<TuiEngineEvent> = vec![
        EngineStreamEvent::StreamChunk {
            thread_id: "t-1".into(),
            content: "abc".into(),
        },
        EngineStreamEvent::TurnCompleted {
            thread_id: "t-1".into(),
        },
        EngineStreamEvent::ToolInvoked {
            thread_id: "t-1".into(),
            tool_name: "read".into(),
            tool_call_id: Some("tc-1".into()),
            summary: Some("file.rs".into()),
        },
        EngineStreamEvent::ToolResult {
            thread_id: "t-1".into(),
            tool_name: "read".into(),
            output: "ok".into(),
            tool_call_id: Some("tc-1".into()),
            is_error: false,
        },
        EngineStreamEvent::PermissionRequested {
            request_id: "req-1".into(),
            description: "allow?".into(),
        },
        EngineStreamEvent::ThreadStatusChanged {
            thread_id: "t-1".into(),
            status: "streaming".into(),
        },
        EngineStreamEvent::ThoughtChunk {
            thread_id: "t-1".into(),
            content: "hmm".into(),
        },
        EngineStreamEvent::ThreadCreated {
            thread: hydrated_thread.clone(),
        },
        EngineStreamEvent::ThreadTitleUpdated {
            thread_id: "t-1".into(),
            title: "Updated".into(),
        },
        EngineStreamEvent::ThreadCustomNameUpdated {
            thread_id: "t-1".into(),
            custom_name: Some("Custom".into()),
        },
        EngineStreamEvent::ThreadMerged {
            thread_id: "t-2".into(),
            target_thread_id: "t-1".into(),
            summary: "done".into(),
        },
        EngineStreamEvent::ThreadMetadataUpdated {
            meta: vec![thread_meta.clone()],
        },
        EngineStreamEvent::ThreadDeleted {
            thread_id: "t-2".into(),
            next_active_thread_id: "t-1".into(),
        },
        EngineStreamEvent::AgentMetaUpdated {
            thread_id: "t-1".into(),
            model: Some("m1".into()),
            mode: Some("code".into()),
        },
        EngineStreamEvent::AgentConfigUpdated {
            thread_id: "t-1".into(),
            config_options: vec![config_opt.clone()],
        },
        EngineStreamEvent::AgentCommandsUpdated {
            thread_id: "t-1".into(),
            commands: vec![slash_cmd.clone()],
        },
        EngineStreamEvent::SubagentUpdated {
            thread_id: "t-1".into(),
            subagent,
        },
        EngineStreamEvent::SubagentRemoved {
            thread_id: "t-1".into(),
            subagent_id: "sa-1".into(),
        },
        EngineStreamEvent::ToolActivityUpdated {
            thread_id: "t-1".into(),
            tool_name: None,
        },
        EngineStreamEvent::ShellOutput {
            thread_id: "t-1".into(),
            command: "pwd".into(),
            stdout: "/tmp\n".into(),
            stderr: String::new(),
            exit_code: 0,
        },
        EngineStreamEvent::PtyScreenUpdated {
            thread_id: "t-1".into(),
            snapshot: full_snap,
        },
        EngineStreamEvent::PtyExited {
            thread_id: "t-1".into(),
            exit_code: Some(0),
        },
        EngineStreamEvent::Error {
            message: "boom".into(),
        },
    ];
    assert_eq!(stream_events.len(), 23);

    let turn_completed_val = serde_json::to_value(&stream_events[1])?;
    assert_eq!(
        turn_completed_val,
        json!({"TurnCompleted": {"thread_id": "t-1"}})
    );

    for ev in &stream_events {
        let encoded = serde_json::to_string(ev)?;
        let decoded: EngineStreamEvent = serde_json::from_str(&encoded)?;
        assert_eq!(&decoded, ev);
    }

    // Legacy missing-default fields on ToolInvoked, ToolResult, ThreadCustomNameUpdated
    let legacy_tool_invoked: EngineStreamEvent =
        serde_json::from_str(r#"{"ToolInvoked":{"thread_id":"t","tool_name":"sh"}}"#)?;
    assert_eq!(
        legacy_tool_invoked,
        EngineStreamEvent::ToolInvoked {
            thread_id: "t".into(),
            tool_name: "sh".into(),
            tool_call_id: None,
            summary: None,
        }
    );

    // Remaining DTOs round-trip check
    let summary: ThreadSummary =
        serde_json::from_str(r#"{"thread_id":"t-1","parent_node_id":"n-0","merged":false}"#)?;
    assert_eq!(summary.status, ThreadStatus::Idle);
    assert_eq!(summary.title, "");

    let session_snap = SessionSnapshot {
        node_count: 1,
        thread_count: 1,
        threads: vec![summary],
    };
    let worktree = WorktreeEntry {
        repo_root: "/repo".into(),
        repo_name: "repo".into(),
        path: "/repo".into(),
        branch: Some("main".into()),
        head_short: Some("abcdef1".into()),
        is_primary: true,
        is_detached: false,
        is_grein_managed: false,
        dirty_count: 0,
        attached_thread_ids: vec!["t-1".into()],
    };
    let opts_snap = AgentOptionsSnapshot {
        thread_id: "t-1".into(),
        config_options: vec![config_opt.clone()],
        available_commands: vec![slash_cmd.clone()],
    };
    let agent_meta = AgentMetaEntry {
        thread_id: "t-1".into(),
        model: Some("m1".into()),
        mode: Some("code".into()),
        config_options: vec![config_opt],
        available_commands: vec![slash_cmd],
    };
    let hydrated_session = HydratedSession {
        active_thread_id: "t-1".into(),
        threads: vec![hydrated_thread],
    };
    let prefs_snap = UserPreferencesSnapshot {
        theme: Some("nord".into()),
        theme_overrides: BTreeMap::new(),
        keybindings: BTreeMap::new(),
        ui: BTreeMap::new(),
    };
    let misc = (
        serde_json::to_string(&session_snap)?,
        serde_json::to_string(&worktree)?,
        serde_json::to_string(&opts_snap)?,
        serde_json::to_string(&agent_meta)?,
        serde_json::to_string(&hydrated_session)?,
        serde_json::to_string(&prefs_snap)?,
        serde_json::to_string(&TranscriptEntry {
            node_id: "n".into(),
            role: "user".into(),
            content: "hi".into(),
        })?,
        serde_json::to_string(&TurnDispatchResult {
            node_id: "n".into(),
        })?,
        serde_json::to_string(&PermissionDecisionPayload {
            request_id: "r".into(),
            allow: false,
        })?,
        serde_json::to_string(&ShellCommandResult {
            command: "ls".into(),
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        })?,
    );
    assert!(!misc.0.is_empty() && !misc.9.is_empty());

    Ok(())
}

#[test]
fn agent_provider_dtos_roundtrip_and_default_missing_fields(
) -> Result<(), Box<dyn std::error::Error>> {
    let provider_full = AgentProviderInfo {
        id: "claude".into(),
        plugin_id: "grein-acp-claude".into(),
        display_name: "Claude Code".into(),
        kind: "acp".into(),
        available: true,
        executable: Some("/usr/local/bin/claude-agent-acp".into()),
        is_default: true,
        install_hint: Some("npm install -g @zed-industries/claude-agent-acp".into()),
    };
    let provider_json = serde_json::to_string(&provider_full)?;
    let provider_decoded: AgentProviderInfo = serde_json::from_str(&provider_json)?;
    assert_eq!(provider_decoded, provider_full);

    let provider_minimal: AgentProviderInfo = serde_json::from_str(
        r#"{
            "id": "codex",
            "plugin_id": "grein-acp-codex",
            "display_name": "Codex",
            "kind": "acp",
            "available": false
        }"#,
    )?;
    assert_eq!(provider_minimal.executable, None);
    assert!(!provider_minimal.is_default);
    assert_eq!(provider_minimal.install_hint, None);

    let legacy_meta: ThreadMeta = serde_json::from_str(
        r#"{
            "thread_id": "t-1",
            "parent_thread_id": null,
            "inherited_turn_count": 0
        }"#,
    )?;
    assert_eq!(legacy_meta.agent_provider, None);
    let meta_with_provider = ThreadMeta {
        agent_provider: Some("claude".into()),
        ..legacy_meta
    };
    let meta_roundtrip: ThreadMeta =
        serde_json::from_str(&serde_json::to_string(&meta_with_provider)?)?;
    assert_eq!(meta_roundtrip, meta_with_provider);

    let legacy_hydrated: HydratedThread = serde_json::from_str(
        r#"{
            "id": "t-1",
            "title": "Main",
            "turns": []
        }"#,
    )?;
    assert_eq!(legacy_hydrated.agent_provider, None);
    let hydrated_with_provider = HydratedThread {
        agent_provider: Some("claude".into()),
        ..legacy_hydrated
    };
    let hydrated_roundtrip: HydratedThread =
        serde_json::from_str(&serde_json::to_string(&hydrated_with_provider)?)?;
    assert_eq!(hydrated_roundtrip, hydrated_with_provider);

    let legacy_summary: ThreadSummary =
        serde_json::from_str(r#"{"thread_id":"t-1","parent_node_id":"n-0","merged":false}"#)?;
    assert_eq!(legacy_summary.agent_provider, None);
    let summary_none_json = serde_json::to_string(&legacy_summary)?;
    assert!(!summary_none_json.contains("agent_provider"));

    let summary_with_provider = ThreadSummary {
        agent_provider: Some("claude".into()),
        ..legacy_summary
    };
    let summary_some_json = serde_json::to_string(&summary_with_provider)?;
    assert!(summary_some_json.contains("\"agent_provider\":\"claude\""));
    let summary_roundtrip: ThreadSummary = serde_json::from_str(&summary_some_json)?;
    assert_eq!(summary_roundtrip, summary_with_provider);

    Ok(())
}
