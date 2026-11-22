use grein_core::model::{
    AnchorId, AuthorRole, ConversationNode, NodeId, Thread, ThreadLifecycleState,
};
use grein_core::storage::sqlite::SqliteDagStorage;

#[test]
fn test_sqlite_storage_initialization_runs_migrations() {
    let storage = SqliteDagStorage::in_memory();
    assert!(
        storage.is_ok(),
        "In-memory SQLite storage should initialize successfully"
    );
}

#[test]
fn test_insert_node_and_get_trunk_path_linear() -> Result<(), Box<dyn std::error::Error>> {
    let storage = SqliteDagStorage::in_memory()?;

    let root_id = NodeId::new_v7();
    let root_node = ConversationNode {
        id: root_id,
        parent_id: None,
        role: AuthorRole::System,
        content: "System instructions".to_string(),
    };
    storage.insert_node(&root_node)?;

    let user_id = NodeId::new_v7();
    let user_node = ConversationNode {
        id: user_id,
        parent_id: Some(root_id),
        role: AuthorRole::User,
        content: "Hello, world!".to_string(),
    };
    storage.insert_node(&user_node)?;

    let assistant_id = NodeId::new_v7();
    let assistant_node = ConversationNode {
        id: assistant_id,
        parent_id: Some(user_id),
        role: AuthorRole::Assistant,
        content: "Hello! How can I help you?".to_string(),
    };
    storage.insert_node(&assistant_node)?;

    let trunk_path = storage.get_trunk_path(assistant_id)?;

    assert_eq!(trunk_path.len(), 3);
    assert_eq!(trunk_path[0].id, root_id);
    assert_eq!(trunk_path[0].role, AuthorRole::System);
    assert_eq!(trunk_path[0].parent_id, None);
    assert_eq!(trunk_path[1].id, user_id);
    assert_eq!(trunk_path[1].role, AuthorRole::User);
    assert_eq!(trunk_path[1].parent_id, Some(root_id));
    assert_eq!(trunk_path[2].id, assistant_id);
    assert_eq!(trunk_path[2].role, AuthorRole::Assistant);
    assert_eq!(trunk_path[2].parent_id, Some(user_id));

    Ok(())
}

#[test]
fn test_branching_dag_isolation() -> Result<(), Box<dyn std::error::Error>> {
    let storage = SqliteDagStorage::in_memory()?;

    let root = ConversationNode {
        id: NodeId::new_v7(),
        parent_id: None,
        role: AuthorRole::System,
        content: "Root".to_string(),
    };
    storage.insert_node(&root)?;

    let node_a = ConversationNode {
        id: NodeId::new_v7(),
        parent_id: Some(root.id),
        role: AuthorRole::User,
        content: "Prompt A".to_string(),
    };
    storage.insert_node(&node_a)?;

    let node_b1 = ConversationNode {
        id: NodeId::new_v7(),
        parent_id: Some(node_a.id),
        role: AuthorRole::Assistant,
        content: "Branch 1 Response".to_string(),
    };
    storage.insert_node(&node_b1)?;

    let node_b2 = ConversationNode {
        id: NodeId::new_v7(),
        parent_id: Some(node_b1.id),
        role: AuthorRole::User,
        content: "Branch 1 Follow-up".to_string(),
    };
    storage.insert_node(&node_b2)?;

    let node_c1 = ConversationNode {
        id: NodeId::new_v7(),
        parent_id: Some(node_a.id),
        role: AuthorRole::Assistant,
        content: "Branch 2 Alternative Response".to_string(),
    };
    storage.insert_node(&node_c1)?;

    let path_b = storage.get_trunk_path(node_b2.id)?;
    assert_eq!(path_b.len(), 4);
    assert_eq!(
        path_b.iter().map(|n| n.id).collect::<Vec<_>>(),
        vec![root.id, node_a.id, node_b1.id, node_b2.id]
    );

    let path_c = storage.get_trunk_path(node_c1.id)?;
    assert_eq!(path_c.len(), 3);
    assert_eq!(
        path_c.iter().map(|n| n.id).collect::<Vec<_>>(),
        vec![root.id, node_a.id, node_c1.id]
    );

    Ok(())
}

