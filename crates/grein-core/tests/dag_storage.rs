use grein_core::model::{
    AnchorId, AuthorRole, ConversationNode, NodeId, Thread, ThreadId, ThreadLifecycleState,
};
use grein_core::storage::sqlite::SqliteDagStorage;
use grein_core::{CoreError, ThreadKind};

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
    assert_eq!(thread.kind, ThreadKind::Agent);
    assert_eq!(thread.cwd, None);
    assert_eq!(thread.custom_name, None);
    assert_eq!(thread.agent_provider, None);
    assert_eq!(thread.agent_provider(), None);

    let titled = thread.with_title("Investigate auth");
    assert_eq!(titled.title, "Investigate auth");
    assert_eq!(titled.acp_session_id, None);
    assert!(!titled.merged);
    assert_eq!(titled.kind, ThreadKind::Agent);
    assert_eq!(titled.cwd, None);
    assert_eq!(titled.custom_name, None);
    assert_eq!(titled.agent_provider, None);
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
    assert_eq!(fetched.kind, ThreadKind::Agent);
    assert_eq!(fetched.cwd, None);
    assert_eq!(fetched.custom_name, None);
    assert_eq!(fetched.agent_provider, None);

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

#[test]
fn test_legacy_threads_migration_adds_kind_and_cwd_idempotently(
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
            title TEXT NOT NULL DEFAULT '',
            acp_session_id TEXT,
            merged INTEGER NOT NULL DEFAULT 0,
            parent_thread_id TEXT,
            FOREIGN KEY (parent_node_id) REFERENCES nodes(id) ON DELETE RESTRICT,
            FOREIGN KEY (active_leaf_id) REFERENCES nodes(id) ON DELETE RESTRICT
        );",
    )?;

    let root_id = NodeId::new_v7();
    let thread_id = ThreadId::new_v7();
    let anchor_id = AnchorId::new_v7();
    conn.execute(
        "INSERT INTO nodes (id, parent_id, role, content, created_at) VALUES (?1, NULL, 'user', 'Legacy root', 1)",
        rusqlite::params![root_id.to_string()],
    )?;
    conn.execute(
        "INSERT INTO threads (id, parent_node_id, anchor_id, active_leaf_id, status, created_at, title)
         VALUES (?1, ?2, ?3, ?2, 'idle', 2, 'Pre-terminal thread')",
        rusqlite::params![thread_id.to_string(), root_id.to_string(), anchor_id.to_string()],
    )?;

    let storage = SqliteDagStorage::new(conn)?;
    storage.run_migrations()?;

    let fetched = storage
        .get_thread(thread_id)?
        .expect("Legacy thread must still exist after migration");
    assert_eq!(fetched.id, thread_id);
    assert_eq!(fetched.title, "Pre-terminal thread");
    assert_eq!(fetched.kind, ThreadKind::Agent);
    assert_eq!(fetched.cwd, None);
    assert_eq!(fetched.custom_name, None);
    assert_eq!(fetched.agent_provider, None);

    let listed = storage.list_threads()?;
    assert_eq!(listed, vec![fetched]);

    Ok(())
}

