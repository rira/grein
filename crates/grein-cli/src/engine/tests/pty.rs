use grein_core::model::ThreadKind;
use grein_tui::engine_port::TuiEngineEvent;

use super::super::{EngineError, InProcessEngine};

#[tokio::test]
async fn test_create_terminal_emits_metadata_updated_before_thread_created() {
    let temp = tempfile::tempdir().expect("tempdir");
    let work_dir = temp.path().join("term_dir");
    std::fs::create_dir_all(&work_dir).expect("mkdir");
    let canonical_dir = std::fs::canonicalize(&work_dir).expect("canonical");
    let db_path = temp.path().join("term_order.db");

    let engine =
        InProcessEngine::open_with_config(&db_path, canonical_dir.clone(), None)
            .expect("open");
    let main = engine.ensure_main_thread().await.expect("main");
    let mut rx = engine.subscribe_tui_events();

    let term = engine
        .create_terminal(Some(&main.thread_id), Some("Ordered Term"), None, 24, 80)
        .await
        .expect("create_terminal");

    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }

    let meta_idx = events
        .iter()
        .position(|ev| {
            matches!(
                ev,
                TuiEngineEvent::ThreadMetadataUpdated { meta }
                    if meta.iter().any(|m| {
                        m.thread_id == term.thread_id
                            && m.kind == ThreadKind::Terminal
                            && m.cwd.as_deref() == Some(canonical_dir.to_string_lossy().as_ref())
                    })
            )
        })
        .expect("expected ThreadMetadataUpdated containing new terminal meta");
    let created_idx = events
        .iter()
        .position(|ev| {
            matches!(
                ev,
                TuiEngineEvent::ThreadCreated { thread } if thread.id == term.thread_id
            )
        })
        .expect("expected ThreadCreated for terminal");

    assert!(
        meta_idx < created_idx,
        "ThreadMetadataUpdated (idx {meta_idx}) must precede ThreadCreated (idx {created_idx}): {events:?}"
    );
}

#[test]
fn test_resolve_terminal_cwd_tilde_edge_cases() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("ws");
    let home = temp.path().join("fake_home");
    let home_sub = home.join("sub");
    std::fs::create_dir_all(&workspace).expect("mkdir ws");
    std::fs::create_dir_all(&home_sub).expect("mkdir home_sub");
    let canonical_home = std::fs::canonicalize(&home).expect("canonical home");
    let canonical_sub = std::fs::canonicalize(&home_sub).expect("canonical sub");

    let db_path = temp.path().join("tilde_unit.db");
    let engine = InProcessEngine::open_with_config(&db_path, workspace, None).expect("open");
    let home_str = canonical_home.to_string_lossy().into_owned();

    assert_eq!(
        engine
            .runtime
            .resolve_terminal_cwd_with_home(Some("~"), None, Some(&home_str))
            .expect("~ resolves"),
        home_str
    );
    assert_eq!(
        engine
            .runtime
            .resolve_terminal_cwd_with_home(Some("~/"), None, Some(&home_str))
            .expect("~/ resolves"),
        home_str
    );
    assert_eq!(
        engine
            .runtime
            .resolve_terminal_cwd_with_home(Some("~/sub"), None, Some(&home_str))
            .expect("~/sub resolves"),
        canonical_sub.to_string_lossy().as_ref()
    );
    assert_eq!(
        engine
            .runtime
            .resolve_terminal_cwd_with_home(Some("~//sub"), None, Some(&home_str))
            .expect("~//sub resolves without dropping home prefix"),
        canonical_sub.to_string_lossy().as_ref()
    );

    // Unset or empty HOME does not expand tilde.
    assert!(matches!(
        engine
            .runtime
            .resolve_terminal_cwd_with_home(Some("~"), None, None),
        Err(EngineError::InvalidParams(_))
    ));
    assert!(matches!(
        engine
            .runtime
            .resolve_terminal_cwd_with_home(Some("~/sub"), None, Some("")),
        Err(EngineError::InvalidParams(_))
    ));
    // ~otheruser is not expanded to $HOME.
    assert!(matches!(
        engine
            .runtime
            .resolve_terminal_cwd_with_home(Some("~otheruser/sub"), None, Some(&home_str)),
        Err(EngineError::InvalidParams(_))
    ));
}

#[tokio::test]
async fn test_pending_pty_sizes_clamping_and_delete_cleanup() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("pending_clamp.db");

    let term_id = {
        let engine =
            InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
                .expect("open");
        let _main = engine.ensure_main_thread().await.expect("main");
        let term = engine
            .create_terminal(None, Some("Clamp Term"), None, 24, 80)
            .await
            .expect("create");
        term.thread_id
    };

    let reopened =
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
            .expect("reopen");
    reopened
        .pty_resize(&term_id, 2, 5)
        .await
        .expect("clamped pending resize");
    assert_eq!(
        reopened
            .runtime
            .pending_pty_sizes
            .lock()
            .unwrap()
            .get(&term_id)
            .copied(),
        Some((4, 10))
    );

    // Zero resize leaves pending size untouched.
    reopened.pty_resize(&term_id, 0, 80).await.expect("zero resize");
    assert_eq!(
        reopened
            .runtime
            .pending_pty_sizes
            .lock()
            .unwrap()
            .get(&term_id)
            .copied(),
        Some((4, 10))
    );

    reopened.delete_thread(&term_id).await.expect("delete");
    assert!(!reopened
        .runtime
        .pending_pty_sizes
        .lock()
        .unwrap()
        .contains_key(&term_id));
}
