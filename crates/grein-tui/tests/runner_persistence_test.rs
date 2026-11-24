use std::path::PathBuf;

use grein_core::blackboard::{BlackboardEntry, BlackboardEntryKind};
use grein_core::model::{
    AnchorId, AuthorRole, BlackboardEntryId, ConversationNode, NodeId, ThreadId,
};
use grein_core::storage::sqlite::SqliteStorage;
use grein_tui::app::{App, ThreadItem};

#[path = "support/sqlite_persistence.rs"]
mod sqlite_persistence;
use sqlite_persistence::AppSqlitePersistenceExt;

fn temp_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "grein-runner-persist-{tag}-{}.sqlite3",
        ThreadId::now_v7()
    ))
}

fn insert_blackboard_note(storage: &SqliteStorage, key: &str, value: &str) {
    storage
        .insert_blackboard_entry(&BlackboardEntry {
            id: BlackboardEntryId::now_v7(),
            key: key.to_string(),
            value: value.to_string(),
            kind: BlackboardEntryKind::Note,
            created_at: 1,
        })
        .unwrap();
}

#[test]
fn test_sqlite_corrupt_file_missing_file_and_unopenable_parent_dir_fail_gracefully() {
    // 1. Corrupt / non-SQLite file path
    let corrupt_path = temp_path("corrupt");
    let garbage = b"NOT A SQLITE DATABASE \x00\xff\xfe";
    std::fs::write(&corrupt_path, garbage).unwrap();

    let mut app = App::new();
    app.submit_composer_prompt("hello corrupt".to_string());
    app.apply_agent_response("response");
    app.persist_state_to_sqlite(&corrupt_path);
    assert_eq!(
        std::fs::read(&corrupt_path).unwrap(),
        garbage,
        "corrupt non-DB file must not be overwritten"
    );

    let mut loaded = App::new();
    loaded.load_state_from_sqlite(&corrupt_path);
    assert!(loaded.threads.is_empty());
    assert!(loaded.active_transcript_md.is_empty());
    let _ = std::fs::remove_file(&corrupt_path);

    // 2. Non-existent path on load must not create a file
    let missing_path = temp_path("missing");
    let _ = std::fs::remove_file(&missing_path);
    let mut loaded_missing = App::new();
    loaded_missing.load_state_from_sqlite(&missing_path);
    assert!(!missing_path.exists());
    assert!(loaded_missing.threads.is_empty());

    // 3. Unopenable parent directory (regular file blocking directory creation)
    let blocker_file = temp_path("blocker");
    std::fs::write(&blocker_file, b"file-not-dir").unwrap();
    let nested_under_file = blocker_file.join("nested/state.sqlite3");
    app.persist_state_to_sqlite(&nested_under_file);
    loaded_missing.load_state_from_sqlite(&nested_under_file);
    assert!(loaded_missing.threads.is_empty());
    let _ = std::fs::remove_file(&blocker_file);

    // 4. Existing directory passed directly as db_path
    let dir_path = temp_path("dir");
    std::fs::create_dir_all(&dir_path).unwrap();
    app.persist_state_to_sqlite(&dir_path);
    loaded_missing.load_state_from_sqlite(&dir_path);
    assert!(loaded_missing.threads.is_empty());
    let _ = std::fs::remove_dir_all(&dir_path);
}