#[test]
fn test_terminal_thread_kind_and_cwd_roundtrip_and_fk_invariants(
) -> Result<(), Box<dyn std::error::Error>> {
    let storage = SqliteDagStorage::in_memory()?;

    let root_node = ConversationNode::new_root(AuthorRole::System, "");
    storage.insert_node(&root_node)?;

    let agent_id = storage.create_thread_with_kind(
        root_node.id,
        AnchorId::new_v7(),
        Some("Agent"),
        None,
        ThreadKind::Agent,
        None,
    )?;

    let term_id = storage.create_thread_with_kind(
        root_node.id,
        AnchorId::new_v7(),
        Some("Shell"),
        Some(agent_id),
        ThreadKind::Terminal,
        Some("/tmp/wt-path"),
    )?;

    let custom_leaf = ConversationNode::new(
        NodeId::new_v7(),
        Some(root_node.id),
        AuthorRole::User,
        "turn".to_string(),
    );
    storage.insert_node(&custom_leaf)?;

    let direct_thread = Thread::new(
        ThreadId::new_v7(),
        root_node.id,
        AnchorId::new_v7(),
        custom_leaf.id,
        ThreadLifecycleState::Idle,
    )
    .with_title("Direct Terminal")
    .with_kind(ThreadKind::Terminal)
    .with_cwd("/tmp/wt-direct");
    storage.insert_thread(&direct_thread)?;

    let fetched_term = storage
        .get_thread(term_id)?
        .expect("Terminal thread should exist");
    assert_eq!(fetched_term.kind, ThreadKind::Terminal);
    assert_eq!(fetched_term.cwd.as_deref(), Some("/tmp/wt-path"));
    assert_eq!(fetched_term.parent_thread_id, Some(agent_id));
    assert_eq!(fetched_term.title, "Shell");

    let fetched_direct = storage
        .get_thread(direct_thread.id)?
        .expect("Inserted terminal thread should exist");
    assert_eq!(fetched_direct.kind, ThreadKind::Terminal);
    assert_eq!(fetched_direct.cwd.as_deref(), Some("/tmp/wt-direct"));
    assert_eq!(fetched_direct.active_leaf_id, custom_leaf.id);

    let all = storage.list_threads()?;
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].kind, ThreadKind::Agent);
    assert_eq!(all[0].cwd, None);
    assert_eq!(all[1], fetched_term);
    assert_eq!(all[2], fetched_direct);

    let missing_node = NodeId::new_v7();
    let err_missing_parent = storage.create_thread_with_kind(
        missing_node,
        AnchorId::new_v7(),
        Some("Bad"),
        None,
        ThreadKind::Terminal,
        Some("/tmp/wt"),
    );
    assert!(matches!(
        err_missing_parent,
        Err(CoreError::NodeNotFound(_))
    ));

    let bad_leaf_thread = Thread::new(
        ThreadId::new_v7(),
        root_node.id,
        AnchorId::new_v7(),
        missing_node,
        ThreadLifecycleState::Idle,
    )
    .with_kind(ThreadKind::Terminal);
    let err_missing_leaf = storage.insert_thread(&bad_leaf_thread);
    assert!(matches!(err_missing_leaf, Err(CoreError::NodeNotFound(_))));

    Ok(())
}

#[test]
fn test_update_thread_cwd_set_clear_and_not_found() -> Result<(), Box<dyn std::error::Error>> {
    let storage = SqliteDagStorage::in_memory()?;

    let root_node = ConversationNode::new_root(AuthorRole::User, "hi");
    storage.insert_node(&root_node)?;

    let thread_id = storage.create_thread_with_kind(
        root_node.id,
        AnchorId::new_v7(),
        Some("Worktree Term"),
        None,
        ThreadKind::Terminal,
        Some("/old/wt"),
    )?;

    storage.update_thread_cwd(&thread_id, Some("/new/wt"))?;
    let updated = storage
        .get_thread(thread_id)?
        .expect("Thread should exist after cwd update");
    assert_eq!(updated.cwd.as_deref(), Some("/new/wt"));
    assert_eq!(updated.kind, ThreadKind::Terminal);

    storage.update_thread_cwd(&thread_id, None)?;
    let cleared = storage
        .get_thread(thread_id)?
        .expect("Thread should exist after cwd clear");
    assert_eq!(cleared.cwd, None);

    let missing_id = ThreadId::new_v7();
    let missing_err = storage.update_thread_cwd(&missing_id, Some("/nope"));
    assert!(matches!(missing_err, Err(CoreError::ThreadNotFound(_))));

    Ok(())
}

#[test]
fn test_thread_json_serde_backward_compatibility_and_kind_roundtrip(
) -> Result<(), Box<dyn std::error::Error>> {
    let tid = ThreadId::new_v7();
    let node_id = NodeId::new_v7();
    let anchor_id = AnchorId::new_v7();

    let legacy_json = serde_json::json!({
        "id": tid,
        "parent_node_id": node_id,
        "head_node_id": node_id,
        "anchor_id": anchor_id,
        "active_leaf_id": node_id,
        "leaf_node_id": node_id,
        "status": "idle",
        "state": "idle",
        "title": "Legacy Thread",
        "acp_session_id": null,
        "merged": false,
        "parent_thread_id": null
    })
    .to_string();

    let parsed: Thread = serde_json::from_str(&legacy_json)?;
    assert_eq!(parsed.id, tid);
    assert_eq!(parsed.title, "Legacy Thread");
    assert_eq!(parsed.kind, ThreadKind::Agent);
    assert_eq!(parsed.cwd, None);
    assert_eq!(parsed.custom_name, None);
    assert_eq!(parsed.agent_provider, None);

    let term = Thread::new(tid, node_id, anchor_id, node_id, ThreadLifecycleState::Idle)
        .with_kind(ThreadKind::Terminal)
        .with_cwd("/tmp/wt-path");
    let serialized = serde_json::to_string(&term)?;
    assert!(serialized.contains("\"kind\":\"terminal\""));
    assert!(serialized.contains("\"cwd\":\"/tmp/wt-path\""));

    let roundtripped: Thread = serde_json::from_str(&serialized)?;
    assert_eq!(roundtripped, term);
    assert_eq!(roundtripped.kind(), ThreadKind::Terminal);
    assert_eq!(roundtripped.cwd(), Some("/tmp/wt-path"));

    Ok(())
}

