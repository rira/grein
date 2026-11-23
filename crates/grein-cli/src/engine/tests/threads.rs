use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use grein_core::model::{AnchorId, AuthorRole, ConversationNode, NodeId, ThreadId};
use grein_core::storage::sqlite::SqliteStorage;
use grein_tui::engine_port::TuiEngineEvent;
use grein_workspace::git_status::RepoGitStatus;

use super::super::types::parse_typed_id;
use super::super::{DaemonEngine, InProcessEngine};

#[tokio::test]
async fn test_thread_metadata_inference_branch_local_turns_and_offline_root() {
    // 1. Offline-root edge case: send_turn before ensure_main_thread.
    let engine = InProcessEngine::in_memory().expect("in_memory");
    engine
        .send_turn("offline root question")
        .await
        .expect("offline send_turn");
    let main = engine.ensure_main_thread().await.expect("ensure main");

    let main_turns = engine
        .get_thread_transcript(&main.thread_id)
        .await
        .expect("main transcript");
    assert_eq!(main_turns.len(), 1);
    assert_eq!(main_turns[0].content, "offline root question");

    let meta_initial = engine.thread_metadata().await.expect("meta");
    assert_eq!(meta_initial.len(), 1);
    assert_eq!(meta_initial[0].parent_thread_id, None);
    assert_eq!(meta_initial[0].inherited_turn_count, 0);

    let offline_fork = engine
        .fork_thread_with_title(Some(&main.thread_id), Some("offline-fork"))
        .await
        .expect("fork from offline main");
    let fork_turns = engine
        .get_thread_transcript(&offline_fork.thread_id)
        .await
        .expect("offline fork turns");
    assert!(
        fork_turns.is_empty(),
        "forked thread must not duplicate offline root node"
    );
    let meta_with_fork = engine.thread_metadata().await.expect("meta");
    let fork_meta = meta_with_fork
        .iter()
        .find(|m| m.thread_id == offline_fork.thread_id)
        .expect("fork meta");
    assert_eq!(
        fork_meta.parent_thread_id.as_deref(),
        Some(main.thread_id.as_str())
    );
    assert_eq!(fork_meta.inherited_turn_count, 1);

    // 2. Multi-level tree + intermediate parent deletion.
    let engine2 = InProcessEngine::in_memory().expect("in_memory 2");
    let root = engine2.ensure_main_thread().await.expect("main");
    let root_tid: ThreadId = parse_typed_id(&root.thread_id).unwrap();
    let root_leaf = engine2
        .runtime
        .storage
        .get_thread(root_tid)
        .unwrap()
        .unwrap()
        .active_leaf_id;
    let u1 = NodeId::new();
    engine2
        .runtime
        .storage
        .insert_node(&ConversationNode::new(
            u1,
            Some(root_leaf),
            AuthorRole::User,
            "u1".into(),
        ))
        .unwrap();
    let a1 = NodeId::new();
    engine2
        .runtime
        .storage
        .insert_node(&ConversationNode::new(
            a1,
            Some(u1),
            AuthorRole::Assistant,
            "a1".into(),
        ))
        .unwrap();
    engine2
        .runtime
        .storage
        .update_thread_leaf(root_tid, a1)
        .unwrap();

    let child = engine2
        .fork_thread_with_title(Some(&root.thread_id), Some("child"))
        .await
        .expect("fork child");
    let child_tid: ThreadId = parse_typed_id(&child.thread_id).unwrap();
    let cu1 = NodeId::new();
    engine2
        .runtime
        .storage
        .insert_node(&ConversationNode::new(
            cu1,
            Some(a1),
            AuthorRole::User,
            "cu1".into(),
        ))
        .unwrap();
    let ca1 = NodeId::new();
    engine2
        .runtime
        .storage
        .insert_node(&ConversationNode::new(
            ca1,
            Some(cu1),
            AuthorRole::Assistant,
            "ca1".into(),
        ))
        .unwrap();
    engine2
        .runtime
        .storage
        .update_thread_leaf(child_tid, ca1)
        .unwrap();

    let child_turns = engine2
        .get_thread_transcript(&child.thread_id)
        .await
        .expect("child turns");
    assert_eq!(child_turns.len(), 2);
    assert_eq!(child_turns[0].content, "cu1");
    assert_eq!(child_turns[1].content, "ca1");

    let grandchild = engine2
        .fork_thread_with_title(Some(&child.thread_id), Some("grandchild"))
        .await
        .expect("fork grandchild");
    let gc_tid: ThreadId = parse_typed_id(&grandchild.thread_id).unwrap();
    let gu1 = NodeId::new();
    engine2
        .runtime
        .storage
        .insert_node(&ConversationNode::new(
            gu1,
            Some(ca1),
            AuthorRole::User,
            "gu1".into(),
        ))
        .unwrap();
    engine2
        .runtime
        .storage
        .update_thread_leaf(gc_tid, gu1)
        .unwrap();

    let gc_turns = engine2
        .get_thread_transcript(&grandchild.thread_id)
        .await
        .expect("gc turns");
    assert_eq!(gc_turns.len(), 1);
    assert_eq!(gc_turns[0].content, "gu1");

    let meta_3 = engine2.thread_metadata().await.expect("meta 3");
    let gc_meta_before = meta_3
        .iter()
        .find(|m| m.thread_id == grandchild.thread_id)
        .unwrap();
    assert_eq!(
        gc_meta_before.parent_thread_id.as_deref(),
        Some(child.thread_id.as_str())
    );
    assert_eq!(gc_meta_before.inherited_turn_count, 4);

    // Delete intermediate parent `child` -> `grandchild` walks up to `root`.
    let next_active = engine2
        .delete_thread(&child.thread_id)
        .await
        .expect("delete child");
    assert_eq!(next_active, root.thread_id);

    let meta_after = engine2.thread_metadata().await.expect("meta after");
    assert_eq!(meta_after.len(), 2);
    let gc_meta_after = meta_after
        .iter()
        .find(|m| m.thread_id == grandchild.thread_id)
        .unwrap();
    assert_eq!(
        gc_meta_after.parent_thread_id.as_deref(),
        Some(root.thread_id.as_str())
    );
    assert_eq!(gc_meta_after.inherited_turn_count, 4);
}