#[test]
fn test_sqlite_empty_transcript_save_and_missing_or_dangling_active_thread_recovery() {
    let db_path = temp_path("empty-dangling");
    let _ = std::fs::remove_file(&db_path);

    // 1. Empty transcript & headerless markdown must not create threads or active_thread_id
    let mut empty_app = App::new();
    empty_app.persist_state_to_sqlite(&db_path);
    empty_app.active_transcript_md = "Just plain markdown without turn headers".to_string();
    empty_app.persist_state_to_sqlite(&db_path);

    {
        let storage = SqliteStorage::open(&db_path).unwrap();
        assert!(storage.list_threads().unwrap().is_empty());
        assert!(storage
            .get_blackboard_entry("active_thread_id")
            .unwrap()
            .is_none());
    }

    let mut loaded = App::new();
    loaded.load_state_from_sqlite(&db_path);
    assert!(loaded.threads.is_empty());
    assert_eq!(loaded.active_thread_id, "trunk");

    // 2. DB with valid node+thread but missing active_thread_id -> no-op on load
    {
        let storage = SqliteStorage::open(&db_path).unwrap();
        let node_id = NodeId::now_v7();
        storage
            .insert_node(&ConversationNode::new(
                node_id,
                None::<NodeId>,
                AuthorRole::User,
                "orphan turn".to_string(),
            ))
            .unwrap();
        storage.create_thread(node_id, AnchorId::now_v7()).unwrap();
    }
    loaded.load_state_from_sqlite(&db_path);
    assert!(loaded.threads.is_empty());

    // 3. Malformed non-UUID active_thread_id -> no-op on load
    {
        let storage = SqliteStorage::open(&db_path).unwrap();
        insert_blackboard_note(&storage, "active_thread_id", "not-a-valid-uuid");
    }
    loaded.load_state_from_sqlite(&db_path);
    assert!(loaded.threads.is_empty());

    // 4. Dangling UUID active_thread_id (no matching thread row) -> no-op on load
    {
        let storage = SqliteStorage::open(&db_path).unwrap();
        insert_blackboard_note(
            &storage,
            "active_thread_id",
            &ThreadId::now_v7().to_string(),
        );
    }
    loaded.load_state_from_sqlite(&db_path);
    assert!(loaded.threads.is_empty());

    let _ = std::fs::remove_file(&db_path);
}

#[test]
fn test_sqlite_active_thread_custom_name_persist_reload_and_clear_round_trip() {
    let db_path = temp_path("custom-name");
    let _ = std::fs::remove_file(&db_path);

    let mut app = App::new();
    app.submit_composer_prompt("hello".to_string());
    app.apply_agent_response("world");
    app.threads[0].title = "Auto Title".to_string();
    app.threads[0].custom_name = Some("  Alpha Custom  ".to_string());
    app.persist_state_to_sqlite(&db_path);

    {
        let storage = SqliteStorage::open(&db_path).unwrap();
        let entry = storage
            .get_blackboard_entry("active_thread_custom_name")
            .unwrap()
            .expect("custom name entry must be persisted");
        assert_eq!(entry.value, "Alpha Custom");
    }

    let mut reloaded = App::new();
    reloaded.load_state_from_sqlite(&db_path);
    assert_eq!(reloaded.threads.len(), 1);
    assert_eq!(
        reloaded.threads[0].custom_name.as_deref(),
        Some("Alpha Custom")
    );

    // Clearing custom_name to None deletes active_thread_custom_name on persist
    reloaded.threads[0].custom_name = None;
    reloaded.persist_state_to_sqlite(&db_path);
    {
        let storage = SqliteStorage::open(&db_path).unwrap();
        assert!(storage
            .get_blackboard_entry("active_thread_custom_name")
            .unwrap()
            .is_none());
    }
    let mut cleared = App::new();
    cleared.load_state_from_sqlite(&db_path);
    assert_eq!(cleared.threads[0].custom_name, None);

    // Setting and then replacing with whitespace-only clears custom_name cleanly
    cleared.threads[0].custom_name = Some("TempName".to_string());
    cleared.persist_state_to_sqlite(&db_path);
    cleared.threads[0].custom_name = Some("   ".to_string());
    cleared.persist_state_to_sqlite(&db_path);
    let mut whitespace_cleared = App::new();
    whitespace_cleared.load_state_from_sqlite(&db_path);
    assert_eq!(whitespace_cleared.threads[0].custom_name, None);

    let _ = std::fs::remove_file(&db_path);
}

