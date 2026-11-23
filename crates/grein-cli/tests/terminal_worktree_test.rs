use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use grein_cli::daemon::run_daemon_with_engine;
use grein_cli::engine::{
    AcpBackendConfig, DaemonEngine, EngineError, InProcessEngine, UnixSocketEngineClient,
};
use grein_core::model::ThreadKind;
use grein_tui::engine_port::{PtyScreenSnapshot, TuiEngineEvent};
use serde_json::Value;
use tokio::net::UnixStream;
use tokio::time::timeout;

fn write_executable_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).expect("write script");
    let mut perms = std::fs::metadata(&path).expect("metadata").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("chmod");
    path
}

fn init_test_git_repo(root: &Path) {
    let run = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("run git");
        assert!(status.success(), "git command failed: {:?}", args);
    };

    run(&["init"]);
    run(&["config", "user.name", "Grein Test"]);
    run(&["config", "user.email", "test@grein.dev"]);
    std::fs::write(root.join("README.md"), "base repo\n").expect("write README");
    run(&["add", "."]);
    run(&["commit", "-m", "Initial commit"]);
}

fn is_process_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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
async fn test_ensure_thread_worktree_updates_cwd_restarts_acp_and_inherits_into_children() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo_root = temp.path().join("repo");
    std::fs::create_dir_all(&repo_root).expect("mkdir repo");
    init_test_git_repo(&repo_root);
    let canonical_repo = std::fs::canonicalize(&repo_root).expect("canonical repo");

    let db_path = temp.path().join("session.db");
    let capture_path = temp.path().join("acp_worktree_capture.jsonl");

    let mock_script = format!(
        r#"#!/bin/sh
export GREIN_CAPTURE={capture:?}
sess_counter=0
while IFS= read -r line; do
    printf '%s\n' "$line" >> "$GREIN_CAPTURE"
    rest=${{line#*\"id\":}}; id=${{rest%%,*}}; id=${{id%%\}}*}}
    case "$line" in
        *'"method":"initialize"'*)
            printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":1,"agentInfo":{{"name":"mock","version":"1.0"}},"authMethods":[],"agentCapabilities":{{}}}}}}\n' "$id"
            ;;
        *'"method":"session/new"'*)
            sess_counter=$((sess_counter+1))
            printf '{{"jsonrpc":"2.0","id":%s,"result":{{"sessionId":"sess-%s"}}}}\n' "$id" "$sess_counter"
            ;;
        *'"method":"session/close"'*)
            printf '{{"jsonrpc":"2.0","id":%s,"result":{{}}}}\n' "$id"
            ;;
        *'"method":"session/prompt"'*)
            case "$line" in
                *'"sessionId":"sess-1"'*) sid="sess-1" ;;
                *) sid="sess-2" ;;
            esac
            printf '{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"%s","update":{{"sessionUpdate":"agent_message_chunk","content":{{"type":"text","text":"ok"}}}}}}}}\n' "$sid"
            printf '{{"jsonrpc":"2.0","id":%s,"result":{{"stopReason":"end_turn"}}}}\n' "$id"
            ;;
    esac
done
"#,
        capture = capture_path.to_string_lossy()
    );
    let script_path = write_executable_script(temp.path(), "mock_wt_acp.sh", &mock_script);
    let cfg = AcpBackendConfig {
        command: script_path.to_string_lossy().into_owned(),
        args: Vec::new(),
        auth_method: None,
        cwd: canonical_repo.clone(),
    };

    let engine = InProcessEngine::open_with_config(&db_path, canonical_repo.clone(), Some(cfg))
        .expect("open engine");
    let main = engine.ensure_main_thread().await.expect("main");
    let branch = engine
        .fork_thread_with_title(Some(&main.thread_id), Some("wt-branch"))
        .await
        .expect("fork branch");

    engine
        .send_turn_for_thread(&branch.thread_id, "before worktree")
        .await
        .expect("turn before worktree");

    let mut rx = engine.subscribe_tui_events();
    let wt_path = engine
        .ensure_thread_worktree(&branch.thread_id)
        .await
        .expect("ensure_thread_worktree");

    let expected_wt = std::fs::canonicalize(
        canonical_repo
            .join(".grein")
            .join("worktrees")
            .join(&branch.thread_id),
    )
    .expect("canonical worktree");
    assert_eq!(wt_path, expected_wt.to_string_lossy());
    assert!(expected_wt.join(".git").exists());

    // Idempotent second call returns identical path.
    let wt_path_again = engine
        .ensure_thread_worktree(&branch.thread_id)
        .await
        .expect("idempotent ensure_thread_worktree");
    assert_eq!(wt_path_again, wt_path);

    let mut saw_meta_update = false;
    while let Ok(ev) = rx.try_recv() {
        if let TuiEngineEvent::ThreadMetadataUpdated { meta } = ev {
            if meta.iter().any(|m| {
                m.thread_id == branch.thread_id
                    && m.cwd.as_deref() == Some(wt_path.as_str())
                    && m.kind == ThreadKind::Agent
            }) {
                saw_meta_update = true;
            }
        }
    }
    assert!(saw_meta_update, "must emit ThreadMetadataUpdated with worktree cwd");

    let meta = engine.thread_metadata().await.expect("thread_metadata");
    let branch_meta = meta
        .iter()
        .find(|m| m.thread_id == branch.thread_id)
        .expect("branch meta");
    assert_eq!(branch_meta.cwd.as_deref(), Some(wt_path.as_str()));

    // !cmd shell runs inside the branch's isolated worktree cwd.
    let shell_res = engine
        .run_shell_command(&branch.thread_id, "pwd")
        .await
        .expect("shell pwd");
    assert_eq!(shell_res.stdout.trim(), wt_path);

    // Subsequent ACP turn uses a fresh session rooted at the isolated worktree cwd.
    engine
        .send_turn_for_thread(&branch.thread_id, "after worktree")
        .await
        .expect("turn after worktree");

    let frames: Vec<Value> = std::fs::read_to_string(&capture_path)
        .expect("read capture")
        .lines()
        .map(|l| serde_json::from_str(l).expect("json"))
        .collect();
    let session_news: Vec<&Value> = frames
        .iter()
        .filter(|f| f.get("method").and_then(Value::as_str) == Some("session/new"))
        .collect();
    assert_eq!(session_news.len(), 2, "must reopen ACP session with new worktree cwd");
    assert_eq!(
        session_news[0]["params"]["cwd"].as_str(),
        Some(canonical_repo.to_string_lossy().as_ref())
    );
    assert_eq!(
        session_news[1]["params"]["cwd"].as_str(),
        Some(wt_path.as_str())
    );

    // Forked Agent child branch inherits parent's worktree cwd.
    let child_agent = engine
        .fork_thread_with_title(Some(&branch.thread_id), Some("child-agent"))
        .await
        .expect("fork child agent");
    let meta_after_fork = engine.thread_metadata().await.expect("meta after fork");
    let child_meta = meta_after_fork
        .iter()
        .find(|m| m.thread_id == child_agent.thread_id)
        .expect("child meta");
    assert_eq!(child_meta.cwd.as_deref(), Some(wt_path.as_str()));

    // Child Terminal created under worktree branch inherits worktree cwd automatically.
    let child_term = engine
        .create_terminal(Some(&branch.thread_id), None, None, 24, 140)
        .await
        .expect("create child terminal");
    assert_eq!(child_term.title, "Terminal");

    let meta_after_term = engine.thread_metadata().await.expect("meta after term");
    let term_meta = meta_after_term
        .iter()
        .find(|m| m.thread_id == child_term.thread_id)
        .expect("term meta");
    assert_eq!(term_meta.kind, ThreadKind::Terminal);
    assert_eq!(term_meta.cwd.as_deref(), Some(wt_path.as_str()));
    assert_eq!(
        term_meta.parent_thread_id.as_deref(),
        Some(branch.thread_id.as_str())
    );

    engine
        .pty_write(
            &child_term.thread_id,
            b"printf '\\nTERM_PWD=%s\\n' \"$(pwd)\"\n".to_vec(),
        )
        .await
        .expect("pty_write");
    let expected_line = format!("TERM_PWD={wt_path}");
    wait_for_pty_snapshot(&engine, &child_term.thread_id, 24, 140, Duration::from_secs(5), |s| {
        s.plain_text().lines().any(|l| l.trim() == expected_line)
    })
    .await;

    // Terminal threads can also allocate their own dedicated worktree.
    let child_wt_path = engine
        .ensure_thread_worktree(&child_term.thread_id)
        .await
        .expect("ensure_thread_worktree on terminal must succeed");
    let expected_child_wt = std::fs::canonicalize(
        canonical_repo
            .join(".grein")
            .join("worktrees")
            .join(&child_term.thread_id),
    )
    .expect("canonical child terminal worktree");
    assert_eq!(child_wt_path, expected_child_wt.to_string_lossy());
    assert!(expected_child_wt.join(".git").exists());
}

