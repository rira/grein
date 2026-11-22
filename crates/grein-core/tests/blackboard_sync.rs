use grein_core::blackboard::{
    Blackboard, BlackboardEntry, BlackboardEntryId, BlackboardEntryKind, BoundedScratchpad,
    KnowledgeImport,
};
use grein_core::model::{AuthorRole, ConversationNode, NodeId, ThreadId};
use grein_core::storage::sqlite::SqliteDagStorage;
use grein_core::sync::{
    build_context_prompt, build_context_prompt_with_blackboard, format_whiteboard_delta_prefix,
    import_thread_knowledge, synthesize_thread_merge, ContextCompiler, ContextPayload,
};

#[test]
fn test_sqlite_storage_blackboard_entries_table() -> Result<(), Box<dyn std::error::Error>> {
    let storage = SqliteDagStorage::in_memory()?;

    // Acceptance criterion 1: SQLite table blackboard_entries exists
    assert!(
        storage.table_exists("blackboard_entries")?,
        "blackboard_entries table must exist after migrations"
    );

    let d_entry = BlackboardEntry::new("db_engine", "PostgreSQL 16", BlackboardEntryKind::Decision);
    let inv_entry = BlackboardEntry::new(
        "zero_null",
        "Never return null; use Option/Result",
        BlackboardEntryKind::Invariant,
    );
    let port_entry = BlackboardEntry::new("api_port", "8080", BlackboardEntryKind::PortMapping);
    let file_entry = BlackboardEntry::new(
        "crates/grein-core/src/lib.rs",
        "crates/grein-core/src/lib.rs",
        BlackboardEntryKind::ActiveFile,
    );

    storage.insert_blackboard_entry(&d_entry)?;
    storage.insert_blackboard_entry(&inv_entry)?;
    storage.insert_blackboard_entry(&port_entry)?;
    storage.insert_blackboard_entry(&file_entry)?;

    let fetched = storage
        .get_blackboard_entry("db_engine")?
        .expect("Entry should exist");
    assert_eq!(fetched.id, d_entry.id);
    let _id_type_check: BlackboardEntryId = fetched.id;
    assert_eq!(fetched.key, "db_engine");
    assert_eq!(fetched.value, "PostgreSQL 16");
    assert_eq!(fetched.kind, BlackboardEntryKind::Decision);

    let all_entries = storage.list_blackboard_entries()?;
    assert_eq!(all_entries.len(), 4);

    let deleted = storage.delete_blackboard_entry("db_engine")?;
    assert!(deleted);
    assert!(storage.get_blackboard_entry("db_engine")?.is_none());

    Ok(())
}

#[test]
fn test_blackboard_in_memory_and_markdown_rendering() {
    let mut bb = Blackboard::with_scratchpad_bound(512);

    bb.add_decision("auth_mechanism", "Ed25519 JWT Bearer tokens");
    bb.add_invariant("purity", "Zero class inheritance in domain core");
    bb.set_port_mapping("backend_http", 9000);
    bb.register_active_file("crates/grein-core/src/model.rs");
    bb.scratchpad_mut()
        .set_content("Active architectural discussion on anchor trees.")
        .expect("Scratchpad set should succeed");

    assert_eq!(bb.entries().len(), 4);
    assert_eq!(bb.find_by_kind(BlackboardEntryKind::Decision).len(), 1);
    assert_eq!(bb.find_by_kind(BlackboardEntryKind::Invariant).len(), 1);
    assert_eq!(bb.find_by_kind(BlackboardEntryKind::PortMapping).len(), 1);
    assert_eq!(bb.find_by_kind(BlackboardEntryKind::ActiveFile).len(), 1);

    let md = bb.to_markdown();
    assert!(md.contains("## Active Architectural Decisions"));
    assert!(md.contains("auth_mechanism"));
    assert!(md.contains("Ed25519 JWT Bearer tokens"));
    assert!(md.contains("## Established Domain Invariants"));
    assert!(md.contains("purity"));
    assert!(md.contains("## Port Mappings"));
    assert!(md.contains("backend_http"));
    assert!(md.contains("9000"));
    assert!(md.contains("## Active Files Registry"));
    assert!(md.contains("crates/grein-core/src/model.rs"));
    assert!(md.contains("## Scratchpad Notes"));
    assert!(md.contains("Active architectural discussion on anchor trees."));
}