#[tokio::test]
async fn test_empty_branch_fork_lineage_multi_root_rename_and_delete_across_reopen() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("lineage.db");

    let engine =
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
            .expect("open");
    let main = engine.ensure_main_thread().await.expect("main");

    // Rename Main sets custom_name while keeping title untouched; blank clears back to None.
    let cleared_main = engine
        .rename_thread(&main.thread_id, "   ")
        .await
        .expect("blank rename clears custom_name");
    assert_eq!(cleared_main.custom_name, None);
    assert_eq!(cleared_main.title, "Main");

    let renamed_main = engine
        .rename_thread(&main.thread_id, "Primary Root")
        .await
        .expect("rename main");
    assert_eq!(renamed_main.custom_name.as_deref(), Some("Primary Root"));
    assert_eq!(renamed_main.title, "Main");

    // 3-level empty-branch fork: Primary Root -> Branch 5 (0 turns) -> Branch 6 (0 turns).
    let b5 = engine
        .fork_thread_with_title(Some(&main.thread_id), Some("Branch 5"))
        .await
        .expect("fork b5");
    let b6 = engine
        .fork_thread_with_title(Some(&b5.thread_id), Some("Branch 6"))
        .await
        .expect("fork b6");

    // Create second root and rename it.
    let root2 = engine.create_root_thread(None).await.expect("create root2");
    assert_eq!(root2.title, "Root 2");
    assert_eq!(root2.custom_name, None);
    let renamed_root2 = engine
        .rename_thread(&root2.thread_id, "Secondary Root")
        .await
        .expect("rename root2");
    assert_eq!(
        renamed_root2.custom_name.as_deref(),
        Some("Secondary Root")
    );
    assert_eq!(renamed_root2.title, "Root 2");
    let root2_child = engine
        .fork_thread_with_title(Some(&root2.thread_id), Some("Root2 Child"))
        .await
        .expect("fork root2 child");

    drop(engine);
    let reopened =
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
            .expect("reopen");
    let ensured = reopened.ensure_main_thread().await.expect("ensure_main_thread");
    assert_eq!(ensured.custom_name.as_deref(), Some("Primary Root"));
    assert_eq!(ensured.title, "Main");

    let threads = reopened.list_threads().await.expect("list");
    let reloaded_root2 = threads
        .iter()
        .find(|t| t.thread_id == root2.thread_id)
        .unwrap();
    assert_eq!(
        reloaded_root2.custom_name.as_deref(),
        Some("Secondary Root")
    );
    assert_eq!(reloaded_root2.title, "Root 2");

    let meta = reopened.thread_metadata().await.expect("meta");
    let m_main = meta.iter().find(|m| m.thread_id == main.thread_id).unwrap();
    let m_b5 = meta.iter().find(|m| m.thread_id == b5.thread_id).unwrap();
    let m_b6 = meta.iter().find(|m| m.thread_id == b6.thread_id).unwrap();
    let m_root2 = meta.iter().find(|m| m.thread_id == root2.thread_id).unwrap();
    let m_r2c = meta
        .iter()
        .find(|m| m.thread_id == root2_child.thread_id)
        .unwrap();

    assert_eq!(m_main.parent_thread_id, None);
    assert_eq!(m_b5.parent_thread_id.as_deref(), Some(main.thread_id.as_str()));
    assert_eq!(m_b6.parent_thread_id.as_deref(), Some(b5.thread_id.as_str()));
    assert_eq!(m_root2.parent_thread_id, None);
    assert_eq!(m_root2.inherited_turn_count, 0);
    assert_eq!(
        m_r2c.parent_thread_id.as_deref(),
        Some(root2.thread_id.as_str())
    );

    // Deleting intermediate parent b5 reparents b6 to main.
    reopened.delete_thread(&b5.thread_id).await.expect("delete b5");
    let meta_after_mid = reopened.thread_metadata().await.expect("meta mid");
    let m_b6_after = meta_after_mid
        .iter()
        .find(|m| m.thread_id == b6.thread_id)
        .unwrap();
    assert_eq!(
        m_b6_after.parent_thread_id.as_deref(),
        Some(main.thread_id.as_str())
    );

    // Deleting root2 promotes root2_child to root cleanly.
    reopened
        .delete_thread(&root2.thread_id)
        .await
        .expect("delete root2");
    let meta_after_root2 = reopened.thread_metadata().await.expect("meta root2");
    let m_r2c_after = meta_after_root2
        .iter()
        .find(|m| m.thread_id == root2_child.thread_id)
        .unwrap();
    assert_eq!(m_r2c_after.parent_thread_id, None);

    // Deleting Primary Root does NOT recreate Main and promotes b6 to root.
    reopened
        .delete_thread(&main.thread_id)
        .await
        .expect("delete main");
    let remaining = reopened.list_threads().await.expect("remaining");
    assert_eq!(remaining.len(), 2);
    assert!(!remaining.iter().any(|t| t.title == "Main" || t.title == "Primary Root"));
    let meta_final = reopened.thread_metadata().await.expect("meta final");
    assert!(meta_final.iter().all(|m| m.parent_thread_id.is_none()));
}