struct TempDirGuard {
    path: std::path::PathBuf,
}

impl TempDirGuard {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("grein-dag-test-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&path).expect("failed to create test temp dir");
        Self { path }
    }

    fn db_path(&self) -> std::path::PathBuf {
        self.path.join("grein.db")
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
fn test_thread_with_custom_name_builder_and_serde_roundtrip(
) -> Result<(), Box<dyn std::error::Error>> {
    let tid = ThreadId::new_v7();
    let node_id = NodeId::new_v7();
    let anchor_id = AnchorId::new_v7();

    let default_thread = Thread::new(tid, node_id, anchor_id, node_id, ThreadLifecycleState::Idle);
    assert_eq!(default_thread.custom_name, None);

    let named = default_thread.with_custom_name(Some("Auth".to_string()));
    assert_eq!(named.custom_name.as_deref(), Some("Auth"));

    let serialized = serde_json::to_string(&named)?;
    assert!(serialized.contains("\"custom_name\":\"Auth\""));
    let roundtripped: Thread = serde_json::from_str(&serialized)?;
    assert_eq!(roundtripped, named);
    assert_eq!(roundtripped.custom_name.as_deref(), Some("Auth"));

    let cleared = named.with_custom_name(None);
    assert_eq!(cleared.custom_name, None);
    let cleared_json = serde_json::to_string(&cleared)?;
    let cleared_roundtrip: Thread = serde_json::from_str(&cleared_json)?;
    assert_eq!(cleared_roundtrip.custom_name, None);

    Ok(())
}

#[test]
fn test_thread_custom_name_persistence_update_clear_and_not_found(
) -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDirGuard::new();
    let db_path = temp.db_path();

    let root_node = ConversationNode::new_root(AuthorRole::User, "hello");
    let default_tid;
    let inserted_tid = ThreadId::new_v7();

    {
        let storage = SqliteDagStorage::open(&db_path)?;
        storage.insert_node(&root_node)?;

        default_tid = storage.create_thread(root_node.id, AnchorId::new_v7())?;
        let created = storage
            .get_thread(default_tid)?
            .expect("created thread must exist");
        assert_eq!(created.custom_name, None);

        let pre_named = Thread::new(
            inserted_tid,
            root_node.id,
            AnchorId::new_v7(),
            root_node.id,
            ThreadLifecycleState::Idle,
        )
        .with_title("Auto Inserted")
        .with_custom_name(Some("Auth".to_string()));
        storage.insert_thread(&pre_named)?;

        storage.update_thread_custom_name(&default_tid, Some("Terminal"))?;
    }

    {
        let reopened = SqliteDagStorage::open(&db_path)?;
        let fetched_default = reopened
            .get_thread(default_tid)?
            .expect("default thread must persist across reload");
        assert_eq!(fetched_default.custom_name.as_deref(), Some("Terminal"));

        let fetched_inserted = reopened
            .get_thread(inserted_tid)?
            .expect("inserted thread must persist across reload");
        assert_eq!(fetched_inserted.title, "Auto Inserted");
        assert_eq!(fetched_inserted.custom_name.as_deref(), Some("Auth"));

        let listed = reopened.list_threads()?;
        assert_eq!(listed, vec![fetched_default, fetched_inserted]);

        reopened.update_thread_custom_name(&default_tid, Some("Auth"))?;
        reopened.update_thread_custom_name(&inserted_tid, None)?;
    }

    {
        let reopened_again = SqliteDagStorage::open(&db_path)?;
        let updated = reopened_again
            .get_thread(default_tid)?
            .expect("updated thread must persist across reload");
        assert_eq!(updated.custom_name.as_deref(), Some("Auth"));

        let cleared = reopened_again
            .get_thread(inserted_tid)?
            .expect("cleared thread must persist across reload");
        assert_eq!(cleared.custom_name, None);
        assert_eq!(cleared.title, "Auto Inserted");

        let listed = reopened_again.list_threads()?;
        assert_eq!(listed, vec![updated, cleared]);

        let missing_id = ThreadId::new_v7();
        let err = reopened_again.update_thread_custom_name(&missing_id, Some("Nope"));
        assert!(matches!(err, Err(CoreError::ThreadNotFound(_))));
    }

    Ok(())
}

#[test]
fn test_legacy_threads_migration_adds_custom_name_column_idempotently(
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
            title TEXT NOT NULL DEFAULT '',
            acp_session_id TEXT,
            merged INTEGER NOT NULL DEFAULT 0,
            parent_thread_id TEXT,
            kind TEXT NOT NULL DEFAULT 'agent',
            cwd TEXT,
            FOREIGN KEY (parent_node_id) REFERENCES nodes(id) ON DELETE RESTRICT,
            FOREIGN KEY (active_leaf_id) REFERENCES nodes(id) ON DELETE RESTRICT
        );",
    )?;

    let root_id = NodeId::new_v7();
    let thread_id = ThreadId::new_v7();
    let anchor_id = AnchorId::new_v7();
    conn.execute(
        "INSERT INTO nodes (id, parent_id, role, content, created_at) VALUES (?1, NULL, 'user', 'Root', 1)",
        rusqlite::params![root_id.to_string()],
    )?;
    conn.execute(
        "INSERT INTO threads (id, parent_node_id, anchor_id, active_leaf_id, status, created_at, title, kind, cwd)
         VALUES (?1, ?2, ?3, ?2, 'idle', 2, 'Auto Title', 'terminal', '/tmp/work')",
        rusqlite::params![thread_id.to_string(), root_id.to_string(), anchor_id.to_string()],
    )?;

    let storage = SqliteDagStorage::new(conn)?;
    storage.run_migrations()?;

    let fetched = storage
        .get_thread(thread_id)?
        .expect("Existing thread must survive migration");
    assert_eq!(fetched.id, thread_id);
    assert_eq!(fetched.title, "Auto Title");
    assert_eq!(fetched.kind, ThreadKind::Terminal);
    assert_eq!(fetched.cwd.as_deref(), Some("/tmp/work"));
    assert_eq!(fetched.custom_name, None);

    let listed = storage.list_threads()?;
    assert_eq!(listed, vec![fetched]);

    storage.update_thread_custom_name(&thread_id, Some("Terminal"))?;
    let renamed = storage
        .get_thread(thread_id)?
        .expect("Migrated thread must support custom_name updates");
    assert_eq!(renamed.custom_name.as_deref(), Some("Terminal"));
    assert_eq!(renamed.title, "Auto Title");
    assert_eq!(renamed.cwd.as_deref(), Some("/tmp/work"));

    Ok(())
}