#[tokio::test]
async fn test_ensure_thread_worktree_on_terminal_creates_worktree_respawns_pty_and_is_idempotent() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo_root = temp.path().join("repo");
    std::fs::create_dir_all(&repo_root).expect("mkdir repo");
    init_test_git_repo(&repo_root);
    let canonical_repo = std::fs::canonicalize(&repo_root).expect("canonical repo");
    let db_path = temp.path().join("term_wt.db");

    let term_id = {
        let engine = InProcessEngine::open_with_config(&db_path, canonical_repo.clone(), None)
            .expect("open engine");
        let term = engine
            .create_terminal(None, Some("Root Term"), None, 26, 120)
            .await
            .expect("create root terminal");

        engine
            .pty_write(
                &term.thread_id,
                b"printf '\\nINIT_PWD=%s PID=%s\\n' \"$(pwd)\" \"$$\"\n".to_vec(),
            )
            .await
            .expect("pty_write init");
        let init_prefix = format!("INIT_PWD={} PID=", canonical_repo.to_string_lossy());
        let init_snap =
            wait_for_pty_snapshot(&engine, &term.thread_id, 26, 120, Duration::from_secs(5), |s| {
                s.plain_text()
                    .lines()
                    .any(|l| l.trim().starts_with(&init_prefix))
            })
            .await;
        let old_pid: u32 = init_snap
            .plain_text()
            .lines()
            .find(|l| l.trim().starts_with(&init_prefix))
            .expect("init pid line")
            .trim()
            .strip_prefix(&init_prefix)
            .expect("strip prefix")
            .trim()
            .parse()
            .expect("parse old pid");
        assert!(is_process_alive(old_pid), "initial PTY shell must be alive");

        let mut rx = engine.subscribe_tui_events();
        let wt_path = engine
            .ensure_thread_worktree(&term.thread_id)
            .await
            .expect("ensure_thread_worktree on terminal");

        let expected_wt = std::fs::canonicalize(
            canonical_repo
                .join(".grein")
                .join("worktrees")
                .join(&term.thread_id),
        )
        .expect("canonical worktree");
        assert_eq!(wt_path, expected_wt.to_string_lossy());
        assert!(expected_wt.join(".git").exists());

        let expected_branch = format!("worktree-{}", term.thread_id);
        let mut saw_idle = false;
        let mut saw_screen = false;
        let mut saw_meta = false;
        let ev_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < ev_deadline && !(saw_idle && saw_screen && saw_meta) {
            match timeout(Duration::from_millis(200), rx.recv()).await {
                Ok(Some(TuiEngineEvent::ThreadStatusChanged { thread_id, status }))
                    if thread_id == term.thread_id && status == "idle" =>
                {
                    saw_idle = true;
                }
                Ok(Some(TuiEngineEvent::PtyScreenUpdated {
                    thread_id,
                    snapshot,
                })) if thread_id == term.thread_id
                    && !snapshot.exited
                    && snapshot.rows == 26
                    && snapshot.cols == 120 =>
                {
                    saw_screen = true;
                }
                Ok(Some(TuiEngineEvent::ThreadMetadataUpdated { meta }))
                    if meta.iter().any(|m| {
                        m.thread_id == term.thread_id
                            && m.cwd.as_deref() == Some(wt_path.as_str())
                            && m.git_branch.as_deref() == Some(expected_branch.as_str())
                    }) =>
                {
                    saw_meta = true;
                }
                Ok(Some(_)) => {}
                _ => break,
            }
        }
        assert!(saw_idle, "expected ThreadStatusChanged idle");
        assert!(saw_screen, "expected PtyScreenUpdated for respawned PTY");
        assert!(saw_meta, "expected ThreadMetadataUpdated with worktree cwd and branch");

        let old_deadline = Instant::now() + Duration::from_secs(2);
        while is_process_alive(old_pid) {
            assert!(
                Instant::now() < old_deadline,
                "old PTY shell {old_pid} must terminate on worktree transition"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        engine
            .pty_write(
                &term.thread_id,
                b"printf '\\nWT_TERM_PWD=%s PID=%s\\n' \"$(pwd)\" \"$$\"\n".to_vec(),
            )
            .await
            .expect("pty_write in worktree");
        let wt_prefix = format!("WT_TERM_PWD={wt_path} PID=");
        let wt_snap =
            wait_for_pty_snapshot(&engine, &term.thread_id, 26, 120, Duration::from_secs(5), |s| {
                s.plain_text()
                    .lines()
                    .any(|l| l.trim().starts_with(&wt_prefix))
            })
            .await;
        let new_pid: u32 = wt_snap
            .plain_text()
            .lines()
            .find(|l| l.trim().starts_with(&wt_prefix))
            .expect("wt pid line")
            .trim()
            .strip_prefix(&wt_prefix)
            .expect("strip wt prefix")
            .trim()
            .parse()
            .expect("parse new pid");
        assert_ne!(new_pid, old_pid, "PTY must respawn with a new shell process");
        assert!(is_process_alive(new_pid));

        let meta_now = engine.thread_metadata().await.expect("meta");
        let term_meta = meta_now
            .iter()
            .find(|m| m.thread_id == term.thread_id)
            .expect("term meta");
        assert_eq!(term_meta.git_branch.as_deref(), Some(expected_branch.as_str()));
        assert_eq!(term_meta.cwd.as_deref(), Some(wt_path.as_str()));

        // Idempotent when invoked twice at worktree root: preserves live PTY.
        let wt_path_again = engine
            .ensure_thread_worktree(&term.thread_id)
            .await
            .expect("second ensure_thread_worktree");
        assert_eq!(wt_path_again, wt_path);
        assert!(is_process_alive(new_pid), "idempotent call must keep live PTY");

        // Move cwd into a subdirectory inside the worktree via OSC 7 and invoke again.
        let sub_wt = expected_wt.join("sub_wt");
        std::fs::create_dir_all(&sub_wt).expect("mkdir sub_wt");
        let canonical_sub_wt = std::fs::canonicalize(&sub_wt).expect("canonical sub_wt");
        let canonical_sub_wt_str = canonical_sub_wt.to_string_lossy().into_owned();

        let osc7_cmd = format!(
            "cd '{}' && printf '\\n' && printf '\\033]7;file://localhost%s\\007\\n' '{}' && printf 'SUB_WT_READY\\n'\n",
            canonical_sub_wt_str, canonical_sub_wt_str
        );
        engine
            .pty_write(&term.thread_id, osc7_cmd.into_bytes())
            .await
            .expect("pty_write sub_wt osc7");
        wait_for_pty_snapshot(&engine, &term.thread_id, 26, 120, Duration::from_secs(5), |s| {
            s.plain_text().lines().any(|l| l.trim() == "SUB_WT_READY")
        })
        .await;

        let sub_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let meta_sub = engine.thread_metadata().await.expect("meta sub");
            let cur_cwd = meta_sub
                .iter()
                .find(|m| m.thread_id == term.thread_id)
                .and_then(|m| m.cwd.clone());
            if cur_cwd.as_deref() == Some(canonical_sub_wt_str.as_str()) {
                break;
            }
            assert!(
                tokio::time::Instant::now() < sub_deadline,
                "expected OSC 7 cwd to update to {canonical_sub_wt_str}, got {cur_cwd:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let wt_path_from_sub = engine
            .ensure_thread_worktree(&term.thread_id)
            .await
            .expect("ensure_thread_worktree from subdir inside worktree");
        assert_eq!(wt_path_from_sub, wt_path);
        assert!(
            is_process_alive(new_pid),
            "ensure_thread_worktree inside worktree subdir must not kill live PTY"
        );

        let meta_after_sub = engine.thread_metadata().await.expect("meta after sub");
        let term_meta_sub = meta_after_sub
            .iter()
            .find(|m| m.thread_id == term.thread_id)
            .expect("term meta sub");
        assert_eq!(
            term_meta_sub.cwd.as_deref(),
            Some(canonical_sub_wt_str.as_str()),
            "idempotent ensure_thread_worktree must preserve subdir cwd inside worktree"
        );

        engine
            .pty_write(
                &term.thread_id,
                b"printf '\\nSTILL_ALIVE_PWD=%s PID=%s\\n' \"$(pwd)\" \"$$\"\n".to_vec(),
            )
            .await
            .expect("pty_write still alive");
        let alive_line = format!("STILL_ALIVE_PWD={canonical_sub_wt_str} PID={new_pid}");
        wait_for_pty_snapshot(&engine, &term.thread_id, 26, 120, Duration::from_secs(5), |s| {
            s.plain_text().lines().any(|l| l.trim() == alive_line)
        })
        .await;

        term.thread_id
    };

    // Cold reopen: verify lazy PTY spawns inside persisted worktree subdir.
    let reopened = InProcessEngine::open_with_config(&db_path, canonical_repo.clone(), None)
        .expect("reopen");
    let _ = reopened
        .pty_snapshot(&term_id, 24, 140)
        .await
        .expect("lazy pty_snapshot reopened");
    reopened
        .pty_write(
            &term_id,
            b"printf '\\nCOLD_SUB_PWD=%s\\n' \"$(pwd)\"\n".to_vec(),
        )
        .await
        .expect("pty_write reopened");
    let expected_sub = canonical_repo
        .join(".grein")
        .join("worktrees")
        .join(&term_id)
        .join("sub_wt");
    let expected_cold = format!("COLD_SUB_PWD={}", expected_sub.to_string_lossy());
    wait_for_pty_snapshot(&reopened, &term_id, 24, 140, Duration::from_secs(5), |s| {
        s.plain_text().lines().any(|l| l.trim() == expected_cold)
    })
    .await;
}

#[tokio::test]
async fn test_ensure_thread_worktree_on_cold_terminal_updates_cwd_before_lazy_spawn() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo_root = temp.path().join("repo");
    std::fs::create_dir_all(&repo_root).expect("mkdir repo");
    init_test_git_repo(&repo_root);
    let canonical_repo = std::fs::canonicalize(&repo_root).expect("canonical repo");
    let db_path = temp.path().join("cold_term_wt.db");

    let term_id = {
        let engine = InProcessEngine::open_with_config(&db_path, canonical_repo.clone(), None)
            .expect("open 1");
        let term = engine
            .create_terminal(None, Some("Cold Term"), None, 24, 80)
            .await
            .expect("create_terminal");
        term.thread_id
    };

    let reopened = InProcessEngine::open_with_config(&db_path, canonical_repo.clone(), None)
        .expect("reopen");
    let wt_path = reopened
        .ensure_thread_worktree(&term_id)
        .await
        .expect("ensure_thread_worktree on cold terminal");
    let expected_wt = std::fs::canonicalize(
        canonical_repo
            .join(".grein")
            .join("worktrees")
            .join(&term_id),
    )
    .expect("canonical worktree");
    assert_eq!(wt_path, expected_wt.to_string_lossy());

    let _ = reopened
        .pty_snapshot(&term_id, 24, 140)
        .await
        .expect("lazy pty_snapshot");
    reopened
        .pty_write(
            &term_id,
            b"printf '\\nLAZY_WT_PWD=%s\\n' \"$(pwd)\"\n".to_vec(),
        )
        .await
        .expect("pty_write lazy");
    let expected_line = format!("LAZY_WT_PWD={wt_path}");
    wait_for_pty_snapshot(&reopened, &term_id, 24, 140, Duration::from_secs(5), |s| {
        s.plain_text().lines().any(|l| l.trim() == expected_line)
    })
    .await;
}

#[tokio::test]
async fn test_create_terminal_coalesced_events_exit_and_guardrails_without_acp() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sub_dir = temp.path().join("sub");
    std::fs::create_dir_all(&sub_dir).expect("mkdir sub");
    let canonical_sub = std::fs::canonicalize(&sub_dir).expect("canonical sub");
    let db_path = temp.path().join("session.db");
    let acp_marker = temp.path().join("acp_should_not_exist.flag");

    let acp_script = write_executable_script(
        temp.path(),
        "fail_if_called_acp.sh",
        &format!(
            "#!/bin/sh\ntouch {:?}\nexit 1\n",
            acp_marker.to_string_lossy()
        ),
    );
    let cfg = AcpBackendConfig {
        command: acp_script.to_string_lossy().into_owned(),
        args: Vec::new(),
        auth_method: None,
        cwd: temp.path().to_path_buf(),
    };

    let engine = InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
        .expect("open engine");
    let main = engine.ensure_main_thread().await.expect("main");
    let branch = engine
        .fork_thread_with_title(Some(&main.thread_id), Some("agent-branch"))
        .await
        .expect("fork branch");

    let mut rx = engine.subscribe_tui_events();
    let term = engine
        .create_terminal(Some(&main.thread_id), Some("Build Term"), Some("sub"), 24, 120)
        .await
        .expect("create_terminal");
    assert_eq!(term.title, "Build Term");
    assert!(!acp_marker.exists(), "create_terminal must never spawn ACP");

    // Root terminal creation + invalid cwd rejection.
    let root_term = engine
        .create_terminal(None, None, None, 24, 80)
        .await
        .expect("create root terminal");
    assert_eq!(root_term.title, "Terminal");
    let missing_cwd_err = engine
        .create_terminal(Some(&main.thread_id), None, Some("does-not-exist"), 24, 80)
        .await
        .expect_err("nonexistent cwd must fail");
    assert!(matches!(missing_cwd_err, EngineError::InvalidParams(_)));

    // Guardrails on Terminal thread without calling ACP.
    let send_err = engine
        .send_turn_for_thread(&term.thread_id, "run prompt")
        .await
        .expect_err("send_turn on terminal must fail");
    assert!(matches!(send_err, EngineError::InvalidParams(_)));

    let fork_err = engine
        .fork_thread_with_title(Some(&term.thread_id), Some("child"))
        .await
        .expect_err("fork on terminal must fail");
    assert!(matches!(fork_err, EngineError::InvalidParams(_)));

    let merge_err = engine
        .merge_thread(&term.thread_id)
        .await
        .expect_err("merge_thread on terminal must fail");
    assert!(matches!(merge_err, EngineError::InvalidParams(_)));

    let merge_summary_err = engine
        .merge_thread_with_summary(&term.thread_id, Some(&main.thread_id), Some("secret"))
        .await
        .expect_err("merge_thread_with_summary on terminal must fail");
    assert!(matches!(merge_summary_err, EngineError::InvalidParams(_)));

    let merge_into_term_err = engine
        .merge_thread_with_summary(&branch.thread_id, Some(&term.thread_id), Some("into term"))
        .await
        .expect_err("merging into terminal must fail");
    assert!(matches!(merge_into_term_err, EngineError::InvalidParams(_)));

    let opts = engine
        .agent_options(&term.thread_id)
        .await
        .expect("agent_options on terminal returns empty");
    assert!(opts.config_options.is_empty());
    assert!(opts.available_commands.is_empty());

    let set_cfg_err = engine
        .set_agent_config(&term.thread_id, "model", "fast", false)
        .await
        .expect_err("set_agent_config on terminal must fail");
    assert!(matches!(set_cfg_err, EngineError::InvalidParams(_)));

    let non_git_wt_err = engine
        .ensure_thread_worktree(&term.thread_id)
        .await
        .expect_err("ensure_thread_worktree outside git repo must fail");
    assert!(matches!(non_git_wt_err, EngineError::InvalidParams(_)));

    assert!(
        !acp_marker.exists(),
        "terminal operations and guardrails must never touch ACP"
    );
    assert!(
        engine
            .get_thread_transcript(&main.thread_id)
            .await
            .expect("main transcript")
            .is_empty(),
        "terminal state must never leak into parent transcript"
    );

    let meta = engine.thread_metadata().await.expect("meta");
    let term_meta = meta
        .iter()
        .find(|m| m.thread_id == term.thread_id)
        .expect("term meta");
    assert_eq!(term_meta.kind, ThreadKind::Terminal);
    assert_eq!(
        term_meta.cwd.as_deref(),
        Some(canonical_sub.to_string_lossy().as_ref())
    );

    // Burst output coalescing + PtyExited event.
    engine
        .pty_write(
            &term.thread_id,
            b"printf '\\n'; i=1; while [ \"$i\" -le 40 ]; do printf \"BURST_%s\\n\" \"$i\"; i=$((i+1)); done; exit 9\n".to_vec(),
        )
        .await
        .expect("pty_write burst and exit");

    let mut created_seen = false;
    let mut meta_seen = false;
    let mut screen_updates = 0usize;
    let mut saw_burst_40 = false;
    let mut exit_seen: Option<Option<i32>> = None;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_millis(300), rx.recv()).await {
            Ok(Some(ev)) => match ev {
                TuiEngineEvent::ThreadCreated { thread } if thread.id == term.thread_id => {
                    created_seen = true;
                }
                TuiEngineEvent::ThreadMetadataUpdated { meta }
                    if meta.iter().any(|m| {
                        m.thread_id == term.thread_id && m.kind == ThreadKind::Terminal
                    }) =>
                {
                    meta_seen = true;
                }
                TuiEngineEvent::PtyScreenUpdated {
                    thread_id,
                    snapshot,
                } if thread_id == term.thread_id => {
                    screen_updates += 1;
                    if snapshot.plain_text().lines().any(|l| l.trim() == "BURST_40") {
                        saw_burst_40 = true;
                    }
                }
                TuiEngineEvent::PtyExited {
                    thread_id,
                    exit_code,
                } if thread_id == term.thread_id => {
                    exit_seen = Some(exit_code);
                    if created_seen && meta_seen && saw_burst_40 {
                        break;
                    }
                }
                _ => {}
            },
            _ => {
                if exit_seen.is_some() {
                    break;
                }
            }
        }
    }

    assert!(created_seen, "expected ThreadCreated for terminal");
    assert!(meta_seen, "expected ThreadMetadataUpdated for terminal");
    assert!(saw_burst_40, "expected BURST_40 in PtyScreenUpdated");
    assert!(
        screen_updates < 40,
        "expected coalesced PtyScreenUpdated events (<40), got {screen_updates}"
    );
    assert_eq!(exit_seen, Some(Some(9)));
}