#[test]
fn test_compiler_build_context_prompt_injects_blackboard() -> Result<(), Box<dyn std::error::Error>>
{
    let thread_id = ThreadId::new_v7();

    // Default stub build_context_prompt
    let empty_payload = build_context_prompt(thread_id)?;
    assert_eq!(empty_payload.thread_id, Some(thread_id));
    assert!(empty_payload.blackboard_notes.is_empty());

    // Acceptance criterion 2: Compiler method injects active Blackboard notes
    let mut bb = Blackboard::new();
    bb.add_decision("port", "Port 8080 selected for REST gateway");
    bb.register_active_file("crates/grein-core/src/sync.rs");

    let compiler = ContextCompiler::new("You are an expert pair-programming AI assistant.");
    let payload = compiler.build_context_prompt_with_blackboard(thread_id, &bb)?;

    // Free function contract test
    let free_fn_payload: ContextPayload = build_context_prompt_with_blackboard(
        thread_id,
        "You are an expert pair-programming AI assistant.",
        &bb,
    )?;
    assert_eq!(payload, free_fn_payload);

    assert_eq!(payload.thread_id, Some(thread_id));
    assert!(payload.blackboard_notes.contains("Port 8080"));
    assert_eq!(payload.active_files.len(), 1);
    assert_eq!(payload.active_files[0], "crates/grein-core/src/sync.rs");

    let prompt = payload.build_system_prompt();
    assert!(prompt.contains("You are an expert pair-programming AI assistant."));
    assert!(prompt.contains("# Shared Blackboard Working Memory"));
    assert!(prompt.contains("Port 8080"));

    Ok(())
}

#[test]
fn test_on_demand_knowledge_import_synthesis() -> Result<(), Box<dyn std::error::Error>> {
    let source_thread_id = ThreadId::new_v7();
    let target_thread_id = ThreadId::new_v7();
    let parent_node_id = NodeId::new_v7();

    // Acceptance criterion 3: On-demand thread import mechanism pulls synthesized decision summary
    let import = KnowledgeImport::new(
        "Thread 1",
        "DB Schema",
        "Finalized user table with columns id, email, created_at",
    );

    let raw_turn = import.to_turn_content();
    let parsed = KnowledgeImport::parse(&raw_turn).expect("Import turn must parse");
    assert_eq!(parsed.source_thread, "Thread 1");
    assert_eq!(parsed.topic, "DB Schema");
    assert_eq!(
        parsed.conclusion,
        "Finalized user table with columns id, email, created_at"
    );

    let node = import_thread_knowledge(
        source_thread_id,
        target_thread_id,
        &import,
        Some(parent_node_id),
    )?;

    assert_eq!(node.role, AuthorRole::System);
    assert_eq!(node.parent_id, Some(parent_node_id));
    assert!(node.content.contains("[Context Import from Thread 1"));
    assert!(node.content.contains("Finalized user table"));

    Ok(())
}

#[test]
fn test_thread_merge_synthesizer_decision_landmark() -> Result<(), Box<dyn std::error::Error>> {
    let thread_id = ThreadId::new_v7();
    let trunk_head_id = NodeId::new_v7();

    // Acceptance criterion 4: Thread merge synthesizer produces permanent decision landmark
    let node = synthesize_thread_merge(
        thread_id,
        "Database Engine Selection",
        "Adopted SQLite embedded WAL for local session state and project dag trees.",
        Some(trunk_head_id),
    )?;

    assert_eq!(node.role, AuthorRole::System);
    assert_eq!(node.parent_id, Some(trunk_head_id));
    assert!(node.content.contains("[Decision Landmark"));
    assert!(node.content.contains("Database Engine Selection"));
    assert!(node.content.contains("Adopted SQLite embedded WAL"));

    Ok(())
}