#[test]
fn test_update_thread_title_and_custom_name_are_independent(
) -> Result<(), Box<dyn std::error::Error>> {
    let storage = SqliteDagStorage::in_memory()?;

    let root_node = ConversationNode::new_root(AuthorRole::User, "root");
    storage.insert_node(&root_node)?;

    let thread_id =
        storage.create_thread_with_title(root_node.id, AnchorId::new_v7(), "Initial Auto Title")?;

    storage.update_thread_custom_name(&thread_id, Some("Auth"))?;
    let after_custom = storage
        .get_thread(thread_id)?
        .expect("Thread must exist after custom_name update");
    assert_eq!(after_custom.title, "Initial Auto Title");
    assert_eq!(after_custom.custom_name.as_deref(), Some("Auth"));

    storage.update_thread_title(&thread_id, "Updated Auto Title")?;
    let after_title = storage
        .get_thread(thread_id)?
        .expect("Thread must exist after title update");
    assert_eq!(after_title.title, "Updated Auto Title");
    assert_eq!(after_title.custom_name.as_deref(), Some("Auth"));

    storage.update_thread_custom_name(&thread_id, Some("Terminal"))?;
    let after_rename = storage
        .get_thread(thread_id)?
        .expect("Thread must exist after custom_name change");
    assert_eq!(after_rename.title, "Updated Auto Title");
    assert_eq!(after_rename.custom_name.as_deref(), Some("Terminal"));

    storage.update_thread_custom_name(&thread_id, None)?;
    let after_clear = storage
        .get_thread(thread_id)?
        .expect("Thread must exist after clearing custom_name");
    assert_eq!(after_clear.title, "Updated Auto Title");
    assert_eq!(after_clear.custom_name, None);

    let listed = storage.list_threads()?;
    assert_eq!(listed, vec![after_clear]);

    Ok(())
}