#[tokio::test]
async fn test_cold_reopen_lazy_respawn_and_delete_kills_pty_child() {
    let temp = tempfile::tempdir().expect("tempdir");
    let work_dir = temp.path().join("workspace");
    std::fs::create_dir_all(&work_dir).expect("mkdir work_dir");
    let canonical_work = std::fs::canonicalize(&work_dir).expect("canonical work_dir");
    let db_path = temp.path().join("reopen.db");

    let term_id = {
        let engine = InProcessEngine::open_with_config(&db_path, canonical_work.clone(), None)
            .expect("open 1");
        let _main = engine.ensure_main_thread().await.expect("main");
        let term = engine
            .create_terminal(None, Some("Persistent Term"), None, 24, 80)
            .await
            .expect("create term");
        term.thread_id
    };

    // Fresh runtime after cold reopen: PTY has not spawned yet until pty_snapshot is called.
    let reopened = InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
        .expect("reopen");
    let meta = reopened.thread_metadata().await.expect("meta");
    let term_meta = meta
        .iter()
        .find(|m| m.thread_id == term_id)
        .expect("persisted terminal");
    assert_eq!(term_meta.kind, ThreadKind::Terminal);
    assert_eq!(
        term_meta.cwd.as_deref(),
        Some(canonical_work.to_string_lossy().as_ref())
    );

    let lazy_snap = reopened
        .pty_snapshot(&term_id, 28, 95)
        .await
        .expect("lazy pty_snapshot");
    assert_eq!(lazy_snap.rows, 28);
    assert_eq!(lazy_snap.cols, 95);
    assert!(!lazy_snap.exited);

    reopened
        .pty_write(
            &term_id,
            b"printf '\\nLAZY_PWD=%s PID=%s\\n' \"$(pwd)\" \"$$\" && sleep 60\n".to_vec(),
        )
        .await
        .expect("pty_write");
    let expected_prefix = format!("LAZY_PWD={} PID=", canonical_work.to_string_lossy());
    let snap = wait_for_pty_snapshot(&reopened, &term_id, 28, 95, Duration::from_secs(5), |s| {
        s.plain_text()
            .lines()
            .any(|l| l.trim().starts_with(&expected_prefix))
    })
    .await;

    let pid_line = snap
        .plain_text()
        .lines()
        .find(|l| l.trim().starts_with(&expected_prefix))
        .expect("pid line")
        .trim()
        .to_string();
    let pid: u32 = pid_line
        .strip_prefix(&expected_prefix)
        .expect("strip prefix")
        .trim()
        .parse()
        .expect("parse pid");
    assert!(is_process_alive(pid), "PTY shell should be alive");

    // Deleting the terminal thread drops InteractivePtySession and kills its process group.
    reopened.delete_thread(&term_id).await.expect("delete term");

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if !is_process_alive(pid) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "PTY process {pid} still alive after delete_thread"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let missing_err = reopened
        .pty_snapshot(&term_id, 24, 80)
        .await
        .expect_err("deleted terminal snapshot must fail");
    assert!(matches!(missing_err, EngineError::NotFound(_)));
}