#[test]
fn test_sqlite_diverging_trunk_rewrite_prefix_truncation_and_full_replace() {
    let db_path = temp_path("diverge");
    let _ = std::fs::remove_file(&db_path);

    let mut app = App::new();
    app.submit_composer_prompt("Q1".to_string());
    app.apply_agent_response("A1");
    app.submit_composer_prompt("Q2".to_string());
    app.apply_agent_response("A2");
    app.persist_state_to_sqlite(&db_path);

    let orig_trunk_ids: Vec<NodeId> = {
        let storage = SqliteStorage::open(&db_path).unwrap();
        let tid: ThreadId = storage
            .get_blackboard_entry("active_thread_id")
            .unwrap()
            .unwrap()
            .value
            .parse()
            .ok()
            .map(ThreadId)
            .unwrap();
        let thread = storage.get_thread(tid).unwrap().unwrap();
        let trunk = storage.get_trunk_path(thread.active_leaf_id).unwrap();
        assert_eq!(trunk.len(), 4);
        trunk.into_iter().map(|n| n.id).collect()
    };

    // 1. Partial divergence (common_len = 2 < existing_trunk.len() = 4)
    let mut diverged = App::new();
    diverged.load_state_from_sqlite(&db_path);
    diverged.active_transcript_md =
        "# User: Q1\n\n## Agent (agy)\nA1\n\n# User: Q2-edited\n\n## Agent (agy)\nA2-edited\n\n"
            .to_string();
    diverged.persist_state_to_sqlite(&db_path);

    {
        let storage = SqliteStorage::open(&db_path).unwrap();
        let tid: ThreadId = storage
            .get_blackboard_entry("active_thread_id")
            .unwrap()
            .unwrap()
            .value
            .parse()
            .ok()
            .map(ThreadId)
            .unwrap();
        let thread = storage.get_thread(tid).unwrap().unwrap();
        let trunk = storage.get_trunk_path(thread.active_leaf_id).unwrap();
        assert_eq!(trunk.len(), 4);
        assert_eq!(trunk[0].id, orig_trunk_ids[0]);
        assert_eq!(trunk[1].id, orig_trunk_ids[1]);
        assert_ne!(trunk[2].id, orig_trunk_ids[2]);
        assert_eq!(trunk[2].content, "Q2-edited");
        assert_eq!(trunk[3].content, "A2-edited");
        // n1 now has both original Q2 child and diverged Q2-edited child
        assert_eq!(storage.get_children(orig_trunk_ids[1]).unwrap().len(), 2);
    }

    let mut reloaded_div = App::new();
    reloaded_div.load_state_from_sqlite(&db_path);
    assert!(reloaded_div.active_transcript_md.contains("Q2-edited"));
    assert!(reloaded_div.active_transcript_md.contains("A2-edited"));
    assert!(!reloaded_div.active_transcript_md.contains("# User: Q2\n"));

    // 2. Prefix truncation (turns.len() = 2 < existing_trunk.len() = 4) moves active_leaf_id back
    reloaded_div.active_transcript_md = "# User: Q1\n\n## Agent (agy)\nA1\n\n".to_string();
    reloaded_div.persist_state_to_sqlite(&db_path);

    let mut reloaded_trunc = App::new();
    reloaded_trunc.load_state_from_sqlite(&db_path);
    assert!(reloaded_trunc.active_transcript_md.contains("Q1"));
    assert!(reloaded_trunc.active_transcript_md.contains("A1"));
    assert!(!reloaded_trunc.active_transcript_md.contains("Q2-edited"));

    // 3. Complete divergence (common_len = 0) creates fresh root chain on same thread
    reloaded_trunc.active_transcript_md =
        "# User: Fresh Q1\n\n## Agent (agy)\nFresh A1\n\n".to_string();
    reloaded_trunc.persist_state_to_sqlite(&db_path);

    let mut reloaded_fresh = App::new();
    reloaded_fresh.load_state_from_sqlite(&db_path);
    assert!(reloaded_fresh.active_transcript_md.contains("Fresh Q1"));
    assert!(reloaded_fresh.active_transcript_md.contains("Fresh A1"));
    assert!(!reloaded_fresh.active_transcript_md.contains("# User: Q1\n"));

    let _ = std::fs::remove_file(&db_path);
}