#[test]
fn test_list_recent_user_prompts_recency_dedup_blanks_and_limit(
) -> Result<(), Box<dyn std::error::Error>> {
    let storage = SqliteDagStorage::in_memory()?;
    assert!(storage.list_recent_user_prompts(10)?.is_empty());
    assert!(storage.list_recent_user_prompts(0)?.is_empty());

    let system = ConversationNode::new_root(AuthorRole::System, "System prompt");
    storage.insert_node(&system)?;

    let u1 = ConversationNode::new(
        NodeId::new_v7(),
        Some(system.id),
        AuthorRole::User,
        "first prompt".to_string(),
    );
    storage.insert_node(&u1)?;

    let a1 = ConversationNode::new(
        NodeId::new_v7(),
        Some(u1.id),
        AuthorRole::Assistant,
        "assistant reply".to_string(),
    );
    storage.insert_node(&a1)?;

    let u2 = ConversationNode::new(
        NodeId::new_v7(),
        Some(a1.id),
        AuthorRole::User,
        "second prompt".to_string(),
    );
    storage.insert_node(&u2)?;

    let t1 = ConversationNode::new(
        NodeId::new_v7(),
        Some(u2.id),
        AuthorRole::ToolResult,
        "tool output".to_string(),
    );
    storage.insert_node(&t1)?;

    let blank_empty = ConversationNode::new(
        NodeId::new_v7(),
        Some(t1.id),
        AuthorRole::User,
        "".to_string(),
    );
    storage.insert_node(&blank_empty)?;

    let blank_ws = ConversationNode::new(
        NodeId::new_v7(),
        Some(blank_empty.id),
        AuthorRole::User,
        "   \n\t  ".to_string(),
    );
    storage.insert_node(&blank_ws)?;

    let u3_dup_trimmed = ConversationNode::new(
        NodeId::new_v7(),
        Some(blank_ws.id),
        AuthorRole::User,
        "  first prompt \n".to_string(),
    );
    storage.insert_node(&u3_dup_trimmed)?;

    let u4_branch = ConversationNode::new(
        NodeId::new_v7(),
        Some(system.id),
        AuthorRole::User,
        "third prompt".to_string(),
    );
    storage.insert_node(&u4_branch)?;

    let u5_dup = ConversationNode::new(
        NodeId::new_v7(),
        Some(u4_branch.id),
        AuthorRole::User,
        "third prompt".to_string(),
    );
    storage.insert_node(&u5_dup)?;

    assert!(storage.list_recent_user_prompts(0)?.is_empty());

    let all = storage.list_recent_user_prompts(10)?;
    assert_eq!(
        all,
        vec![
            "third prompt".to_string(),
            "first prompt".to_string(),
            "second prompt".to_string(),
        ]
    );

    let capped = storage.list_recent_user_prompts(2)?;
    assert_eq!(
        capped,
        vec!["third prompt".to_string(), "first prompt".to_string()]
    );

    Ok(())
}

#[test]
fn test_thread_with_agent_provider_builder_and_serde_roundtrip(
) -> Result<(), Box<dyn std::error::Error>> {
    let tid = ThreadId::new_v7();
    let node_id = NodeId::new_v7();
    let anchor_id = AnchorId::new_v7();

    let default_thread = Thread::new(tid, node_id, anchor_id, node_id, ThreadLifecycleState::Idle);
    assert_eq!(default_thread.agent_provider, None);
    assert_eq!(default_thread.agent_provider(), None);

    let with_provider = default_thread.with_agent_provider("claude");
    assert_eq!(with_provider.agent_provider.as_deref(), Some("claude"));
    assert_eq!(with_provider.agent_provider(), Some("claude"));

    let serialized = serde_json::to_string(&with_provider)?;
    assert!(serialized.contains("\"agent_provider\":\"claude\""));
    let roundtripped: Thread = serde_json::from_str(&serialized)?;
    assert_eq!(roundtripped, with_provider);
    assert_eq!(roundtripped.agent_provider(), Some("claude"));

    Ok(())
}