#[test]
fn test_bounded_scratchpad_limit_guardrail() {
    let mut scratchpad = BoundedScratchpad::new(40);
    assert_eq!(scratchpad.remaining_capacity(), 40);

    assert!(scratchpad.append("Short note.").is_ok());
    assert_eq!(scratchpad.content(), "Short note.");

    let overflow_res =
        scratchpad.append(" This extra note is definitely going to exceed forty characters.");
    assert!(overflow_res.is_err());
    // Content unchanged on error
    assert_eq!(scratchpad.content(), "Short note.");

    scratchpad.clear();
    assert_eq!(scratchpad.remaining_capacity(), 40);
    assert!(scratchpad.content().is_empty());
}

#[test]
fn test_blackboard_notes_markdown_ordering_and_from_entries() {
    let entries = vec![
        BlackboardEntry::new("db", "SQLite WAL", BlackboardEntryKind::Decision),
        BlackboardEntry::new(
            "src/lib.rs",
            "src/lib.rs",
            BlackboardEntryKind::ActiveFile,
        ),
        BlackboardEntry::new(
            "migration_tip",
            "Run PRAGMA table_info first",
            BlackboardEntryKind::Note,
        ),
        BlackboardEntry::new(
            "retry_note",
            "Keep backoff exponential",
            BlackboardEntryKind::Note,
        ),
        BlackboardEntry::new(
            "migration_tip",
            "Inspect PRAGMA table_info before ALTER",
            BlackboardEntryKind::Note,
        ),
    ];

    let mut bb = Blackboard::from_entries(entries);
    assert_eq!(bb.entries().len(), 4);
    assert_eq!(
        bb.get("migration_tip").expect("deduplicated note").value,
        "Inspect PRAGMA table_info before ALTER"
    );

    bb.scratchpad_mut()
        .set_content("Scratchpad tail note.")
        .expect("scratchpad set");

    let md = bb.to_markdown();
    assert!(md.contains("## Notes\n"));
    assert!(md.contains("- **migration_tip**: Inspect PRAGMA table_info before ALTER\n"));
    assert!(md.contains("- **retry_note**: Keep backoff exponential\n"));

    let active_files_pos = md
        .find("## Active Files Registry")
        .expect("Active Files Registry section");
    let notes_pos = md.find("## Notes").expect("Notes section");
    let scratchpad_pos = md
        .find("## Scratchpad Notes")
        .expect("Scratchpad Notes section");

    assert!(active_files_pos < notes_pos);
    assert!(notes_pos < scratchpad_pos);
}

#[test]
fn test_format_whiteboard_delta_prefix() {
    let empty_bb = Blackboard::new();
    assert_eq!(format_whiteboard_delta_prefix(&empty_bb), "");

    let mut bb = Blackboard::new();
    bb.add_entry(BlackboardEntry::new(
        "finding",
        "Token TTL is 15m",
        BlackboardEntryKind::Note,
    ));

    let prefix = format_whiteboard_delta_prefix(&bb);
    let expected_md = bb.to_markdown();
    assert_eq!(
        prefix,
        format!(
            "[Shared Whiteboard Updated]\n{}\n\n---\n\n",
            expected_md.trim()
        )
    );
}

