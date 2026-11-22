use std::ffi::OsString;
use std::time::{Duration, Instant};

use grein_cli::engine::{DaemonEngine, EngineError, InProcessEngine};
use grein_tui::engine_port::PtyScreenSnapshot;

struct HomeEnvGuard {
    previous: Option<OsString>,
}

impl HomeEnvGuard {
    fn set(new_home: &std::path::Path) -> Self {
        let previous = std::env::var_os("HOME");
        std::env::set_var("HOME", new_home);
        Self { previous }
    }
}

impl Drop for HomeEnvGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(prev) => std::env::set_var("HOME", prev),
            None => std::env::remove_var("HOME"),
        }
    }
}

async fn wait_for_pty_snapshot(
    engine: &impl DaemonEngine,
    thread_id: &str,
    rows: u16,
    cols: u16,
    timeout_dur: Duration,
    predicate: impl Fn(&PtyScreenSnapshot) -> bool,
) -> PtyScreenSnapshot {
    let deadline = Instant::now() + timeout_dur;
    loop {
        let snap = engine
            .pty_snapshot(thread_id, rows, cols)
            .await
            .expect("pty_snapshot");
        if predicate(&snap) {
            return snap;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for PTY condition; plain_text:\n---\n{}\n---",
            snap.plain_text()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn test_create_terminal_expands_leading_tilde_cwd() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fake_home = temp.path().join("home_dir");
    let sub_dir = fake_home.join("projects").join("demo");
    let workspace_dir = temp.path().join("workspace");
    std::fs::create_dir_all(&sub_dir).expect("mkdir sub_dir");
    std::fs::create_dir_all(&workspace_dir).expect("mkdir workspace_dir");
    std::fs::write(fake_home.join(".zshrc"), "").expect("write .zshrc");

    let canonical_home = std::fs::canonicalize(&fake_home).expect("canonical home");
    let canonical_sub = std::fs::canonicalize(&sub_dir).expect("canonical sub");
    let canonical_workspace = std::fs::canonicalize(&workspace_dir).expect("canonical workspace");

    let _home_guard = HomeEnvGuard::set(&canonical_home);

    let db_path = temp.path().join("tilde.db");
    let engine = InProcessEngine::open_with_config(&db_path, canonical_workspace, None)
        .expect("open engine");
    let main = engine.ensure_main_thread().await.expect("main");

    let home_term = engine
        .create_terminal(Some(&main.thread_id), Some("Home Term"), Some("~"), 24, 140)
        .await
        .expect("create_terminal ~");
    let sub_term = engine
        .create_terminal(
            Some(&main.thread_id),
            Some("Sub Term"),
            Some("~/projects/demo"),
            24,
            140,
        )
        .await
        .expect("create_terminal ~/projects/demo");

    let missing_err = engine
        .create_terminal(
            Some(&main.thread_id),
            None,
            Some("~/does-not-exist"),
            24,
            80,
        )
        .await
        .expect_err("nonexistent ~/subdir must fail");
    assert!(matches!(missing_err, EngineError::InvalidParams(_)));

    let meta = engine.thread_metadata().await.expect("meta");
    let home_meta = meta
        .iter()
        .find(|m| m.thread_id == home_term.thread_id)
        .expect("home_term meta");
    let sub_meta = meta
        .iter()
        .find(|m| m.thread_id == sub_term.thread_id)
        .expect("sub_term meta");

    assert_eq!(
        home_meta.cwd.as_deref(),
        Some(canonical_home.to_string_lossy().as_ref())
    );
    assert_eq!(
        sub_meta.cwd.as_deref(),
        Some(canonical_sub.to_string_lossy().as_ref())
    );

    engine
        .pty_write(
            &home_term.thread_id,
            b"printf '\\nHOME_PWD=%s\\n' \"$(pwd)\"\n".to_vec(),
        )
        .await
        .expect("pty_write home");
    let expected_home = format!("HOME_PWD={}", canonical_home.to_string_lossy());
    wait_for_pty_snapshot(
        &engine,
        &home_term.thread_id,
        24,
        140,
        Duration::from_secs(5),
        |s| s.plain_text().lines().any(|l| l.trim() == expected_home),
    )
    .await;

    engine
        .pty_write(
            &sub_term.thread_id,
            b"printf '\\nSUB_PWD=%s\\n' \"$(pwd)\"\n".to_vec(),
        )
        .await
        .expect("pty_write sub");
    let expected_sub = format!("SUB_PWD={}", canonical_sub.to_string_lossy());
    wait_for_pty_snapshot(
        &engine,
        &sub_term.thread_id,
        24,
        140,
        Duration::from_secs(5),
        |s| s.plain_text().lines().any(|l| l.trim() == expected_sub),
    )
    .await;
}