#[tokio::test]
async fn test_daemon_socket_pty_rpcs_do_not_stall_behind_in_flight_send_turn() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("daemon.db");
    let socket_path = temp.path().join("daemon.sock");

    let slow_script = write_executable_script(
        temp.path(),
        "slow_acp.sh",
        r#"#!/bin/sh
while IFS= read -r line; do
    rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
    case "$line" in
        *'"method":"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"slow","version":"1.0"},"authMethods":[],"agentCapabilities":{}}}\n' "$id"
            ;;
        *'"method":"session/new"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sess-slow"}}\n' "$id"
            ;;
        *'"method":"session/prompt"'*)
            sleep 1.4
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-slow","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"slow-done"}}}}\n'
            printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            ;;
    esac
done
"#,
    );
    let cfg = AcpBackendConfig {
        command: slow_script.to_string_lossy().into_owned(),
        args: Vec::new(),
        auth_method: None,
        cwd: temp.path().to_path_buf(),
    };
    let engine = Arc::new(
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
            .expect("open"),
    );

    let sock_clone = socket_path.clone();
    let engine_clone = Arc::clone(&engine);
    let server = tokio::spawn(async move {
        let _ = run_daemon_with_engine(sock_clone, engine_clone).await;
    });

    for _ in 0..50 {
        if socket_path.exists() && UnixStream::connect(&socket_path).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let client = Arc::new(
        UnixSocketEngineClient::connect(&socket_path)
            .await
            .expect("connect"),
    );
    let main = client.ensure_main_thread().await.expect("main");
    let term = client
        .create_terminal(Some(&main.thread_id), Some("Live Term"), None, 24, 80)
        .await
        .expect("create_terminal");

    let turn_client = Arc::clone(&client);
    let main_id = main.thread_id.clone();
    let turn_task =
        tokio::spawn(async move { turn_client.send_turn_for_thread(&main_id, "slow").await });

    // Give send_turn_for_thread time to acquire UnixSocketEngineClient's main io mutex and enter ACP sleep.
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(!turn_task.is_finished(), "send_turn must still be in flight");

    let pty_start = Instant::now();
    timeout(
        Duration::from_millis(400),
        client.pty_resize(&term.thread_id, 30, 100),
    )
    .await
    .expect("pty_resize must not stall behind send_turn")
    .expect("pty_resize ok");

    timeout(
        Duration::from_millis(400),
        client.pty_write(&term.thread_id, b"printf '\\nFAST_%s\\n' 'NONBLOCK'\n".to_vec()),
    )
    .await
    .expect("pty_write must not stall behind send_turn")
    .expect("pty_write ok");

    let snap = timeout(
        Duration::from_millis(400),
        client.pty_snapshot(&term.thread_id, 30, 100),
    )
    .await
    .expect("pty_snapshot must not stall behind send_turn")
    .expect("pty_snapshot ok");

    assert!(
        pty_start.elapsed() < Duration::from_millis(500),
        "PTY RPCs took too long while send_turn was active: {:?}",
        pty_start.elapsed()
    );
    assert_eq!(snap.rows, 30);
    assert_eq!(snap.cols, 100);
    assert!(!turn_task.is_finished(), "send_turn should still be running after PTY RPCs completed");

    wait_for_pty_snapshot(client.as_ref(), &term.thread_id, 30, 100, Duration::from_secs(5), |s| {
        s.plain_text().lines().any(|l| l.trim() == "FAST_NONBLOCK")
    })
    .await;

    turn_task
        .await
        .expect("join turn")
        .expect("slow turn finishes cleanly");

    server.abort();
}