#[test]
fn test_create_thread_and_leaf_pointer() -> Result<(), Box<dyn std::error::Error>> {
    let storage = SqliteDagStorage::in_memory()?;

    let root_node = ConversationNode {
        id: NodeId::new_v7(),
        parent_id: None,
        role: AuthorRole::User,
        content: "Initial prompt".to_string(),
    };
    storage.insert_node(&root_node)?;

    let anchor_id = AnchorId::new();
    let thread_id = storage.create_thread(root_node.id, anchor_id)?;

    let active_leaf = storage.get_thread_leaf(thread_id)?;
    assert_eq!(active_leaf, root_node.id);

    Ok(())
}

#[test]
fn test_thread_lifecycle_states_persisted_transactionally() -> Result<(), Box<dyn std::error::Error>>
{
    let storage = SqliteDagStorage::in_memory()?;

    let root_node = ConversationNode {
        id: NodeId::new_v7(),
        parent_id: None,
        role: AuthorRole::User,
        content: "Task start".to_string(),
    };
    storage.insert_node(&root_node)?;

    let thread_id = storage.create_thread(root_node.id, AnchorId::new())?;

    let states = [
        ThreadLifecycleState::Streaming,
        ThreadLifecycleState::Executing,
        ThreadLifecycleState::PermissionRequired,
        ThreadLifecycleState::AwaitingInput,
        ThreadLifecycleState::Idle,
        ThreadLifecycleState::Error,
    ];

    for state in states {
        storage.update_thread_state(thread_id, state)?;
        let current_state = storage.get_thread_state(thread_id)?;
        assert_eq!(current_state, state);
    }

    Ok(())
}

#[test]
fn test_tool_result_author_role_persistence() -> Result<(), Box<dyn std::error::Error>> {
    let storage = SqliteDagStorage::in_memory()?;

    let tool_node = ConversationNode {
        id: NodeId::new_v7(),
        parent_id: None,
        role: AuthorRole::ToolResult,
        content: "{\"status\": \"ok\"}".to_string(),
    };
    storage.insert_node(&tool_node)?;

    let path = storage.get_trunk_path(tool_node.id)?;
    assert_eq!(path.len(), 1);
    assert_eq!(path[0].role, AuthorRole::ToolResult);
    assert_eq!(path[0].content, "{\"status\": \"ok\"}");

    Ok(())
}

#[test]
fn test_get_thread_returns_head_leaf_and_state() -> Result<(), Box<dyn std::error::Error>> {
    let storage = SqliteDagStorage::in_memory()?;

    let root_node = ConversationNode {
        id: NodeId::new_v7(),
        parent_id: None,
        role: AuthorRole::User,
        content: "Thread root".to_string(),
    };
    storage.insert_node(&root_node)?;

    let anchor_id = AnchorId::new();
    let thread_id = storage.create_thread(root_node.id, anchor_id)?;

    let thread_opt: Option<Thread> = storage.get_thread(thread_id)?;
    assert!(thread_opt.is_some(), "Thread should be found");

    let thread = thread_opt.unwrap();
    assert_eq!(thread.id, thread_id);
    assert_eq!(thread.anchor_id, anchor_id);
    assert_eq!(thread.head_node_id, root_node.id);
    assert_eq!(thread.leaf_node_id, root_node.id);
    assert_eq!(thread.title, "");
    assert_eq!(thread.acp_session_id, None);
    assert!(!thread.merged);

    Ok(())
}

#[test]
fn test_author_role_as_str_and_display() {
    assert_eq!(AuthorRole::User.as_str(), "user");
    assert_eq!(AuthorRole::Assistant.as_str(), "assistant");
    assert_eq!(AuthorRole::System.as_str(), "system");
    assert_eq!(AuthorRole::ToolResult.as_str(), "tool_result");

    assert_eq!(format!("{}", AuthorRole::User), "user");
    assert_eq!(format!("{}", AuthorRole::Assistant), "assistant");
    assert_eq!(format!("{}", AuthorRole::System), "system");
    assert_eq!(format!("{}", AuthorRole::ToolResult), "tool_result");
}

