use grein_core::model::{AuthorRole, ThreadId};
use grein_core::storage::sqlite::SqliteStorage;
use grein_tui::app::App;
use grein_tui::runner::execute_agent_prompt;

#[test]
fn test_runner_shell_execution_and_sqlite_dag_persistence() {
    let out = execute_agent_prompt("printf 'real_shell_ok'");
    assert_eq!(
        out,
        "real_shell_ok",
        "execute_agent_prompt must run real shell commands without mock fallback (runner.rs:40 runner.rs:120 runner.rs:200 runner.rs:280)"
    );
    assert!(!out.contains("Processed prompt in standalone session."));

    let db_path =
        std::env::temp_dir().join(format!("grein-runner-test-{}.sqlite3", ThreadId::now_v7()));
    let _ = std::fs::remove_file(&db_path);

    let mut app = App::new();
    app.submit_composer_prompt("echo first".to_string());
    app.apply_agent_response("first_output");
    app.persist_state_to_sqlite(&db_path);

    let meta_path = std::path::PathBuf::from(format!("{}.meta", db_path.display()));
    assert!(!meta_path.exists(), "Sidecar .meta file must not exist");

    let storage = SqliteStorage::open(&db_path).unwrap();
    let tid_entry = storage
        .get_blackboard_entry("active_thread_id")
        .unwrap()
        .expect("active_thread_id must be persisted in blackboard_entries");
    let tid: ThreadId = tid_entry.value.parse().ok().map(ThreadId).unwrap();
    let thread = storage
        .get_thread(tid)
        .unwrap()
        .expect("Thread row must exist");
    let trunk = storage.get_trunk_path(thread.active_leaf_id).unwrap();
    assert_eq!(trunk.len(), 2);
    assert_eq!(trunk[0].role, AuthorRole::User);
    assert_eq!(trunk[1].role, AuthorRole::Assistant);

    let mut loaded = App::new();
    loaded.load_state_from_sqlite(&db_path);
    assert_eq!(loaded.threads.len(), 1);
    assert_ne!(loaded.threads[0].id, "thread-1");
    assert!(loaded.active_transcript_md.contains("first_output"));

    let _ = std::fs::remove_file(&db_path);
}