#[tokio::test]
async fn test_cold_reopen_pending_pty_resize_before_pty_snapshot() {
    let temp = tempfile::tempdir().expect("tempdir");
    let work_dir = temp.path().join("workspace");
    std::fs::create_dir_all(&work_dir).expect("mkdir");
    let canonical_work = std::fs::canonicalize(&work_dir).expect("canonical");
    let db_path = temp.path().join("pending_resize.db");

    let term_id = {
        let engine = InProcessEngine::open_with_config(&db_path, canonical_work.clone(), None)
            .expect("open 1");
        let _main = engine.ensure_main_thread().await.expect("main");
        let term = engine
            .create_terminal(None, Some("Cold Resize Term"), None, 24, 80)
            .await
            .expect("create term");
        term.thread_id
    };

    let reopened = InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
        .expect("reopen");

    reopened
        .pty_resize(&term_id, 42, 132)
        .await
        .expect("pending pty_resize before snapshot");

    let snap = reopened
        .pty_snapshot(&term_id, 24, 80)
        .await
        .expect("lazy pty_snapshot");
    assert_eq!(
        (snap.rows, snap.cols),
        (42, 132),
        "cold-reopen lazy pty_snapshot must preserve pending pty_resize dimensions instead of 24x80"
    );

    reopened
        .pty_write(&term_id, b"printf '\\nCOLD_RESIZE_OK\\n'\n".to_vec())
        .await
        .expect("pty_write");
    wait_for_pty_snapshot(&reopened, &term_id, 0, 0, Duration::from_secs(5), |s| {
        s.rows == 42
            && s.cols == 132
            && s.plain_text().lines().any(|l| l.trim() == "COLD_RESIZE_OK")
    })
    .await;
}

#[tokio::test]
async fn test_daemon_socket_attach_rpcs_unblocked_during_streaming_turn() {
    use grein_cli::engine::PermissionDecisionPayload;

    let temp = tempfile::tempdir().expect("tempdir");
    let repo_root = temp.path().join("repo");
    std::fs::create_dir_all(&repo_root).expect("mkdir repo");
    init_test_git_repo(&repo_root);
    let canonical_repo = std::fs::canonicalize(&repo_root).expect("canonical repo");

    let db_path = temp.path().join("daemon_unblock.db");
    let socket_path = temp.path().join("daemon_unblock.sock");

    let acp_script = write_executable_script(
        temp.path(),
        "interactive_acp.sh",
        r#"#!/bin/sh
sess_counter=0
while IFS= read -r line; do
    rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
    case "$line" in
        *'"method":"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"interactive","version":"1.0"},"authMethods":[],"agentCapabilities":{}}}\n' "$id"
            ;;
        *'"method":"session/new"'*)
            sess_counter=$((sess_counter+1))
            printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sess-%s"}}\n' "$id" "$sess_counter"
            ;;
        *'"method":"session/prompt"'*)
            if printf '%s' "$line" | grep -q "wait-cancel"; then
                while IFS= read -r next_line; do
                    if printf '%s' "$next_line" | grep -q '"method":"session/cancel"'; then
                        printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"cancelled"}}\n' "$id"
                        break
                    fi
                done
            elif printf '%s' "$line" | grep -q "wait-perm"; then
                printf '{"jsonrpc":"2.0","id":"perm-rpc","method":"session/request_permission","params":{"sessionId":"sess-1","toolCall":{"toolCallId":"tc-rpc","name":"run_command","title":"Run"},"options":[{"optionId":"allow_once","name":"Allow","kind":"allow_once"},{"optionId":"reject_once","name":"Reject","kind":"reject_once"}]}}\n'
                IFS= read -r _perm_reply
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"perm-unblocked"}}}}\n'
                printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            else
                printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            fi
            ;;
    esac