#[tokio::test]
async fn test_legacy_null_parent_thread_id_infers_nesting_under_main_and_other_roots() {
    let storage = SqliteStorage::in_memory().expect("sqlite");
    let r1 = ConversationNode::new_root(AuthorRole::System, "");
    storage.insert_node(&r1).unwrap();
    let main_tid = storage
        .create_thread_with_title(r1.id, AnchorId::new(), "Main")
        .unwrap();
    let u1 = ConversationNode::new(NodeId::new(), Some(r1.id), AuthorRole::User, "hi".into());
    storage.insert_node(&u1).unwrap();
    storage.update_thread_leaf(main_tid, u1.id).unwrap();

    let main_child_tid = storage
        .create_thread_with_title(u1.id, AnchorId::new(), "Legacy Main Child")
        .unwrap();

    // Second root at idx > 0 with 0 turns and a legacy child with parent_thread_id = None.
    let r2 = ConversationNode::new_root(AuthorRole::System, "");
    storage.insert_node(&r2).unwrap();
    let root2_tid = storage
        .create_thread_with_title(r2.id, AnchorId::new(), "Root 2")
        .unwrap();
    let r2_child_tid = storage
        .create_thread_with_title(r2.id, AnchorId::new(), "Legacy Root2 Child")
        .unwrap();

    let engine = InProcessEngine::from_storage(storage).expect("engine");
    let meta = engine.thread_metadata().await.expect("meta");
    assert_eq!(meta.len(), 4);

    let m_main = meta
        .iter()
        .find(|m| m.thread_id == main_tid.0.to_string())
        .unwrap();
    let m_mc = meta
        .iter()
        .find(|m| m.thread_id == main_child_tid.0.to_string())
        .unwrap();
    let m_r2 = meta
        .iter()
        .find(|m| m.thread_id == root2_tid.0.to_string())
        .unwrap();
    let m_r2c = meta
        .iter()
        .find(|m| m.thread_id == r2_child_tid.0.to_string())
        .unwrap();

    assert_eq!(m_main.parent_thread_id, None);
    assert_eq!(
        m_mc.parent_thread_id.as_deref(),
        Some(main_tid.0.to_string().as_str())
    );
    assert_eq!(m_r2.parent_thread_id, None);
    assert_eq!(
        m_r2c.parent_thread_id.as_deref(),
        Some(root2_tid.0.to_string().as_str())
    );
}