#[test]
fn test_sqlite_fallback_title_uses_first_user_content_or_active_session() {
    // 1. Unnamed active trunk with User node falls back to first_user_content
    let db_user = temp_path("title-user");
    let _ = std::fs::remove_file(&db_user);
    {
        let storage = SqliteStorage::open(&db_user).unwrap();
        let u1 = NodeId::now_v7();
        let a1 = NodeId::now_v7();
        storage
            .insert_node(&ConversationNode::new(
                u1,
                None::<NodeId>,
                AuthorRole::User,
                "First user prompt title".to_string(),
            ))
            .unwrap();
        storage
            .insert_node(&ConversationNode::new(
                a1,
                Some(u1),
                AuthorRole::Assistant,
                "Reply".to_string(),
            ))
            .unwrap();
        let tid = storage.create_thread(u1, AnchorId::now_v7()).unwrap();
        storage.update_active_leaf(tid, a1).unwrap();
        insert_blackboard_note(&storage, "active_thread_id", &tid.to_string());
    }
    let mut app_user = App::new();
    app_user.load_state_from_sqlite(&db_user);
    assert_eq!(app_user.threads.len(), 1);
    assert_eq!(app_user.threads[0].title, "First user prompt title");

    // Whitespace-only active_thread_title also falls back to first_user_content
    {
        let storage = SqliteStorage::open(&db_user).unwrap();
        insert_blackboard_note(&storage, "active_thread_title", "   \t  ");
    }
    let mut app_ws = App::new();
    app_ws.load_state_from_sqlite(&db_user);
    assert_eq!(app_ws.threads[0].title, "First user prompt title");
    let _ = std::fs::remove_file(&db_user);

    // 2. Unnamed active trunk with Assistant-only node falls back to "Active Session"
    let db_asst = temp_path("title-asst");
    let _ = std::fs::remove_file(&db_asst);
    {
        let storage = SqliteStorage::open(&db_asst).unwrap();
        let a1 = NodeId::now_v7();
        storage
            .insert_node(&ConversationNode::new(
                a1,
                None::<NodeId>,
                AuthorRole::Assistant,
                "Assistant only turn".to_string(),
            ))
            .unwrap();
        let tid = storage.create_thread(a1, AnchorId::now_v7()).unwrap();
        insert_blackboard_note(&storage, "active_thread_id", &tid.to_string());
    }
    let mut app_asst = App::new();
    app_asst.load_state_from_sqlite(&db_asst);
    assert_eq!(app_asst.threads.len(), 1);
    assert_eq!(app_asst.threads[0].title, "Active Session");

    // 3. Persisting with empty thread title writes "Active Session" when no title set
    let mut empty_title_app = App::new();
    empty_title_app.active_transcript_md = "## Agent (agy)\nOnly assistant\n\n".to_string();
    empty_title_app.threads.push(ThreadItem {
        id: ThreadId::now_v7().to_string(),
        title: "   ".to_string(),
        custom_name: None,
        anchors: Vec::new(),
    });
    let db_empty_title = temp_path("title-empty");
    let _ = std::fs::remove_file(&db_empty_title);
    empty_title_app.persist_state_to_sqlite(&db_empty_title);
    let mut reloaded_empty = App::new();
    reloaded_empty.load_state_from_sqlite(&db_empty_title);
    assert_eq!(reloaded_empty.threads[0].title, "Active Session");

    let _ = std::fs::remove_file(&db_asst);
    let _ = std::fs::remove_file(&db_empty_title);
}