done
"#,
    );
    let cfg = AcpBackendConfig {
        command: acp_script.to_string_lossy().into_owned(),
        args: Vec::new(),
        auth_method: None,
        cwd: canonical_repo.clone(),
    };
    let engine = Arc::new(
        InProcessEngine::open_with_config(&db_path, canonical_repo, Some(cfg)).expect("open"),
    );

    let sock_clone = socket_path.clone();
    let engine_clone = Arc::clone(&engine);
    let server = tokio::spawn(async move {
        let _ = run_daemon_with_engine(sock_clone, engine_clone).await;
    });

    for _ in 0..50 {
        if socket_path.exists() && UnixStream::connect(&socket_path).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let client = Arc::new(
        UnixSocketEngineClient::connect(&socket_path)
            .await
            .expect("connect"),
    );
    let mut rx = client
        .subscribe_events()
        .await
        .expect("subscribe")
        .expect("event receiver");

    let main = client.ensure_main_thread().await.expect("main");
    let wt_branch = client
        .fork_thread_with_title(Some(&main.thread_id), Some("wt-branch"))
        .await
        .expect("fork wt");
    let del_branch = client
        .fork_thread_with_title(Some(&main.thread_id), Some("del-branch"))
        .await
        .expect("fork del");

    // 1. In-flight turn waiting for cancel: thread_metadata, ensure_thread_worktree, delete_thread, and cancel_turn must not block.
    let turn_client = Arc::clone(&client);
    let main_id = main.thread_id.clone();
    let cancel_turn_task = tokio::spawn(async move {
        turn_client
            .send_turn_for_thread(&main_id, "wait-cancel")
            .await
    });

    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(
        !cancel_turn_task.is_finished(),
        "wait-cancel turn must still be in flight"
    );

    let meta = timeout(Duration::from_millis(500), client.thread_metadata())
        .await
        .expect("thread_metadata must not block behind send_turn")
        .expect("thread_metadata ok");
    assert_eq!(meta.len(), 3);

    let wt_path = timeout(
        Duration::from_millis(2000),
        client.ensure_thread_worktree(&wt_branch.thread_id),
    )
    .await
    .expect("ensure_thread_worktree must not block behind send_turn")
    .expect("ensure_thread_worktree ok");
    assert!(Path::new(&wt_path).join(".git").exists());

    let next_active = timeout(
        Duration::from_millis(500),
        client.delete_thread(&del_branch.thread_id),
    )
    .await
    .expect("delete_thread must not block behind send_turn")
    .expect("delete_thread ok");
    assert_eq!(next_active, main.thread_id);

    timeout(Duration::from_millis(500), client.cancel_turn(&main.thread_id))
        .await
        .expect("cancel_turn must not block behind send_turn")
        .expect("cancel_turn ok");

    timeout(Duration::from_secs(2), cancel_turn_task)
        .await
        .expect("cancelled turn should finish promptly")
        .expect("join")
        .expect("turn ok");

    // 2. In-flight turn waiting for permission: resolve_permission must not deadlock behind send_turn.
    let turn_client2 = Arc::clone(&client);
    let main_id2 = main.thread_id.clone();
    let perm_turn_task = tokio::spawn(async move {
        turn_client2
            .send_turn_for_thread(&main_id2, "wait-perm")
            .await
    });

    let perm_req_id = timeout(Duration::from_secs(2), async {
        loop {
            match rx.recv().await {
                Some(TuiEngineEvent::PermissionRequested { request_id, .. }) => {
                    return request_id;
                }
                Some(_) => continue,
                None => panic!("event stream closed before PermissionRequested"),
            }
        }
    })
    .await
    .expect("timed out waiting for PermissionRequested");

    timeout(
        Duration::from_millis(500),
        client.resolve_permission(PermissionDecisionPayload {
            request_id: perm_req_id,
            allow: true,
        }),
    )
    .await
    .expect("resolve_permission must not deadlock behind send_turn")
    .expect("resolve_permission ok");

    timeout(Duration::from_secs(2), perm_turn_task)
        .await
        .expect("permission turn should finish promptly")
        .expect("join")
        .expect("perm turn ok");

    server.abort();
}

#[tokio::test]
async fn test_pty_scroll_scrollback_navigation_and_events() {
    let temp = tempfile::tempdir().expect("tempdir");
    let work_dir = temp.path().join("scroll_ws");
    std::fs::create_dir_all(&work_dir).expect("mkdir");
    let canonical_work = std::fs::canonicalize(&work_dir).expect("canonical");
    let db_path = temp.path().join("scroll.db");

    let engine = InProcessEngine::open_with_config(&db_path, canonical_work, None).expect("open");
    let main = engine.ensure_main_thread().await.expect("main");
    let term = engine
        .create_terminal(Some(&main.thread_id), Some("Scroll Term"), None, 24, 80)
        .await
        .expect("create_terminal");

    let mut rx = engine.subscribe_tui_events();

    // Flood >40 lines into scrollback (leading newline for Nix sandbox bash-5.3 hermeticity).
    engine
        .pty_write(
            &term.thread_id,
            b"printf '\\n'; i=1; while [ \"$i\" -le 60 ]; do printf \"SCROLL_%02d\\n\" \"$i\"; i=$((i+1)); done\n".to_vec(),
        )
        .await
        .expect("pty_write flood");

    let bottom_snap = wait_for_pty_snapshot(&engine, &term.thread_id, 24, 80, Duration::from_secs(5), |s| {
        s.plain_text().lines().any(|l| l.trim() == "SCROLL_60")
    })
    .await;
    assert_eq!(bottom_snap.scrollback_offset, 0);
    let bottom_text = bottom_snap.plain_text();
    assert!(
        !bottom_text.lines().any(|l| l.trim() == "SCROLL_10"),
        "SCROLL_10 should have scrolled off screen at offset 0:\n{bottom_text}"
    );

    // Drain prior PTY output events before issuing pty_scroll.
    while rx.try_recv().is_ok() {}

    let up_snap = engine
        .pty_scroll(&term.thread_id, 10)
        .await
        .expect("pty_scroll up 10");
    assert_eq!(up_snap.scrollback_offset, 10);
    let up_text = up_snap.plain_text();
    assert_ne!(up_text, bottom_text, "scrolling up must expose earlier lines");
    let exposed_earlier = up_text
        .lines()
        .map(str::trim)
        .any(|line| line.starts_with("SCROLL_") && !bottom_text.lines().any(|b| b.trim() == line));
    assert!(
        exposed_earlier,
        "pty_scroll(+10) must expose earlier SCROLL_XX lines not visible at offset 0:\n{up_text}"
    );

    let mut saw_scroll_event = false;
    while let Ok(Some(ev)) = timeout(Duration::from_millis(250), rx.recv()).await {
        if let TuiEngineEvent::PtyScreenUpdated {
            thread_id,
            snapshot,
        } = ev
        {
            if thread_id == term.thread_id && snapshot.scrollback_offset == 10 {
                saw_scroll_event = true;
                break;
            }
        }
    }
    assert!(
        saw_scroll_event,
        "pty_scroll must emit TuiEngineEvent::PtyScreenUpdated with updated scrollback_offset"
    );

    let down_snap = engine
        .pty_scroll(&term.thread_id, -10)
        .await
        .expect("pty_scroll down 10");
    assert_eq!(down_snap.scrollback_offset, 0);
    assert!(
        down_snap
            .plain_text()
            .lines()
            .any(|l| l.trim() == "SCROLL_60"),
        "scrolling back -10 must restore bottom screen:\n{}",
        down_snap.plain_text()
    );

    // Guardrails: non-terminal and missing thread errors.
    let agent_err = engine
        .pty_scroll(&main.thread_id, 5)
        .await
        .expect_err("pty_scroll on agent thread must fail");
    assert!(matches!(agent_err, EngineError::InvalidParams(_)));

    let missing_err = engine
        .pty_scroll("00000000-0000-0000-0000-000000000000", 5)
        .await
        .expect_err("pty_scroll on missing thread must fail");
    assert!(matches!(missing_err, EngineError::NotFound(_)));
}