#[tokio::test]
async fn test_compute_thread_metadata_deduplicates_git_status_per_unique_path() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root_dir = temp.path().join("root_ws");
    let sub_dir = temp.path().join("sub_ws");
    std::fs::create_dir_all(&root_dir).expect("mkdir root");
    std::fs::create_dir_all(&sub_dir).expect("mkdir sub");
    let canonical_root = std::fs::canonicalize(&root_dir).expect("canonical root");
    let canonical_sub = std::fs::canonicalize(&sub_dir).expect("canonical sub");
    let db_path = temp.path().join("dedup_git.db");

    let engine = InProcessEngine::open_with_config(&db_path, canonical_root.clone(), None)
        .expect("open");
    let main = engine.ensure_main_thread().await.expect("main");
    let _branch1 = engine
        .fork_thread_with_title(Some(&main.thread_id), Some("b1"))
        .await
        .expect("b1");
    let _branch2 = engine
        .fork_thread_with_title(Some(&main.thread_id), Some("b2"))
        .await
        .expect("b2");
    let term1 = engine
        .create_terminal(
            Some(&main.thread_id),
            Some("t1"),
            Some(&canonical_sub.to_string_lossy()),
            24,
            80,
        )
        .await
        .expect("t1");
    let term2 = engine
        .create_terminal(
            Some(&main.thread_id),
            Some("t2"),
            Some(&canonical_sub.to_string_lossy()),
            24,
            80,
        )
        .await
        .expect("t2");

    let total_calls = AtomicUsize::new(0);
    let calls_per_path: std::sync::Mutex<HashMap<PathBuf, usize>> =
        std::sync::Mutex::new(HashMap::new());

    let meta = engine
        .runtime
        .compute_thread_metadata_with_inspector(|path| {
            total_calls.fetch_add(1, Ordering::SeqCst);
            *calls_per_path
                .lock()
                .unwrap()
                .entry(path.to_path_buf())
                .or_insert(0) += 1;
            if path == canonical_root.as_path() {
                Some(RepoGitStatus {
                    branch: Some("main".to_string()),
                    dirty_count: 2,
                    is_worktree: false,
                })
            } else if path == canonical_sub.as_path() {
                Some(RepoGitStatus {
                    branch: Some("feat/sub".to_string()),
                    dirty_count: 5,
                    is_worktree: true,
                })
            } else {
                None
            }
        })
        .expect("compute_thread_metadata_with_inspector");

    assert_eq!(meta.len(), 5);
    assert_eq!(
        total_calls.load(Ordering::SeqCst),
        2,
        "inspect_git_status must be called once per unique path across 5 threads"
    );
    let counts = calls_per_path.lock().unwrap();
    assert_eq!(counts.get(&canonical_root).copied(), Some(1));
    assert_eq!(counts.get(&canonical_sub).copied(), Some(1));

    let main_meta = meta.iter().find(|m| m.thread_id == main.thread_id).unwrap();
    assert_eq!(main_meta.git_branch.as_deref(), Some("main"));
    assert_eq!(main_meta.git_dirty, 2);

    let t1_meta = meta.iter().find(|m| m.thread_id == term1.thread_id).unwrap();
    let t2_meta = meta.iter().find(|m| m.thread_id == term2.thread_id).unwrap();
    assert_eq!(t1_meta.git_branch.as_deref(), Some("feat/sub"));
    assert_eq!(t1_meta.git_dirty, 5);
    assert_eq!(t2_meta.git_branch.as_deref(), Some("feat/sub"));
    assert_eq!(t2_meta.git_dirty, 5);
}

