use grein_core::anchor::{parse_anchors, AnchorKind};
use grein_core::blackboard::{
    Blackboard, BlackboardEntry, BlackboardEntryKind, BoundedScratchpad, KnowledgeImport,
};
use grein_core::dag::{get_trunk_path, ConversationDag, DagStorage};
use grein_core::model::{Anchor, AnchorId, AuthorRole, ConversationNode, NodeId, ThreadId, ThreadStatus};
use grein_core::storage::sqlite::SqliteStorage;
use grein_core::sync::{ContextCompiler, ContextPayload};
use grein_core::{CoreError, SettingsFormat, SettingsPatch, ThreadKind, UserPrefsStore};
use rusqlite::{params, Connection};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr;
use uuid::Uuid;

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir().join(format!("{prefix}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
fn dag_in_memory_and_trait_storage_full_coverage() -> Result<(), Box<dyn std::error::Error>> {
    let mut mem_dag = ConversationDag::default();
    let root = ConversationNode::new_root(AuthorRole::System, "root");
    let child = ConversationNode::new(NodeId::new_v7(), Some(root.id), AuthorRole::User, "hello".into());
    mem_dag.insert_node(root.clone());
    mem_dag.insert_node(child.clone());

    assert_eq!(mem_dag.get_node(&root.id.0)?, &root);
    assert_eq!(mem_dag.get_node(&child.id.0)?, &child);
    let missing_uuid = Uuid::now_v7();
    assert!(matches!(
        mem_dag.get_node(&missing_uuid),
        Err(CoreError::NodeNotFound(_))
    ));

    let storage = SqliteStorage::open_in_memory()?;
    assert!(storage.is_initialized());

    DagStorage::insert_node(&storage, &root)?;
    DagStorage::insert_node(&storage, &child)?;
    assert_eq!(DagStorage::get_node(&storage, &root.id.0)?, Some(root.clone()));
    assert_eq!(DagStorage::get_children(&storage, &root.id.0)?, vec![child.clone()]);
    assert_eq!(DagStorage::list_nodes(&storage)?, vec![root.clone(), child.clone()]);
    assert_eq!(
        get_trunk_path(&storage, &child.id.0)?,
        vec![root.clone(), child.clone()]
    );
    assert!(matches!(
        get_trunk_path(&storage, &missing_uuid),
        Err(CoreError::NodeNotFound(_))
    ));

    let anchor_id = AnchorId::new_v7();
    let anchor = Anchor {
        level: 2,
        title: "Design".into(),
        slug: "design".into(),
        line_number: 5,
    };
    storage.insert_anchor(&anchor_id.0, &anchor)?;
    assert_eq!(DagStorage::get_anchor(&storage, &anchor_id.0)?, Some(anchor));
    assert_eq!(DagStorage::get_anchor(&storage, &Uuid::now_v7())?, None);

    let tid = DagStorage::create_thread(&storage, &root.id.0, &anchor_id.0)?;
    let fetched = DagStorage::get_thread(&storage, &tid)?
        .expect("created thread");
    assert_eq!(fetched.id.0, tid);
    assert_eq!(DagStorage::list_threads(&storage)?.len(), 1);

    DagStorage::update_thread_leaf(&storage, &tid, &child.id.0)?;
    DagStorage::update_thread_status(&storage, &tid, ThreadStatus::Executing)?;
    let updated = DagStorage::get_thread(&storage, &tid)?.expect("updated thread");
    assert_eq!(updated.active_leaf_id, child.id);
    assert_eq!(updated.status, ThreadStatus::Executing);

    struct CycleMockStorage {
        a: ConversationNode,
        b: ConversationNode,
    }

    impl DagStorage for CycleMockStorage {
        fn insert_node(&self, _: &ConversationNode) -> Result<(), CoreError> {
            Ok(())
        }
        fn get_node(&self, _: &Uuid) -> Result<Option<ConversationNode>, CoreError> {
            Ok(None)
        }
        fn get_children(&self, _: &Uuid) -> Result<Vec<ConversationNode>, CoreError> {
            Ok(Vec::new())
        }
        fn get_trunk_path(&self, leaf_id: &Uuid) -> Result<Vec<ConversationNode>, CoreError> {
            if *leaf_id == self.b.id.0 || *leaf_id == self.a.id.0 {
                Err(CoreError::CycleDetected("cycle in mock".into()))
            } else {
                Err(CoreError::NodeNotFound(leaf_id.to_string()))
            }
        }
        fn create_thread(&self, _: &Uuid, _: &Uuid) -> Result<Uuid, CoreError> {
            Ok(Uuid::now_v7())
        }
        fn get_thread(&self, _: &Uuid) -> Result<Option<grein_core::model::Thread>, CoreError> {
            Ok(None)
        }
        fn list_threads(&self) -> Result<Vec<grein_core::model::Thread>, CoreError> {
            Ok(Vec::new())
        }
        fn list_nodes(&self) -> Result<Vec<ConversationNode>, CoreError> {
            Ok(vec![self.a.clone(), self.b.clone()])
        }
        fn get_anchor(&self, _: &Uuid) -> Result<Option<Anchor>, CoreError> {
            Ok(None)
        }
        fn update_thread_leaf(&self, _: &Uuid, _: &Uuid) -> Result<(), CoreError> {
            Ok(())
        }
        fn update_thread_status(&self, _: &Uuid, _: ThreadStatus) -> Result<(), CoreError> {
            Ok(())
        }
    }

    let mock = CycleMockStorage { a: root, b: child };
    assert!(matches!(
        get_trunk_path(&mock, &mock.b.id.0),
        Err(CoreError::CycleDetected(_))
    ));

    Ok(())
}

#[test]
fn sqlite_storage_node_anchor_thread_errors_and_corrupt_rows() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDirGuard::new("grein-core-sqlite-hardening");
    let db_path = temp.path().join("dag.db");
    let storage = SqliteStorage::open(&db_path)?;

    let self_id = NodeId::new_v7();
    let self_parent = ConversationNode::new(self_id, Some(self_id), AuthorRole::User, "self".into());
    assert!(matches!(
        storage.insert_node(&self_parent),
        Err(CoreError::CycleDetected(_))
    ));

    let dangling = ConversationNode::new(
        NodeId::new_v7(),
        Some(NodeId::new_v7()),
        AuthorRole::User,
        "orphan".into(),
    );
    assert!(matches!(
        storage.insert_node(&dangling),
        Err(CoreError::NodeNotFound(_))
    ));

    let root1 = ConversationNode::new_root(AuthorRole::System, "r1");
    let root2 = ConversationNode::new_root(AuthorRole::User, "r2");
    let child = ConversationNode::new(NodeId::new_v7(), Some(root1.id), AuthorRole::Assistant, "c1".into());
    storage.insert_node(&root1)?;
    storage.insert_node(&root2)?;
    storage.insert_node(&child)?;

    assert_eq!(storage.list_root_nodes()?, vec![root1.clone(), root2.clone()]);
    assert_eq!(storage.get_children(root1.id)?, vec![child.clone()]);

    let a1_id = AnchorId::new_v7();
    let a1 = Anchor {
        level: 2,
        title: "First".into(),
        slug: "first".into(),
        line_number: 10,
    };
    let a2_id = AnchorId::new_v7();
    let a2 = Anchor {
        level: 3,
        title: "Second".into(),
        slug: "second".into(),
        line_number: 2,
    };
    storage.insert_anchor(&a1_id.0, &a1)?;
    storage.insert_anchor(&a2_id.0, &a2)?;
    storage.link_node_anchor(child.id, a1_id)?;
    storage.link_node_anchor(child.id, a1_id)?;
    storage.link_node_anchor(child.id, a2_id)?;
    assert_eq!(storage.list_anchors_for_node(child.id)?, vec![a2, a1]);

    let missing_node = NodeId::new_v7();
    assert!(matches!(
        storage.get_trunk_path(missing_node),
        Err(CoreError::NodeNotFound(_))
    ));

    let tid = storage.create_thread(root1.id, a1_id)?;
    storage.update_active_leaf(tid, child.id)?;
    assert_eq!(storage.get_active_leaf(tid)?, child.id);
    assert!(storage.delete_thread(tid)?);
    assert!(!storage.delete_thread(tid)?);

    let missing_tid = ThreadId::new_v7();
    assert!(matches!(
        storage.get_thread_leaf(missing_tid),
        Err(CoreError::ThreadNotFound(_))
    ));
    assert!(matches!(
        storage.get_thread_state(missing_tid),
        Err(CoreError::ThreadNotFound(_))
    ));
    assert!(matches!(
        storage.update_thread_leaf(tid, missing_node),
        Err(CoreError::NodeNotFound(_))
    ));
    assert!(matches!(
        storage.update_thread_leaf(missing_tid, child.id),
        Err(CoreError::ThreadNotFound(_))
    ));
    assert!(matches!(
        storage.update_thread_status(missing_tid, ThreadStatus::Error),
        Err(CoreError::ThreadNotFound(_))
    ));
    assert!(matches!(
        storage.update_thread_state(missing_tid, ThreadStatus::Idle),
        Err(CoreError::ThreadNotFound(_))
    ));

    assert!(storage.get_blackboard_entry("nonexistent")?.is_none());
    assert!(!storage.delete_blackboard_entry("nonexistent")?);

    // Inject cycle + corrupt rows via raw connection to verify decoding & cycle guards.
    let raw = Connection::open(&db_path)?;
    raw.execute(
        "UPDATE nodes SET parent_id = ?1 WHERE id = ?2",
        params![child.id.to_string(), root1.id.to_string()],
    )?;
    assert!(matches!(
        storage.get_trunk_path(child.id),
        Err(CoreError::CycleDetected(_))
    ));
    assert!(matches!(
        get_trunk_path(&storage, &child.id.0),
        Err(CoreError::CycleDetected(_))
    ));
    raw.execute(
        "UPDATE nodes SET parent_id = NULL WHERE id = ?1",
        params![root1.id.to_string()],
    )?;

    let bad_thread_kind_id = ThreadId::new_v7();
    raw.execute(
        "INSERT INTO threads (id, parent_node_id, anchor_id, active_leaf_id, status, created_at, kind)
         VALUES (?1, ?2, ?3, ?2, 'idle', 10, 'invalid_kind')",
        params![
            bad_thread_kind_id.to_string(),
            root1.id.to_string(),
            a1_id.to_string()
        ],
    )?;
    assert!(matches!(
        storage.get_thread(bad_thread_kind_id),
        Err(CoreError::Serialization(_))
    ));
    raw.execute(
        "DELETE FROM threads WHERE id = ?1",
        params![bad_thread_kind_id.to_string()],
    )?;

    let bad_uuid_thread_id = ThreadId::new_v7();
    raw.execute(
        "INSERT INTO threads (id, parent_node_id, anchor_id, active_leaf_id, status, created_at, parent_thread_id, kind)
         VALUES (?1, ?2, ?3, ?2, 'idle', 11, 'not-a-uuid', 'agent')",
        params![
            bad_uuid_thread_id.to_string(),
            root1.id.to_string(),
            a1_id.to_string()
        ],
    )?;
    assert!(matches!(
        storage.get_thread(bad_uuid_thread_id),
        Err(CoreError::Storage(_))
    ));
    assert!(matches!(
        storage.list_threads(),
        Err(CoreError::Storage(_))
    ));
    raw.execute(
        "DELETE FROM threads WHERE id = ?1",
        params![bad_uuid_thread_id.to_string()],
    )?;

    raw.execute(
        "INSERT INTO blackboard_entries (id, key, value, kind, created_at)
         VALUES (?1, 'bad_kind_key', 'v', 'bogus_bb_kind', 1)",
        params![Uuid::now_v7().to_string()],
    )?;
    assert!(matches!(
        storage.get_blackboard_entry("bad_kind_key"),
        Err(CoreError::Serialization(_))
    ));
    assert!(matches!(
        storage.list_blackboard_entries(),
        Err(CoreError::Serialization(_))
    ));
    raw.execute("DELETE FROM blackboard_entries WHERE key = 'bad_kind_key'", [])?;

    raw.execute(
        "INSERT INTO blackboard_entries (id, key, value, kind, created_at)
         VALUES ('corrupt-uuid', 'bad_uuid_key', 'v', 'note', 2)",
        [],
    )?;
    assert!(matches!(
        storage.get_blackboard_entry("bad_uuid_key"),
        Err(CoreError::Storage(_))
    ));
    assert!(matches!(
        storage.list_blackboard_entries(),
        Err(CoreError::Storage(_))
    ));
    raw.execute("DELETE FROM blackboard_entries WHERE key = 'bad_uuid_key'", [])?;

    raw.execute(
        "INSERT INTO nodes (id, parent_id, role, content, created_at)
         VALUES ('not-a-node-uuid', NULL, 'user', 'bad', 99)",
        [],
    )?;
    assert!(matches!(
        storage.list_nodes(),
        Err(CoreError::Storage(_))
    ));
    raw.execute("DELETE FROM nodes WHERE id = 'not-a-node-uuid'", [])?;

    raw.execute(
        "INSERT INTO anchors (id, level, title, slug, line_number, created_at)
         VALUES ('not-an-anchor-uuid', 1, 't', 's', 1, 99)",
        [],
    )?;
    assert!(matches!(
        storage.list_anchors(),
        Err(CoreError::Storage(_))
    ));

    assert_eq!(ThreadKind::from_str("agent")?, ThreadKind::Agent);
    assert_eq!(ThreadKind::from_str("terminal")?, ThreadKind::Terminal);
    assert_eq!(ThreadKind::Terminal.to_string(), "terminal");
    assert!(matches!(
        ThreadKind::from_str("unknown"),
        Err(CoreError::Serialization(_))
    ));
    assert_eq!(
        BlackboardEntryKind::from_str("note")?,
        BlackboardEntryKind::Note
    );
    assert_eq!(BlackboardEntryKind::ActiveFile.to_string(), "active_file");
    assert!(matches!(
        BlackboardEntryKind::from_str("unknown"),
        Err(CoreError::Serialization(_))
    ));

    Ok(())
}

#[test]
fn sync_context_compiler_errors_anchor_and_tight_budget_trimming(
) -> Result<(), Box<dyn std::error::Error>> {
    let storage = SqliteStorage::in_memory()?;
    let compiler = ContextCompiler::default();
    assert_eq!(compiler.base_prompt(), "");

    let missing_tid = ThreadId::new_v7();
    assert!(matches!(
        compiler.compile_thread_context(&storage, missing_tid),
        Err(CoreError::ThreadNotFound(_))
    ));

    let root = ConversationNode::new_root(AuthorRole::System, "init");
    let trunk_child =
        ConversationNode::new(NodeId::new_v7(), Some(root.id), AuthorRole::User, "trunk".into());
    let sibling = ConversationNode::new(
        NodeId::new_v7(),
        Some(root.id),
        AuthorRole::User,
        "sibling".into(),
    );
    storage.insert_node(&root)?;
    storage.insert_node(&trunk_child)?;
    storage.insert_node(&sibling)?;

    let anchor_id = AnchorId::new_v7();
    storage.insert_anchor(
        &anchor_id.0,
        &Anchor {
            level: 2,
            title: "Branch Anchor".into(),
            slug: "branch-anchor".into(),
            line_number: 3,
        },
    )?;

    let bad_tid = storage.create_thread(trunk_child.id, anchor_id)?;
    storage.update_thread_leaf(bad_tid, sibling.id)?;
    assert!(matches!(
        compiler.compile_thread_context(&storage, bad_tid),
        Err(CoreError::SyncError(msg)) if msg.contains("not a descendant")
    ));

    let good_tid = storage.create_thread(trunk_child.id, anchor_id)?;
    let branch_node = ConversationNode::new(
        NodeId::new_v7(),
        Some(trunk_child.id),
        AuthorRole::System,
        "[Decision Landmark (Thread x - Topic)]:\nSummary".into(),
    );
    storage.insert_node(&branch_node)?;
    storage.update_thread_leaf(good_tid, branch_node.id)?;

    let compiled = ContextCompiler::new("Sys").compile_thread_context(&storage, good_tid)?;
    assert_eq!(compiled.anchor_header.as_deref(), Some("## Branch Anchor"));
    assert_eq!(compiled.decision_landmarks.len(), 1);

    let single_branch = ContextPayload {
        branch_turns: vec![ConversationNode::new_root(
            AuthorRole::User,
            "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        )],
        ..ContextPayload::default()
    }
    .with_thread(good_tid);
    // "# Branch History\n" = 17 chars, "[user]\n" = 7 chars -> total prefix 24 chars.
    let mid_budget = single_branch.build_acp_system_prompt_append(true, 30);
    assert_eq!(mid_budget, "# Branch History\n[user]\nUVWXYZ");

    let tight_entry_budget = single_branch.build_acp_system_prompt_append(true, 20);
    assert_eq!(tight_entry_budget, "# Branch History\nXYZ");

    let tiny_budget = single_branch.build_acp_system_prompt_append(true, 10);
    assert_eq!(tiny_budget, "QRSTUVWXYZ");

    let single_trunk_with_preserved = ContextPayload {
        base_system_prompt: "Sys".into(),
        trunk_path: vec![ConversationNode::new_root(
            AuthorRole::Assistant,
            "abcdefghijklmnopqrstuvwxyz",
        )],
        ..ContextPayload::default()
    };
    // preserved "Sys" (3) + "\n\n" (2) = 5; "# Ancestor Conversation Trunk\n" = 30 chars.
    // max_chars = 15 -> history_budget = 10 <= header_chars (30) -> returns preserved only.
    let header_exceeds_budget = single_trunk_with_preserved.build_acp_system_prompt_append(false, 15);
    assert_eq!(header_exceeds_budget, "Sys");

    // max_chars = 5 + 30 + 15 = 50 -> single trunk node trims content tail (12-char "[assistant]\n" leaves 3 chars).
    let trunk_tail = single_trunk_with_preserved.build_acp_system_prompt_append(false, 50);
    assert_eq!(
        trunk_tail,
        "Sys\n\n# Ancestor Conversation Trunk\n[assistant]\nxyz"
    );

    Ok(())
}

#[test]
fn prefs_toml_json_store_edge_cases_and_bom() -> Result<(), Box<dyn std::error::Error>> {
    let toml_escapes = r#"
[theme]
unicode8 = "\U0001F980"
ctrl_bf = "a\bb\fc"
quoted_key = 'literal'
'single quoted key' = "ok"
["spaced section"]
count = 1_000
neg = -42
pos = +12
sci = 1e5
sci_frac = 1_0.2_5e-2
ctrl_bell_val = "x"
"#;
    let patch = UserPrefsStore::parse_settings_patch(toml_escapes, Some(SettingsFormat::Toml))?;
    assert_eq!(patch.version, 1);
    assert_eq!(
        patch.preferences.get("theme"),
        Some(&grein_core::SectionPatch::Entries(BTreeMap::from([
            ("ctrl_bf".into(), Some("a\u{0008}b\u{000C}c".into())),
            ("quoted_key".into(), Some("literal".into())),
            ("single quoted key".into(), Some("ok".into())),
            ("unicode8".into(), Some("🦀".into())),
        ])))
    );
    assert_eq!(
        patch.preferences.get("spaced section"),
        Some(&grein_core::SectionPatch::Entries(BTreeMap::from([
            ("count".into(), Some("1000".into())),
            ("ctrl_bell_val".into(), Some("x".into())),
            ("neg".into(), Some("-42".into())),
            ("pos".into(), Some("+12".into())),
            ("sci".into(), Some("1e5".into())),
            ("sci_frac".into(), Some("10.25e-2".into())),
        ])))
    );

    let version_underscored = "version = 1_000\n[theme]\nname = \"ok\"\n";
    assert_eq!(
        UserPrefsStore::parse_settings_patch(version_underscored, None)?.version,
        1000
    );

    let bad_toml_cases = [
        "[theme]\nx = \"\\U00110000\"\n",
        "[theme]\nx = \"\\uD800\"\n",
        "[theme]\nx = \"\\u12\"\n",
        "[theme]\nx = \"\\uZZZZ\"\n",
        "[theme]\nx = \"a\x01b\"\n",
        "[theme]\nx = 'a\x01b'\n",
        "[theme]\nx = 1__000\n",
        "[theme]\nx = _100\n",
        "[theme]\nx = 100_\n",
        "[theme]\nx = 1e_5\n",
        "[theme]\nx = 1e\n",
        "[theme]\nx = 1._5\n",
        "[theme]\nx = .5\n",
        "version = 1\nversion = 2\n",
        "version = 9999999999999999\n",
        "[theme]\na = \"1\"\na = \"2\"\n",
        "a = null\na = null\n",
        "\"\" = \"empty\"\n",
        "name = \"root_scalar\"\n[theme]\naccent = \"#fff\"\n",
    ];
    for raw in bad_toml_cases {
        assert!(
            UserPrefsStore::parse_settings_patch(raw, Some(SettingsFormat::Toml)).is_err(),
            "expected TOML error for: {raw:?}"
        );
    }
    assert!(UserPrefsStore::parse_theme_patch(
        "extra = \"bad\"\n[theme]\nname = \"x\"\n",
        Some(SettingsFormat::Toml)
    )
    .is_err());

    let bad_json_cases = [
        r#"{"": {"a": "1"}}"#,
        r#"{"theme": {"": "1"}}"#,
        r#"{"preferences": {"": {"a": "1"}}}"#,
        r#"{"preferences": {"theme": {"": "1"}}}"#,
        r#"{"preferences": {"theme": {"a": 1}}, "version": 4294967296}"#,
    ];
    for raw in bad_json_cases {
        assert!(
            UserPrefsStore::parse_settings_patch(raw, Some(SettingsFormat::Json)).is_err(),
            "expected JSON error for: {raw}"
        );
    }

    let bad_theme_json_cases = [
        r#"{"": "dark"}"#,
        r#"{"other": {"a": "b"}}"#,
        r#"{"colors": [1, 2]}"#,
        r#"{"preferences": {"theme": {"": "1"}}}"#,
    ];
    for raw in bad_theme_json_cases {
        assert!(
            UserPrefsStore::parse_theme_patch(raw, Some(SettingsFormat::Json)).is_err(),
            "expected theme JSON error for: {raw}"
        );
    }

    // BOM detection routes `{...}` to JSON (failing serde_json) and TOML to TOML.
    let bom_json = "\u{feff}{\"theme\":{\"name\":\"nord\"}}";
    assert!(matches!(
        UserPrefsStore::parse_settings_patch(bom_json, None),
        Err(CoreError::Serialization(_))
    ));
    let bom_toml = "\u{feff}[theme]\nname = \"nord\"\n";
    assert!(matches!(
        UserPrefsStore::parse_settings_patch(bom_toml, None),
        Err(CoreError::SyncError(_))
    ));

    let store = UserPrefsStore::in_memory()?;
    store.set("theme", "ctrl", "a\x07b\r\nc")?;
    store.set("theme", "non bare", "val")?;
    let exported = store.export_string(SettingsFormat::Toml, None)?;
    assert!(exported.contains("\\u0007"));
    assert!(exported.contains("\"non bare\" = "));

    let empty_sec_patch = SettingsPatch {
        version: 1,
        preferences: BTreeMap::from([(
            "   ".into(),
            grein_core::SectionPatch::ResetToDefault,
        )]),
    };
    assert!(matches!(
        store.import_patch(&empty_sec_patch, None, false),
        Err(CoreError::SyncError(_))
    ));
    let blank_filter = store.export_snapshot(Some(&["   ".to_string()]))?;
    assert!(blank_filter.preferences.is_empty());

    Ok(())
}

#[test]
fn blackboard_and_anchor_edge_cases() {
    let custom = BlackboardEntry::with_id(
        grein_core::BlackboardEntryId::new_v7(),
        "k",
        "v",
        BlackboardEntryKind::Note,
        123,
    );
    assert_eq!(custom.created_at, 123);

    let mut bb = Blackboard::new();
    assert_eq!(bb.to_markdown(), "");
    bb.add_decision("d1", "use WAL");
    assert_eq!(bb.remove_entry("missing"), None);
    let removed = bb.remove_entry("d1").expect("removed decision");
    assert_eq!(removed.value, "use WAL");
    assert!(bb.entries().is_empty());

    bb.scratchpad_mut()
        .set_content("no trailing newline")
        .expect("set scratchpad");
    let md = bb.to_markdown();
    assert_eq!(md, "## Scratchpad Notes\nno trailing newline\n");

    let pad = BoundedScratchpad::default();
    assert_eq!(pad.max_chars(), BoundedScratchpad::DEFAULT_MAX_CHARS);
    let mut small = BoundedScratchpad::new(5);
    small.set_content("ok").expect("within limit");
    let err = small
        .set_content("too long")
        .expect_err("exceeds limit");
    assert!(matches!(
        err,
        CoreError::ScratchpadLimitExceeded {
            limit: 5,
            attempted: 8
        }
    ));
    assert_eq!(small.content(), "ok");

    let invalid_imports = [
        "Context Import from t1 (topic): val]",
        "[Context Import from t1 (topic): val",
        "[Context Import from t1 topic: val]",
        "[Context Import from t1 (topic) val]",
        "[Context Import from   (topic): val]",
        "[Context Import from t1 (): val]",
        "[Context Import from t1 (topic): ]",
    ];
    for raw in invalid_imports {
        assert_eq!(
            KnowledgeImport::parse(raw),
            None,
            "expected None for `{raw}`"
        );
    }

    let md_anchors = r#"~~~~markdown
# Ignored Inside 4-Tilde Block
~~~
# Still Ignored Because 3 Tildes Cannot Close 4
~~~~

####### Seven Hashes Is Not A Heading
#NoSpaceAfterHash
###   

1. http://short
2. https://short
3. https://example.com/very/long/descriptive/url/item
4. **   **
5. Real Section: Details here
"#;
    let anchors = parse_anchors(md_anchors);
    assert_eq!(anchors.len(), 2);
    assert_eq!(anchors[0].kind, AnchorKind::NumberedList);
    assert_eq!(
        anchors[0].title,
        "https://example.com/very/long/descriptive/url/item"
    );
    assert_eq!(anchors[1].kind, AnchorKind::NumberedList);
    assert_eq!(anchors[1].title, "Real Section");
}