#[tokio::test]
async fn test_pty_restart_replaces_exited_pty_in_persisted_cwd_and_resets_state() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace_dir = temp.path().join("workspace");
    let term_dir = temp.path().join("term_cwd");
    std::fs::create_dir_all(&workspace_dir).expect("mkdir ws");
    std::fs::create_dir_all(&term_dir).expect("mkdir term_cwd");
    let canonical_term_dir = std::fs::canonicalize(&term_dir).expect("canonical term_dir");
    let db_path = temp.path().join("restart.db");

    let engine = InProcessEngine::open_with_config(&db_path, workspace_dir, None).expect("open");
    let main = engine.ensure_main_thread().await.expect("main");
    let term = engine
        .create_terminal(
            Some(&main.thread_id),
            Some("Restart Term"),
            Some(&canonical_term_dir.to_string_lossy()),
            24,
            80,
        )
        .await
        .expect("create_terminal");

    engine
        .pty_write(&term.thread_id, b"printf '\\nEXITING_NOW\\n'; exit 7\n".to_vec())
        .await
        .expect("pty_write exit 7");

    let exited_snap = wait_for_pty_snapshot(&engine, &term.thread_id, 24, 80, Duration::from_secs(5), |s| {
        s.exited && s.exit_code == Some(7)
    })
    .await;
    assert!(exited_snap.exited);
    assert_eq!(exited_snap.exit_code, Some(7));

    let mut rx = engine.subscribe_tui_events();
    let restarted = engine
        .pty_restart(&term.thread_id, 30, 110)
        .await
        .expect("pty_restart");
    assert!(!restarted.exited, "pty_restart must reset exited == false");
    assert_eq!(restarted.exit_code, None, "pty_restart must reset exit_code == None");
    assert_eq!((restarted.rows, restarted.cols), (30, 110));

    let mut saw_meta_updated = false;
    let mut saw_screen_updated = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline && !(saw_meta_updated && saw_screen_updated) {
        match timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Some(TuiEngineEvent::ThreadMetadataUpdated { meta })) => {
                if meta.iter().any(|m| {
                    m.thread_id == term.thread_id
                        && m.cwd.as_deref() == Some(canonical_term_dir.to_string_lossy().as_ref())
                }) {
                    saw_meta_updated = true;
                }
            }
            Ok(Some(TuiEngineEvent::PtyScreenUpdated {
                thread_id,
                snapshot,
            })) if thread_id == term.thread_id
                && !snapshot.exited
                && snapshot.exit_code.is_none()
                && snapshot.rows == 30
                && snapshot.cols == 110 =>
            {
                saw_screen_updated = true;
            }
            Ok(Some(_)) => {}
            _ => break,
        }
    }
    assert!(saw_meta_updated, "pty_restart must emit ThreadMetadataUpdated");
    assert!(saw_screen_updated, "pty_restart must emit PtyScreenUpdated");

    engine
        .pty_write(
            &term.thread_id,
            b"printf '\\nRESTARTED_PWD=%s\\n' \"$(pwd)\"\n".to_vec(),
        )
        .await
        .expect("pty_write after restart");
    let expected_pwd = format!("RESTARTED_PWD={}", canonical_term_dir.to_string_lossy());
    let live_snap = wait_for_pty_snapshot(&engine, &term.thread_id, 30, 110, Duration::from_secs(5), |s| {
        !s.exited
            && s.exit_code.is_none()
            && s.plain_text().lines().any(|l| l.trim() == expected_pwd)
    })
    .await;
    assert!(!live_snap.exited);
    assert_eq!(live_snap.exit_code, None);

    // Guardrails: non-terminal and missing thread errors.
    let agent_err = engine
        .pty_restart(&main.thread_id, 24, 80)
        .await
        .expect_err("pty_restart on agent thread must fail");
    assert!(matches!(agent_err, EngineError::InvalidParams(_)));

    let missing_err = engine
        .pty_restart("00000000-0000-0000-0000-000000000000", 24, 80)
        .await
        .expect_err("pty_restart on missing thread must fail");
    assert!(matches!(missing_err, EngineError::NotFound(_)));
}