#[test]
fn test_build_acp_system_prompt_append_sections_and_utf8_truncation() {
    let root_id = NodeId::new_v7();
    let trunk_1 = ConversationNode::new(
        root_id,
        None::<NodeId>,
        AuthorRole::User,
        "Oldest trunk turn with extra padding".to_string(),
    );
    let trunk_2 = ConversationNode::new(
        NodeId::new_v7(),
        Some(root_id),
        AuthorRole::Assistant,
        "Second trunk turn".to_string(),
    );
    let branch_1 = ConversationNode::new(
        NodeId::new_v7(),
        Some(trunk_2.id),
        AuthorRole::User,
        "Branch question".to_string(),
    );
    let branch_2 = ConversationNode::new(
        NodeId::new_v7(),
        Some(branch_1.id),
        AuthorRole::ToolResult,
        "Branch result".to_string(),
    );

    let mut payload = ContextPayload::new(
        "Base system prompt.",
        "## Notes\n- **invariant**: Preserve UTF-8",
    );
    payload.anchor_header = Some("## Auth Design".to_string());
    payload.trunk_path = vec![trunk_1, trunk_2];
    payload.branch_turns = vec![branch_1, branch_2];

    let without_branch = payload.build_acp_system_prompt_append(false, 0);
    assert!(without_branch.contains("Base system prompt."));
    assert!(without_branch.contains("# Shared Blackboard Working Memory\n## Notes"));
    assert!(without_branch.contains("# Branch Context\n## Auth Design"));
    assert!(without_branch.contains("# Ancestor Conversation Trunk\n[user]\nOldest trunk turn"));
    assert!(without_branch.contains("[assistant]\nSecond trunk turn"));
    assert!(!without_branch.contains("# Branch History"));
    assert!(!without_branch.contains("Branch question"));

    let full = payload.build_acp_system_prompt_append(true, 0);
    assert!(full.contains("# Branch History\n[user]\nBranch question\n\n[tool_result]\nBranch result"));

    // UTF-8 safe truncation preserving base, whiteboard, and branch context
    let mut utf8_payload = ContextPayload::new(
        "Base prompt.",
        "## Notes\n- **wb**: Keep Whiteboard Intact",
    );
    utf8_payload.anchor_header = Some("## Unicode Section".to_string());
    utf8_payload.trunk_path = vec![
        ConversationNode::new(
            NodeId::new_v7(),
            None::<NodeId>,
            AuthorRole::User,
            "Older entry 🚀✨🔥".to_string(),
        ),
        ConversationNode::new(
            NodeId::new_v7(),
            None::<NodeId>,
            AuthorRole::Assistant,
            format!("{} tail-marker", "🦀é日本語".repeat(30)),
        ),
    ];

    let preserved_len = utf8_payload
        .build_acp_system_prompt_append(false, 0)
        .split("# Ancestor Conversation Trunk")
        .next()
        .unwrap()
        .trim_end()
        .chars()
        .count();
    let max_chars = preserved_len + 60;
    let truncated = utf8_payload.build_acp_system_prompt_append(false, max_chars);

    assert!(truncated.contains("Base prompt."));
    assert!(truncated.contains("# Shared Blackboard Working Memory\n## Notes\n- **wb**: Keep Whiteboard Intact"));
    assert!(truncated.contains("# Branch Context\n## Unicode Section"));
    assert!(truncated.contains("tail-marker"));
    assert!(!truncated.contains("Older entry"));
    assert!(truncated.chars().count() <= max_chars);

    // When preserved sections already exceed max_chars, preserved sections stay intact.
    let small_limit = utf8_payload.build_acp_system_prompt_append(false, 10);
    assert!(small_limit.contains("Base prompt."));
    assert!(small_limit.contains("# Shared Blackboard Working Memory\n## Notes\n- **wb**: Keep Whiteboard Intact"));
    assert!(small_limit.contains("# Branch Context\n## Unicode Section"));
    assert!(!small_limit.contains("# Ancestor Conversation Trunk"));

    // Branch turns preserved over older trunk turns when truncating with include_branch_turns=true.
    let mut branch_payload = ContextPayload::new("Sys.", "Board.");
    branch_payload.trunk_path = vec![ConversationNode::new(
        NodeId::new_v7(),
        None::<NodeId>,
        AuthorRole::User,
        "Very long trunk context ".repeat(10),
    )];
    branch_payload.branch_turns = vec![
        ConversationNode::new(
            NodeId::new_v7(),
            None::<NodeId>,
            AuthorRole::User,
            "Old branch turn".to_string(),
        ),
        ConversationNode::new(
            NodeId::new_v7(),
            None::<NodeId>,
            AuthorRole::Assistant,
            "Newest branch reply".to_string(),
        ),
    ];
    let branch_budget = branch_payload
        .build_acp_system_prompt_append(true, 0)
        .chars()
        .count()
        / 2;
    let branch_trimmed = branch_payload.build_acp_system_prompt_append(true, branch_budget);
    assert!(branch_trimmed.contains("Sys."));
    assert!(branch_trimmed.contains("# Shared Blackboard Working Memory\nBoard."));
    assert!(branch_trimmed.contains("Newest branch reply"));
    assert!(branch_trimmed.chars().count() <= branch_budget);
    assert!(!branch_trimmed.contains("Very long trunk context"));

    // Empty payload produces empty string.
    assert_eq!(ContextPayload::default().build_acp_system_prompt_append(true, 100), "");
}