#[tokio::test]
async fn test_rename_thread_sets_and_clears_custom_name_without_touching_title() {
    let engine = InProcessEngine::in_memory().expect("in_memory");
    let main = engine.ensure_main_thread().await.expect("main");
    assert_eq!(main.title, "Main");
    assert_eq!(main.custom_name, None);

    let term = engine
        .create_terminal(Some(&main.thread_id), None, None, 24, 80)
        .await
        .expect("create_terminal");
    assert_eq!(term.title, "Terminal");
    assert_eq!(term.custom_name, None);

    let mut rx = engine.subscribe_tui_events();

    let renamed_main = engine
        .rename_thread(&main.thread_id, "  Terminal  ")
        .await
        .expect("rename main");
    assert_eq!(renamed_main.custom_name.as_deref(), Some("Terminal"));
    assert_eq!(renamed_main.title, "Main");

    let renamed_term = engine
        .rename_thread(&term.thread_id, "Dev Shell")
        .await
        .expect("rename term");
    assert_eq!(renamed_term.custom_name.as_deref(), Some("Dev Shell"));
    assert_eq!(renamed_term.title, "Terminal");

    let meta_after_set = engine.thread_metadata().await.expect("meta after set");
    assert_eq!(
        meta_after_set
            .iter()
            .find(|m| m.thread_id == main.thread_id)
            .unwrap()
            .custom_name
            .as_deref(),
        Some("Terminal")
    );
    assert_eq!(
        meta_after_set
            .iter()
            .find(|m| m.thread_id == term.thread_id)
            .unwrap()
            .custom_name
            .as_deref(),
        Some("Dev Shell")
    );

    let cleared_main = engine
        .rename_thread(&main.thread_id, "")
        .await
        .expect("clear main custom_name");
    assert_eq!(cleared_main.custom_name, None);
    assert_eq!(cleared_main.title, "Main");

    let cleared_term = engine
        .rename_thread(&term.thread_id, "   ")
        .await
        .expect("clear term custom_name");
    assert_eq!(cleared_term.custom_name, None);
    assert_eq!(cleared_term.title, "Terminal");

    let meta_after_clear = engine.thread_metadata().await.expect("meta after clear");
    assert_eq!(
        meta_after_clear
            .iter()
            .find(|m| m.thread_id == main.thread_id)
            .unwrap()
            .custom_name,
        None
    );
    assert_eq!(
        meta_after_clear
            .iter()
            .find(|m| m.thread_id == term.thread_id)
            .unwrap()
            .custom_name,
        None
    );

    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    assert!(!events
        .iter()
        .any(|e| matches!(e, TuiEngineEvent::ThreadTitleUpdated { .. })));
    assert!(events.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ThreadCustomNameUpdated { thread_id, custom_name }
            if thread_id == &main.thread_id && custom_name.as_deref() == Some("Terminal")
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ThreadCustomNameUpdated { thread_id, custom_name }
            if thread_id == &term.thread_id && custom_name.as_deref() == Some("Dev Shell")
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ThreadCustomNameUpdated { thread_id, custom_name }
            if thread_id == &main.thread_id && custom_name.is_none()
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ThreadCustomNameUpdated { thread_id, custom_name }
            if thread_id == &term.thread_id && custom_name.is_none()
    )));
}
