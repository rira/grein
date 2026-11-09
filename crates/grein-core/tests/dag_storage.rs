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

    Ok(())
}