#[test]
fn test_build_acp_system_prompt_append_preserves_decision_landmarks_across_truncation(
) -> Result<(), Box<dyn std::error::Error>> {
    let storage = SqliteDagStorage::in_memory()?;

    let root = ConversationNode::new(
        NodeId::new_v7(),
        None::<NodeId>,
        AuthorRole::System,
        "".to_string(),
    );
    storage.insert_node(&root)?;

    let old_turn = ConversationNode::new(
        NodeId::new_v7(),
        Some(root.id),
        AuthorRole::User,
        "Very long legacy trunk turn ".repeat(30),
    );
    storage.insert_node(&old_turn)?;

    let source_tid = ThreadId::new_v7();
    let landmark_node = synthesize_thread_merge(
        source_tid,
        "Auth Protocol",
        "Adopted Ed25519 tokens with 15m TTL",
        Some(old_turn.id),
    )?;
    storage.insert_node(&landmark_node)?;

    let recent_turn = ConversationNode::new(
        NodeId::new_v7(),
        Some(landmark_node.id),
        AuthorRole::User,
        "Follow-up trunk turn ".repeat(20),
    );
    storage.insert_node(&recent_turn)?;

    let child_tid = storage.create_thread(recent_turn.id, grein_core::model::AnchorId::new_v7())?;
    let branch_turn = ConversationNode::new(
        NodeId::new_v7(),
        Some(recent_turn.id),
        AuthorRole::Assistant,
        "Branch working note".to_string(),
    );
    storage.insert_node(&branch_turn)?;
    storage.update_thread_leaf(child_tid, branch_turn.id)?;

    let mut bb = Blackboard::new();
    bb.add_decision("bb_key", "Blackboard fact");

    let payload = ContextCompiler::new("System base.")
        .compile_thread_context(&storage, child_tid, &bb)?;
    assert_eq!(payload.decision_landmarks.len(), 1);

    let full = payload.build_acp_system_prompt_append(true, 0);
    assert!(full.contains("System base."));
    assert!(full.contains("# Shared Blackboard Working Memory"));
    assert!(full.contains("# Decision Landmarks"));
    assert!(full.contains("Adopted Ed25519 tokens with 15m TTL"));

    let small_budget = 260;
    let truncated = payload.build_acp_system_prompt_append(true, small_budget);
    assert!(truncated.contains("System base."));
    assert!(truncated.contains("# Shared Blackboard Working Memory"));
    assert!(truncated.contains("# Decision Landmarks"));
    assert!(
        truncated.contains("Adopted Ed25519 tokens with 15m TTL"),
        "decision_landmarks must survive history truncation: {truncated}"
    );
    assert!(!truncated.contains("Very long legacy trunk turn"));

    Ok(())
}
