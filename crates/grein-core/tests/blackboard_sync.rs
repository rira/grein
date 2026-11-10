use grein_core::blackboard::{
    Blackboard, BlackboardEntry, BlackboardEntryId, BlackboardEntryKind, BoundedScratchpad,
    KnowledgeImport,
};
use grein_core::model::{AuthorRole, NodeId, ThreadId};
use grein_core::storage::sqlite::SqliteDagStorage;
use grein_core::sync::{
    build_context_prompt, build_context_prompt_with_blackboard, import_thread_knowledge,
    synthesize_thread_merge, ContextCompiler, ContextPayload,
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