#[test]
fn test_thread_agent_provider_persistence_update_clear_and_not_found(
) -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDirGuard::new();
    let db_path = temp.db_path();

    let root_node = ConversationNode::new_root(AuthorRole::User, "hello");
    let none_tid = ThreadId::new_v7();
    let provider_tid = ThreadId::new_v7();

    {
        let storage = SqliteDagStorage::open(&db_path)?;
        storage.insert_node(&root_node)?;

        let without_provider = Thread::new(
            none_tid,
            root_node.id,
            AnchorId::new_v7(),
            root_node.id,
            ThreadLifecycleState::Idle,
        )
        .with_title("Default");
        storage.insert_thread(&without_provider)?;

        let with_provider = Thread::new(
            provider_tid,
            root_node.id,
            AnchorId::new_v7(),
            root_node.id,
            ThreadLifecycleState::Idle,
        )
        .with_title("Claude")
        .with_agent_provider("claude");
        storage.insert_thread(&with_provider)?;

        let fetched_none = storage.get_thread(none_tid)?.expect("none_tid must exist");
        assert_eq!(fetched_none.agent_provider(), None);

        let fetched_provider = storage
            .get_thread(provider_tid)?
            .expect("provider_tid must exist");
        assert_eq!(fetched_provider.agent_provider(), Some("claude"));

        let listed = storage.list_threads()?;
        assert_eq!(listed, vec![fetched_none, fetched_provider]);
    }

    {
        let reopened = SqliteDagStorage::open(&db_path)?;
        reopened.update_thread_agent_provider(&none_tid, Some("claude"))?;
        reopened.update_thread_agent_provider(&provider_tid, None)?;
    }

    {
        let reopened_again = SqliteDagStorage::open(&db_path)?;
        let updated_to_some = reopened_again
            .get_thread(none_tid)?
            .expect("none_tid must persist across reload");
        assert_eq!(updated_to_some.agent_provider(), Some("claude"));

        let cleared_to_none = reopened_again
            .get_thread(provider_tid)?
            .expect("provider_tid must persist across reload");
        assert_eq!(cleared_to_none.agent_provider(), None);

        let listed = reopened_again.list_threads()?;
        assert_eq!(listed, vec![updated_to_some, cleared_to_none]);

        let missing_id = ThreadId::new_v7();
        let err = reopened_again.update_thread_agent_provider(&missing_id, Some("claude"));
        assert!(matches!(err, Err(CoreError::ThreadNotFound(_))));
    }

    Ok(())
}

#[test]
fn test_legacy_threads_migration_adds_agent_provider_column_idempotently(
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
            title TEXT NOT NULL DEFAULT '',
            acp_session_id TEXT,
            merged INTEGER NOT NULL DEFAULT 0,
            parent_thread_id TEXT,
            kind TEXT NOT NULL DEFAULT 'agent',
            cwd TEXT,
            custom_name TEXT,
            FOREIGN KEY (parent_node_id) REFERENCES nodes(id) ON DELETE RESTRICT,
            FOREIGN KEY (active_leaf_id) REFERENCES nodes(id) ON DELETE RESTRICT
        );",
    )?;

    let root_id = NodeId::new_v7();
    let thread_id = ThreadId::new_v7();
    let anchor_id = AnchorId::new_v7();
    conn.execute(
        "INSERT INTO nodes (id, parent_id, role, content, created_at) VALUES (?1, NULL, 'user', 'Root', 1)",
        rusqlite::params![root_id.to_string()],
    )?;
    conn.execute(
        "INSERT INTO threads (id, parent_node_id, anchor_id, active_leaf_id, status, created_at, title, kind, cwd, custom_name)
         VALUES (?1, ?2, ?3, ?2, 'idle', 2, 'Auto Title', 'agent', '/tmp/work', 'LegacyCustom')",
        rusqlite::params![thread_id.to_string(), root_id.to_string(), anchor_id.to_string()],
    )?;

    let storage = SqliteDagStorage::new(conn)?;
    storage.run_migrations()?;

    let fetched = storage
        .get_thread(thread_id)?
        .expect("Existing thread must survive migration");
    assert_eq!(fetched.id, thread_id);
    assert_eq!(fetched.title, "Auto Title");
    assert_eq!(fetched.custom_name.as_deref(), Some("LegacyCustom"));
    assert_eq!(fetched.agent_provider, None);

    let listed = storage.list_threads()?;
    assert_eq!(listed, vec![fetched]);

    storage.update_thread_agent_provider(&thread_id, Some("claude"))?;
    let updated = storage
        .get_thread(thread_id)?
        .expect("Migrated thread must support agent_provider updates");
    assert_eq!(updated.agent_provider(), Some("claude"));

    storage.update_thread_agent_provider(&thread_id, None)?;
    let cleared = storage
        .get_thread(thread_id)?
        .expect("Migrated thread must support clearing agent_provider");
    assert_eq!(cleared.agent_provider(), None);

    Ok(())
}