#[tokio::test]
async fn test_osc7_live_cwd_persistence_cold_reopen_and_ignores_nonexistent() {
    let temp = tempfile::tempdir().expect("tempdir");
    let initial_dir = temp.path().join("initial");
    let sub_dir = initial_dir.join("sub_osc7");
    std::fs::create_dir_all(&sub_dir).expect("mkdir sub_dir");
    let canonical_initial = std::fs::canonicalize(&initial_dir).expect("canonical initial");
    let canonical_sub = std::fs::canonicalize(&sub_dir).expect("canonical sub");
    let canonical_sub_str = canonical_sub.to_string_lossy().into_owned();
    let db_path = temp.path().join("osc7.db");

    let term_id = {
        let engine = InProcessEngine::open_with_config(&db_path, canonical_initial.clone(), None)
            .expect("open");
        let main = engine.ensure_main_thread().await.expect("main");
        let term = engine
            .create_terminal(Some(&main.thread_id), Some("OSC7 Term"), None, 24, 140)
            .await
            .expect("create_terminal");

        let mut rx = engine.subscribe_tui_events();
        let cmd = format!(
            "printf '\\n'; printf '\\033]7;file://localhost%s\\007\\n' '{}'\n",
            canonical_sub_str
        );
        engine
            .pty_write(&term.thread_id, cmd.into_bytes())
            .await
            .expect("pty_write osc7");

        let mut saw_cwd_meta = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            match timeout(Duration::from_millis(250), rx.recv()).await {
                Ok(Some(TuiEngineEvent::ThreadMetadataUpdated { meta })) => {
                    if meta.iter().any(|m| {
                        m.thread_id == term.thread_id
                            && m.cwd.as_deref() == Some(canonical_sub_str.as_str())
                    }) {
                        saw_cwd_meta = true;
                        break;
                    }
                }
                Ok(Some(_)) => {}
                _ => {}
            }
        }
        assert!(
            saw_cwd_meta,
            "live OSC 7 cwd update must persist and emit ThreadMetadataUpdated with {canonical_sub_str}"
        );

        let meta_now = engine.thread_metadata().await.expect("thread_metadata");
        let term_meta = meta_now
            .iter()
            .find(|m| m.thread_id == term.thread_id)
            .expect("term meta");
        assert_eq!(term_meta.cwd.as_deref(), Some(canonical_sub_str.as_str()));

        // Writing normal output in same cwd must NOT trigger redundant ThreadMetadataUpdated loops.
        while rx.try_recv().is_ok() {}
        engine
            .pty_write(&term.thread_id, b"printf '\\nNO_REDUNDANT_META\\n'\n".to_vec())
            .await
            .expect("pty_write marker");
        wait_for_pty_snapshot(&engine, &term.thread_id, 24, 140, Duration::from_secs(5), |s| {
            s.plain_text()
                .lines()
                .any(|l| l.trim() == "NO_REDUNDANT_META")
        })
        .await;
        tokio::time::sleep(Duration::from_millis(60)).await;
        while let Ok(ev) = rx.try_recv() {
            assert!(
                !matches!(ev, TuiEngineEvent::ThreadMetadataUpdated { .. }),
                "unchanged OSC 7 cwd must not re-emit ThreadMetadataUpdated: {ev:?}"
            );
        }

        // Nonexistent OSC 7 directory must be ignored safely without overwriting persisted cwd.
        let missing_path = temp.path().join("does_not_exist_osc7");
        let bad_cmd = format!(
            "printf '\\n'; printf '\\033]7;file://localhost%s\\007\\n' '{}'; printf 'BAD_OSC7_DONE\\n'\n",
            missing_path.to_string_lossy()
        );
        engine
            .pty_write(&term.thread_id, bad_cmd.into_bytes())
            .await
            .expect("pty_write bad osc7");
        wait_for_pty_snapshot(&engine, &term.thread_id, 24, 140, Duration::from_secs(5), |s| {
            s.plain_text().lines().any(|l| l.trim() == "BAD_OSC7_DONE")
        })
        .await;
        tokio::time::sleep(Duration::from_millis(60)).await;

        let meta_after_bad = engine.thread_metadata().await.expect("meta after bad osc7");
        let term_meta_after_bad = meta_after_bad
            .iter()
            .find(|m| m.thread_id == term.thread_id)
            .expect("term meta");
        assert_eq!(
            term_meta_after_bad.cwd.as_deref(),
            Some(canonical_sub_str.as_str()),
            "nonexistent OSC 7 directory must be ignored safely"
        );
        while let Ok(ev) = rx.try_recv() {
            assert!(
                !matches!(ev, TuiEngineEvent::ThreadMetadataUpdated { .. }),
                "nonexistent OSC 7 cwd must not emit ThreadMetadataUpdated: {ev:?}"
            );
        }

        term.thread_id
    };

    // Cold reopen: pty_snapshot lazily respawns PTY inside persisted $SUBDIR.
    let reopened = InProcessEngine::open_with_config(&db_path, canonical_initial, None)
        .expect("cold reopen");
    let _ = reopened
        .pty_snapshot(&term_id, 24, 140)
        .await
        .expect("lazy pty_snapshot");
    reopened
        .pty_write(
            &term_id,
            b"printf '\\nREOPEN_OSC7_PWD=%s\\n' \"$(pwd)\"\n".to_vec(),
        )
        .await
        .expect("pty_write after reopen");

    let expected_pwd = format!("REOPEN_OSC7_PWD={canonical_sub_str}");
    wait_for_pty_snapshot(&reopened, &term_id, 24, 140, Duration::from_secs(5), |s| {
        s.plain_text().lines().any(|l| l.trim() == expected_pwd)
    })
    .await;
}

#[tokio::test]
async fn test_worktree_git_status_metadata_clean_and_dirty() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo_root = temp.path().join("repo");
    std::fs::create_dir_all(&repo_root).expect("mkdir repo");
    init_test_git_repo(&repo_root);
    let canonical_repo = std::fs::canonicalize(&repo_root).expect("canonical repo");
    let db_path = temp.path().join("wt_git.db");

    let engine = InProcessEngine::open_with_config(&db_path, canonical_repo.clone(), None)
        .expect("open engine");
    let main = engine.ensure_main_thread().await.expect("main");
    let branch = engine
        .fork_thread_with_title(Some(&main.thread_id), Some("wt-status"))
        .await
        .expect("fork branch");
    let wt_path = engine
        .ensure_thread_worktree(&branch.thread_id)
        .await
        .expect("ensure_thread_worktree");
    let wt_term = engine
        .create_terminal(Some(&branch.thread_id), Some("WT Term"), None, 24, 80)
        .await
        .expect("create_terminal");

    let clean_meta = engine.thread_metadata().await.expect("clean thread_metadata");
    let main_clean = clean_meta
        .iter()
        .find(|m| m.thread_id == main.thread_id)
        .expect("main meta");
    let branch_clean = clean_meta
        .iter()
        .find(|m| m.thread_id == branch.thread_id)
        .expect("branch meta");
    let term_clean = clean_meta
        .iter()
        .find(|m| m.thread_id == wt_term.thread_id)
        .expect("wt term meta");

    let expected_wt_branch = format!("worktree-{}", branch.thread_id);
    assert!(
        main_clean.git_branch.is_some(),
        "main thread on git repo must populate git_branch"
    );
    assert_eq!(main_clean.git_dirty, 0);
    assert_eq!(branch_clean.git_branch.as_deref(), Some(expected_wt_branch.as_str()));
    assert_eq!(branch_clean.git_dirty, 0);
    assert_eq!(term_clean.git_branch.as_deref(), Some(expected_wt_branch.as_str()));
    assert_eq!(term_clean.git_dirty, 0);

    // Dirty the isolated worktree without dirtying the primary repo root.
    std::fs::write(Path::new(&wt_path).join("dirty_file.txt"), "uncommitted\n")
        .expect("write dirty file");

    let dirty_meta = engine.thread_metadata().await.expect("dirty thread_metadata");
    let main_dirty = dirty_meta
        .iter()
        .find(|m| m.thread_id == main.thread_id)
        .expect("main meta");
    let branch_dirty = dirty_meta
        .iter()
        .find(|m| m.thread_id == branch.thread_id)
        .expect("branch meta");
    let term_dirty = dirty_meta
        .iter()
        .find(|m| m.thread_id == wt_term.thread_id)
        .expect("wt term meta");

    assert_eq!(main_dirty.git_dirty, 0, "primary repo root must stay clean");
    assert_eq!(branch_dirty.git_branch.as_deref(), Some(expected_wt_branch.as_str()));
    assert_eq!(branch_dirty.git_dirty, 1);
    assert_eq!(term_dirty.git_branch.as_deref(), Some(expected_wt_branch.as_str()));
    assert_eq!(term_dirty.git_dirty, 1);
}