#[test]
fn test_thread_new_defaults_and_with_title() {
    let tid = grein_core::model::ThreadId::new_v7();
    let parent = NodeId::new_v7();
    let anchor = AnchorId::new_v7();
    let leaf = NodeId::new_v7();

    let thread = Thread::new(tid, parent, anchor, leaf, ThreadLifecycleState::Idle);
    assert_eq!(thread.title, "");
    assert_eq!(thread.acp_session_id, None);
    assert!(!thread.merged);

    let titled = thread.with_title("Investigate auth");
    assert_eq!(titled.title, "Investigate auth");
    assert_eq!(titled.acp_session_id, None);
    assert!(!titled.merged);
}

#[test]
fn test_legacy_threads_schema_migration_adds_metadata_columns_idempotently(
) -> Result<(), Box<dyn std::error::Error>> {
    let conn = rusqlite::Connection::open_in_memory()?;
    conn.execute_batch(
        "CREATE TABLE nodes (
            id TEXT PRIMARY KEY NOT NULL,
            parent_id TEXT,
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            FOREIGN KEY (parent_id) REFERENCES nodes(id) ON DELETE RESTRICT
        );
        CREATE TABLE threads (
            id TEXT PRIMARY KEY NOT NULL,
            parent_node_id TEXT NOT NULL,
            anchor_id TEXT NOT NULL,
            active_leaf_id TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            FOREIGN KEY (parent_node_id) REFERENCES nodes(id) ON DELETE RESTRICT,
            FOREIGN KEY (active_leaf_id) REFERENCES nodes(id) ON DELETE RESTRICT
        );",
    )?;

    let root_id = NodeId::new_v7();
    let thread_id = grein_core::model::ThreadId::new_v7();
    let anchor_id = AnchorId::new_v7();
    conn.execute(
        "INSERT INTO nodes (id, parent_id, role, content, created_at) VALUES (?1, NULL, 'user', 'Legacy root', 1)",
        rusqlite::params![root_id.to_string()],
    )?;
    conn.execute(
        "INSERT INTO threads (id, parent_node_id, anchor_id, active_leaf_id, status, created_at)
         VALUES (?1, ?2, ?3, ?2, 'idle', 2)",
        rusqlite::params![thread_id.to_string(), root_id.to_string(), anchor_id.to_string()],
    )?;

    let storage = SqliteDagStorage::new(conn)?;
    // Re-running migrations must remain idempotent.
    storage.run_migrations()?;

    let fetched = storage
        .get_thread(thread_id)?
        .expect("Legacy thread must still exist after migration");
    assert_eq!(fetched.id, thread_id);
    assert_eq!(fetched.title, "");
    assert_eq!(fetched.acp_session_id, None);
    assert!(!fetched.merged);
    assert_eq!(fetched.parent_thread_id, None);

    let all = storage.list_threads()?;
    assert_eq!(all.len(), 1);
    assert_eq!(all[0], fetched);

    Ok(())
}

