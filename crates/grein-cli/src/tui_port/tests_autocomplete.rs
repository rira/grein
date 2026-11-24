use std::process::Command;

use grein_cli::engine::{
    DaemonEngine, EngineError, EngineEvent, InProcessEngine, PermissionDecisionPayload,
    SessionSnapshot, ShellCommandResult, ThreadSummary, TranscriptEntry, TurnDispatchResult,
};
use grein_core::model::{AuthorRole, ConversationNode, NodeId, ThreadStatus};
use grein_core::storage::sqlite::SqliteStorage;
use grein_tui::engine_port::{TuiEngineEvent, TuiEnginePort};

use super::EngineClientPort;

fn init_git_repo(root: &std::path::Path) {
    let run = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("run git");
        assert!(status.success(), "git failed: {:?}", args);
    };
    run(&["init"]);
    run(&["config", "user.name", "Grein Test"]);
    run(&["config", "user.email", "test@grein.dev"]);
}

#[tokio::test]
async fn test_engine_client_port_autocomplete_and_create_root_in_cwd_in_process() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo_dir = temp.path().join("repo");
    let alt_dir = temp.path().join("alt");
    std::fs::create_dir_all(repo_dir.join("src")).expect("mkdir repo/src");
    std::fs::create_dir_all(&alt_dir).expect("mkdir alt");
    init_git_repo(&repo_dir);
    std::fs::write(repo_dir.join(".gitignore"), "*.log\n").expect("write gitignore");
    std::fs::write(repo_dir.join("README.md"), "readme\n").expect("write readme");
    std::fs::write(repo_dir.join("src/lib.rs"), "pub fn ok() {}\n").expect("write lib.rs");
    std::fs::write(repo_dir.join("ignored.log"), "secret\n").expect("write ignored");

    init_git_repo(&alt_dir);
    std::fs::write(alt_dir.join("alt_only.rs"), "pub fn alt() {}\n").expect("write alt");

    let canonical_repo = std::fs::canonicalize(&repo_dir).expect("canonical repo");
    let canonical_alt = std::fs::canonicalize(&alt_dir).expect("canonical alt");
    let db_path = temp.path().join("port_autocomplete.db");

    {
        let storage = SqliteStorage::open(&db_path).expect("open sqlite");
        let root = ConversationNode::new_root(AuthorRole::System, "");
        storage.insert_node(&root).expect("insert root");
        let u1 = ConversationNode::new(NodeId::new(), Some(root.id), AuthorRole::User, "first prompt".into());
        let a1 = ConversationNode::new(NodeId::new(), Some(u1.id), AuthorRole::Assistant, "reply 1".into());
        let u2 = ConversationNode::new(NodeId::new(), Some(a1.id), AuthorRole::User, "second prompt".into());
        let u3 = ConversationNode::new(NodeId::new(), Some(u2.id), AuthorRole::User, "first prompt".into());
        for node in [&u1, &a1, &u2, &u3] {
            storage.insert_node(node).expect("insert node");
        }
    }

    let engine = InProcessEngine::open_with_config(&db_path, canonical_repo.clone(), None)
        .expect("open engine");
    let port = EngineClientPort::new(engine);
    let _session = port.hydrate_session().await.expect("hydrate");

    let prompts = port
        .list_recent_user_prompts(10)
        .await
        .expect("list_recent_user_prompts");
    assert_eq!(prompts, vec!["first prompt", "second prompt"]);

    let limited = port
        .list_recent_user_prompts(1)
        .await
        .expect("list_recent_user_prompts limit 1");
    assert_eq!(limited, vec!["first prompt"]);

    let repo_files = port
        .list_workspace_files(None)
        .await
        .expect("list_workspace_files None");
    assert!(repo_files.contains(&"README.md".to_string()));
    assert!(repo_files.contains(&"src/lib.rs".to_string()));
    assert!(!repo_files.contains(&"ignored.log".to_string()));

    let alt_files = port
        .list_workspace_files(Some(&alt_dir.to_string_lossy()))
        .await
        .expect("list_workspace_files alt");
    assert_eq!(alt_files, vec!["alt_only.rs".to_string()]);

    let created = port
        .create_root_in_cwd(Some("Scoped Root"), Some(&alt_dir.to_string_lossy()))
        .await
        .expect("create_root_in_cwd");
    assert_eq!(created.custom_name.as_deref(), Some("Scoped Root"));

    let meta = port.thread_metadata().await.expect("thread_metadata");
    let created_meta = meta
        .iter()
        .find(|m| m.thread_id == created.id)
        .expect("created meta");
    assert_eq!(
        created_meta.cwd.as_deref(),
        Some(canonical_alt.to_string_lossy().as_ref())
    );

    let mut rx = port.events_rx.lock().await.take().expect("events_rx");
    let mut offline_evs = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        offline_evs.push(ev);
    }
    assert!(offline_evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ThreadCreated { thread } if thread.id == created.id
    )));
    assert!(offline_evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ThreadMetadataUpdated { meta }
            if meta.iter().any(|m| {
                m.thread_id == created.id
                    && m.cwd.as_deref() == Some(canonical_alt.to_string_lossy().as_ref())
            })
    )));

    let err_missing = port
        .create_root_in_cwd(Some("Bad"), Some("/nonexistent/grein_missing_dir"))
        .await
        .expect_err("missing cwd must fail");
    assert!(err_missing.to_string().contains("does not exist"));
}