#[test]
fn test_thread_metadata_crud_operations() -> Result<(), Box<dyn std::error::Error>> {
    let storage = SqliteDagStorage::in_memory()?;

    let root_node = ConversationNode::new(
        NodeId::new_v7(),
        None::<NodeId>,
        AuthorRole::User,
        "Root prompt".to_string(),
    );
    storage.insert_node(&root_node)?;

    let anchor_id = AnchorId::new_v7();
    let thread_id = storage.create_thread_with_title(root_node.id, anchor_id, "Initial Title")?;

    let created = storage.get_thread(thread_id)?.expect("Thread must exist");
    assert_eq!(created.title, "Initial Title");
    assert_eq!(created.acp_session_id, None);
    assert!(!created.merged);

    storage.update_thread_title(&thread_id, "Renamed Branch")?;
    storage.update_thread_acp_session_id(&thread_id, Some("acp-sess-123"))?;
    storage.mark_thread_merged(&thread_id, true)?;

    let updated = storage.get_thread(thread_id)?.expect("Thread must exist");
    assert_eq!(updated.title, "Renamed Branch");
    assert_eq!(updated.acp_session_id.as_deref(), Some("acp-sess-123"));
    assert!(updated.merged);

    let listed = storage.list_threads()?;
    assert_eq!(listed, vec![updated]);

    storage.update_thread_acp_session_id(&thread_id, None)?;
    storage.mark_thread_merged(&thread_id, false)?;
    let cleared = storage.get_thread(thread_id)?.expect("Thread must exist");
    assert_eq!(cleared.acp_session_id, None);
    assert!(!cleared.merged);

    let missing_id = grein_core::model::ThreadId::new_v7();
    assert!(storage.update_thread_title(&missing_id, "x").is_err());
    assert!(storage
        .update_thread_acp_session_id(&missing_id, Some("s"))
        .is_err());
    assert!(storage.mark_thread_merged(&missing_id, true).is_err());

    Ok(())
}

#[test]
fn test_list_anchors_returns_inserted_anchors() -> Result<(), Box<dyn std::error::Error>> {
    let storage = SqliteDagStorage::in_memory()?;

    assert!(storage.list_anchors()?.is_empty());

    let id1 = AnchorId::new_v7();
    let a1 = grein_core::model::Anchor {
        level: 1,
        title: "Architecture".to_string(),
        slug: "architecture".to_string(),
        line_number: 1,
    };
    let id2 = AnchorId::new_v7();
    let a2 = grein_core::model::Anchor {
        level: 2,
        title: "Storage Layer".to_string(),
        slug: "storage-layer".to_string(),
        line_number: 12,
    };

    storage.insert_anchor(&id1.0, &a1)?;
    storage.insert_anchor(&id2.0, &a2)?;

    let anchors = storage.list_anchors()?;
    assert_eq!(anchors, vec![(id1, a1), (id2, a2)]);

    Ok(())
}

#[test]
fn test_create_thread_with_parent_and_reparent_child_threads(
) -> Result<(), Box<dyn std::error::Error>> {
    let storage = SqliteDagStorage::in_memory()?;

    let root_node = ConversationNode::new_root(AuthorRole::System, "");
    assert_eq!(root_node.parent_id, None);
    assert_eq!(root_node.role, AuthorRole::System);
    assert_eq!(root_node.content, "");
    storage.insert_node(&root_node)?;

    let root_tid = storage.create_thread_with_parent(
        root_node.id,
        AnchorId::new_v7(),
        Some("Main"),
        None,
    )?;
    let mid_tid = storage.create_thread_with_parent(
        root_node.id,
        AnchorId::new_v7(),
        Some("Mid"),
        Some(root_tid),
    )?;
    let leaf_tid = storage.create_thread_with_parent(
        root_node.id,
        AnchorId::new_v7(),
        Some("Leaf"),
        Some(mid_tid),
    )?;

    assert_eq!(
        storage.get_thread(root_tid)?.unwrap().parent_thread_id,
        None
    );
    assert_eq!(
        storage.get_thread(mid_tid)?.unwrap().parent_thread_id,
        Some(root_tid)
    );
    assert_eq!(
        storage.get_thread(leaf_tid)?.unwrap().parent_thread_id,
        Some(mid_tid)
    );

    let reparented_to_grandparent = storage.reparent_child_threads(&mid_tid, Some(&root_tid))?;
    assert_eq!(reparented_to_grandparent, 1);
    assert_eq!(
        storage.get_thread(leaf_tid)?.unwrap().parent_thread_id,
        Some(root_tid)
    );

    let promoted_to_root = storage.reparent_child_threads(&root_tid, None)?;
    assert_eq!(promoted_to_root, 2);
    assert_eq!(storage.get_thread(mid_tid)?.unwrap().parent_thread_id, None);
    assert_eq!(
        storage.get_thread(leaf_tid)?.unwrap().parent_thread_id,
        None
    );

    Ok(())
}