#[tokio::test]
async fn test_engine_client_port_graceful_fallback_on_32601_and_unsupported() {
    struct StaleMockEngine {
        fail_transport: bool,
    }

    #[async_trait::async_trait]
    impl DaemonEngine for StaleMockEngine {
        async fn ping(&self) -> Result<(), EngineError> {
            Ok(())
        }
        async fn ensure_main_thread(&self) -> Result<ThreadSummary, EngineError> {
            Ok(ThreadSummary {
                thread_id: "t-main".into(),
                title: "Main".into(),
                custom_name: None,
                parent_node_id: "n-1".into(),
                active_leaf_id: "n-1".into(),
                merged: false,
                status: ThreadStatus::Idle,
            })
        }
        async fn send_turn(&self, _: &str) -> Result<TurnDispatchResult, EngineError> {
            unreachable!()
        }
        async fn send_turn_for_thread(
            &self,
            _: &str,
            _: &str,
        ) -> Result<TurnDispatchResult, EngineError> {
            unreachable!()
        }
        async fn fork_thread(&self, _: Option<&str>) -> Result<ThreadSummary, EngineError> {
            unreachable!()
        }
        async fn fork_thread_with_title(
            &self,
            _: Option<&str>,
            _: Option<&str>,
        ) -> Result<ThreadSummary, EngineError> {
            unreachable!()
        }
        async fn create_root_thread(&self, title: Option<&str>) -> Result<ThreadSummary, EngineError> {
            Ok(ThreadSummary {
                thread_id: "t-fallback".into(),
                title: "Root 2".into(),
                custom_name: title.map(str::to_string),
                parent_node_id: "n-2".into(),
                active_leaf_id: "n-2".into(),
                merged: false,
                status: ThreadStatus::Idle,
            })
        }
        async fn create_root_thread_in_cwd(
            &self,
            _: Option<&str>,
            _: Option<&str>,
        ) -> Result<ThreadSummary, EngineError> {
            Err(EngineError::Protocol("method not found: session.create_root".into()))
        }
        async fn rename_thread(&self, _: &str, _: &str) -> Result<ThreadSummary, EngineError> {
            unreachable!()
        }
        async fn list_threads(&self) -> Result<Vec<ThreadSummary>, EngineError> {
            Ok(vec![self.ensure_main_thread().await?])
        }
        async fn get_transcript(&self) -> Result<Vec<TranscriptEntry>, EngineError> {
            Ok(Vec::new())
        }
        async fn get_thread_transcript(&self, _: &str) -> Result<Vec<TranscriptEntry>, EngineError> {
            Ok(Vec::new())
        }
        async fn merge_thread(&self, _: &str) -> Result<ThreadSummary, EngineError> {
            unreachable!()
        }
        async fn merge_thread_with_summary(
            &self,
            _: &str,
            _: Option<&str>,
            _: Option<&str>,
        ) -> Result<ThreadSummary, EngineError> {
            unreachable!()
        }
        async fn resolve_permission(
            &self,
            _: PermissionDecisionPayload,
        ) -> Result<EngineEvent, EngineError> {
            unreachable!()
        }
        async fn cancel_turn(&self, _: &str) -> Result<(), EngineError> {
            Ok(())
        }
        async fn run_shell_command(&self, _: &str, _: &str) -> Result<ShellCommandResult, EngineError> {
            unreachable!()
        }
        async fn snapshot(&self) -> Result<SessionSnapshot, EngineError> {
            unreachable!()
        }
        async fn list_recent_user_prompts(&self, _: usize) -> Result<Vec<String>, EngineError> {
            if self.fail_transport {
                Err(EngineError::Transport("broken pipe".into()))
            } else {
                Err(EngineError::Protocol(
                    "method not found: session.list_recent_prompts".into(),
                ))
            }
        }
        async fn list_workspace_files(&self, _: Option<&str>) -> Result<Vec<String>, EngineError> {
            if self.fail_transport {
                Err(EngineError::Transport("broken pipe".into()))
            } else {
                Err(EngineError::Protocol("-32601: unsupported".into()))
            }
        }
    }

    let stale_port = EngineClientPort::new(StaleMockEngine {
        fail_transport: false,
    });
    assert!(stale_port
        .list_recent_user_prompts(200)
        .await
        .expect("unsupported prompts must return empty")
        .is_empty());
    assert!(stale_port
        .list_workspace_files(None)
        .await
        .expect("unsupported files must return empty")
        .is_empty());

    let fallback_root = stale_port
        .create_root_in_cwd(Some("Fallback"), Some("/tmp"))
        .await
        .expect("create_root_in_cwd fallback");
    assert_eq!(fallback_root.id, "t-fallback");
    assert_eq!(fallback_root.custom_name.as_deref(), Some("Fallback"));

    let mut rx = stale_port.events_rx.lock().await.take().expect("events_rx");
    let mut evs = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        evs.push(ev);
    }
    assert!(evs.iter().any(|e| matches!(
        e,
        TuiEngineEvent::ThreadCreated { thread } if thread.id == "t-fallback"
    )));

    let transport_err_port = EngineClientPort::new(StaleMockEngine {
        fail_transport: true,
    });
    assert!(transport_err_port
        .list_recent_user_prompts(200)
        .await
        .is_err());
    assert!(transport_err_port.list_workspace_files(None).await.is_err());
}
