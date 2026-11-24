use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

use crate::engine::{dispatch_json_rpc, GreinEngine, InProcessEngine};

const STATE_DIR_NAME: &str = ".grein";

/// Resolves `<git_root>/.grein` inside a Git repository, `<cwd>/.grein` otherwise.
pub fn resolve_state_dir(cwd: &Path) -> PathBuf {
    let root = cwd
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .unwrap_or(cwd);
    root.join(STATE_DIR_NAME)
}

fn current_state_dir() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    resolve_state_dir(&cwd)
}

pub fn default_db_path() -> PathBuf {
    current_state_dir().join("session.db")
}

pub fn default_socket_path() -> PathBuf {
    current_state_dir().join("daemon.sock")
}

pub async fn run_daemon_with_socket(
    socket_path: PathBuf,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let db_path = socket_path
        .parent()
        .map(|p| p.join("session.db"))
        .unwrap_or_else(|| PathBuf::from("session.db"));
    run_daemon_with_socket_and_db(socket_path, db_path).await
}

async fn handle_legacy(engine: &dyn GreinEngine, line: &str) -> String {
    if line == "ping" {
        return "pong\n".to_string();
    }
    if let Some(prompt) = line.strip_prefix("send_turn:") {
        return match engine.send_turn(prompt).await {
            Ok(result) => format!("ok:{}\n", result.node_id),
            Err(e) => format!("error:{e}\n"),
        };
    }
    if line == "list_entries" {
        return match engine.snapshot().await {
            Ok(snapshot) => format!("entries:{}\n", snapshot.node_count),
            Err(e) => format!("error:{e}\n"),
        };
    }
    "ok\n".to_string()
}

fn parse_subscribe_events_id(line: &str) -> Option<Value> {
    let req: Value = serde_json::from_str(line).ok()?;
    let method = req.get("method").and_then(Value::as_str)?;
    if method == "session.subscribe_events" {
        Some(req.get("id").cloned().unwrap_or(Value::Null))
    } else {
        None
    }
}

pub async fn run_daemon_with_socket_and_db(
    socket_path: PathBuf,
    db_path: PathBuf,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let engine = Arc::new(InProcessEngine::open(&db_path)?);
    run_daemon_with_engine(socket_path, engine).await
}

pub async fn run_daemon_with_engine(
    socket_path: PathBuf,
    engine: Arc<dyn GreinEngine>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }
    if let Some(parent) = socket_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let listener = UnixListener::bind(&socket_path)?;

    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, _addr)) => {
                        let engine = Arc::clone(&engine);
                        tokio::spawn(async move {
                            let (reader, mut writer) = stream.into_split();
                            let mut lines = BufReader::new(reader).lines();
                            while let Ok(Some(line)) = lines.next_line().await {
                                let trimmed = line.trim();
                                if trimmed.is_empty() {
                                    continue;
                                }
                                if trimmed.starts_with('{') {
                                    if let Some(id) = parse_subscribe_events_id(trimmed) {
                                        let ack = json!({
                                            "jsonrpc": "2.0",
                                            "id": id,
                                            "result": {"subscribed": true}
                                        });
                                        if writer.write_all(format!("{ack}\n").as_bytes()).await.is_err()
                                            || writer.flush().await.is_err()
                                        {
                                            break;
                                        }
                                        let Ok(Some(mut rx)) = engine.subscribe_events().await else {
                                            break;
                                        };
                                        while let Some(ev) = rx.recv().await {
                                            let Ok(encoded) = serde_json::to_string(&ev) else {
                                                continue;
                                            };
                                            if writer.write_all(format!("{encoded}\n").as_bytes()).await.is_err()
                                                || writer.flush().await.is_err()
                                            {
                                                break;
                                            }
                                        }
                                        break;
                                    }
                                    let response = format!("{}\n", dispatch_json_rpc(engine.as_ref(), trimmed).await);
                                    if writer.write_all(response.as_bytes()).await.is_err()
                                        || writer.flush().await.is_err()
                                    {
                                        break;
                                    }
                                } else {
                                    let response = handle_legacy(engine.as_ref(), trimmed).await;
                                    if writer.write_all(response.as_bytes()).await.is_err()
                                        || writer.flush().await.is_err()
                                    {
                                        break;
                                    }
                                }
                            }
                        });
                    }
                    Err(e) => {
                        eprintln!("Socket error: {e}");
                        break;
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                break;
            }
        }
    }

    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::connect_to_daemon;
    use crate::engine::UnixSocketEngineClient;
    use grein_core::port::dto::TuiEngineEvent;
    use std::time::Duration;
    use tokio::net::UnixStream;
    use tokio::time::timeout;

    #[tokio::test]
    async fn test_daemon_socket_rpc_methods_and_live_event_subscription() {
        let temp = tempfile::tempdir().expect("tempdir");
        let socket_path = temp.path().join("daemon.sock");
        let db_path = temp.path().join("session.db");

        let sock_clone = socket_path.clone();
        let db_clone = db_path.clone();
        let server = tokio::spawn(async move {
            let _ = run_daemon_with_socket_and_db(sock_clone, db_clone).await;
        });

        for _ in 0..50 {
            if socket_path.exists() && UnixStream::connect(&socket_path).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let client = UnixSocketEngineClient::connect(&socket_path)
            .await
            .expect("connect UnixSocketEngineClient");
        let mut rx = client
            .subscribe_events()
            .await
            .expect("subscribe")
            .expect("live socket event receiver");

        let main = client.ensure_main_thread().await.expect("ensure main");
        assert_eq!(main.title, "Main");

        let forked = client
            .fork_thread_with_title(Some(&main.thread_id), Some("socket-branch"))
            .await
            .expect("fork");
        assert_eq!(forked.custom_name.as_deref(), Some("socket-branch"));
        assert_eq!(forked.title, "Branch 2");

        let shell_res = client
            .run_shell_command(&main.thread_id, "printf 'via-socket'")
            .await
            .expect("shell");
        assert_eq!(shell_res.stdout, "via-socket");

        let mut seen = Vec::new();
        for _ in 0..8 {
            if let Ok(Some(ev)) = timeout(Duration::from_millis(300), rx.recv()).await {
                let done = matches!(ev, TuiEngineEvent::ShellOutput { .. });
                seen.push(ev);
                if done {
                    break;
                }
            } else {
                break;
            }
        }
        assert!(seen.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ThreadCreated { thread } if thread.id == forked.thread_id
        )));
        assert!(seen.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ThreadMetadataUpdated { meta } if meta.len() == 2
        )));
        assert!(seen.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ShellOutput { stdout, .. } if stdout == "via-socket"
        )));

        let socket_meta = client.thread_metadata().await.expect("thread_metadata");
        assert_eq!(socket_meta.len(), 2);
        assert_eq!(
            socket_meta[1].parent_thread_id.as_deref(),
            Some(main.thread_id.as_str())
        );

        let mut raw_client = connect_to_daemon(&socket_path)
            .await
            .expect("connect DaemonClient");
        let meta_val = raw_client
            .session_thread_metadata()
            .await
            .expect("session_thread_metadata");
        assert_eq!(meta_val["result"].as_array().unwrap().len(), 2);
        let threads_val = raw_client
            .session_list_threads()
            .await
            .expect("session_list_threads");
        assert_eq!(threads_val["result"].as_array().unwrap().len(), 2);
        let bb_removed = raw_client
            .json_rpc("blackboard.list", json!({}))
            .await
            .expect("blackboard.list rpc");
        assert_eq!(bb_removed["error"]["code"], -32601);
        let root_join_err = raw_client
            .session_merge_thread(&main.thread_id)
            .await
            .expect("root join rpc");
        assert_eq!(root_join_err["error"]["code"], -32602);
        let merged_val = raw_client
            .session_merge_thread(&forked.thread_id)
            .await
            .expect("session_merge_thread");
        assert_eq!(merged_val["result"]["merged"], true);

        let deleted_val = raw_client
            .session_delete_thread(&forked.thread_id)
            .await
            .expect("session_delete_thread");
        assert_eq!(
            deleted_val["result"]["next_active_thread_id"],
            main.thread_id
        );
        assert_eq!(client.list_threads().await.expect("after delete").len(), 1);

        let root2_val = raw_client
            .session_create_root_thread(Some("Second Root"))
            .await
            .expect("session_create_root_thread");
        let root2_id = root2_val["result"]["thread_id"]
            .as_str()
            .expect("root2 id")
            .to_string();
        assert_eq!(root2_val["result"]["custom_name"], "Second Root");
        assert_eq!(root2_val["result"]["title"], "Root 2");

        let cleared_val = raw_client
            .session_rename_thread(&root2_id, "")
            .await
            .expect("clear session_rename_thread");
        assert_eq!(cleared_val["result"]["custom_name"], Value::Null);
        assert_eq!(cleared_val["result"]["title"], "Root 2");

        let renamed_val = raw_client
            .session_rename_thread(&root2_id, "Renamed Root")
            .await
            .expect("session_rename_thread");
        assert_eq!(renamed_val["result"]["custom_name"], "Renamed Root");
        assert_eq!(renamed_val["result"]["title"], "Root 2");

        let forked2 = client
            .fork_thread_with_title(Some(&root2_id), Some("Socket Join Branch"))
            .await
            .expect("fork2");
        let joined_val = raw_client
            .session_merge_thread_with_summary(
                &forked2.thread_id,
                Some(&main.thread_id),
                Some("Decided over JSON-RPC"),
            )
            .await
            .expect("session_merge_thread_with_summary");
        assert_eq!(joined_val["result"]["merged"], true);

        let mut post_events = Vec::new();
        while let Ok(Some(ev)) = timeout(Duration::from_millis(150), rx.recv()).await {
            post_events.push(ev);
        }
        assert!(post_events.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ThreadCustomNameUpdated { thread_id, custom_name }
                if thread_id == &root2_id && custom_name.is_none()
        )));
        assert!(post_events.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ThreadCustomNameUpdated { thread_id, custom_name }
                if thread_id == &root2_id && custom_name.as_deref() == Some("Renamed Root")
        )));
        assert!(post_events.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ThreadMerged { thread_id, target_thread_id, summary }
                if thread_id == &forked2.thread_id
                    && target_thread_id == &main.thread_id
                    && summary == "Decided over JSON-RPC"
        )));

        let term_rpc = raw_client
            .session_create_terminal(Some(&main.thread_id), Some("Daemon Term"), None, 24, 90)
            .await
            .expect("session_create_terminal");
        let term_id = term_rpc["result"]["thread_id"]
            .as_str()
            .expect("term_id")
            .to_string();
        assert_eq!(term_rpc["result"]["custom_name"], "Daemon Term");
        assert_eq!(term_rpc["result"]["title"], "Terminal");

        let resize_rpc = raw_client
            .session_pty_resize(&term_id, 26, 96)
            .await
            .expect("session_pty_resize");
        assert_eq!(resize_rpc["result"]["ok"], true);

        let write_rpc = raw_client
            .session_pty_write(&term_id, b"printf '\\nRPC_%s\\n' 'TERM_OK'\n")
            .await
            .expect("session_pty_write");
        assert_eq!(write_rpc["result"]["ok"], true);

        let mut saw_rpc_term_ok = false;
        let mut last_screen = String::new();
        for _ in 0..150 {
            let snap_rpc = raw_client
                .session_pty_snapshot(&term_id, 26, 96)
                .await
                .expect("session_pty_snapshot");
            let snap: grein_core::port::dto::PtyScreenSnapshot =
                serde_json::from_value(snap_rpc["result"].clone()).expect("decode PtyScreenSnapshot");
            assert_eq!(snap.rows, 26);
            assert_eq!(snap.cols, 96);
            last_screen = snap.plain_text();
            if last_screen.lines().any(|l| l.trim() == "RPC_TERM_OK") {
                saw_rpc_term_ok = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        assert!(
            saw_rpc_term_ok,
            "DaemonClient PTY RPCs must drive live PTY; last screen:\n{last_screen}"
        );

        let wt_err_rpc = raw_client
            .session_ensure_thread_worktree("nonexistent-thread-id")
            .await
            .expect("session_ensure_thread_worktree on unknown thread");
        assert_eq!(wt_err_rpc["error"]["code"], -32004);

        let wt_missing_rpc = raw_client
            .json_rpc("session.ensure_thread_worktree", json!({}))
            .await
            .expect("session_ensure_thread_worktree without thread_id");
        assert_eq!(wt_missing_rpc["error"]["code"], -32602);

        server.abort();
    }

    #[tokio::test]
    async fn test_daemon_socket_pty_scroll_and_restart_round_trip() {
        let temp = tempfile::tempdir().expect("tempdir");
        let work_dir = temp.path().join("daemon_ws");
        std::fs::create_dir_all(&work_dir).expect("mkdir");
        let canonical_work = std::fs::canonicalize(&work_dir).expect("canonical");
        let socket_path = temp.path().join("pty_rpc.sock");
        let db_path = temp.path().join("pty_rpc.db");

        let sock_clone = socket_path.clone();
        let db_clone = db_path.clone();
        let server = tokio::spawn(async move {
            let _ = run_daemon_with_socket_and_db(sock_clone, db_clone).await;
        });

        for _ in 0..50 {
            if socket_path.exists() && UnixStream::connect(&socket_path).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let client = UnixSocketEngineClient::connect(&socket_path)
            .await
            .expect("connect UnixSocketEngineClient");
        let mut raw_client = connect_to_daemon(&socket_path)
            .await
            .expect("connect DaemonClient");

        let main = client.ensure_main_thread().await.expect("ensure main");
        let term = client
            .create_terminal(
                Some(&main.thread_id),
                Some("RPC Scroll Term"),
                Some(&canonical_work.to_string_lossy()),
                24,
                80,
            )
            .await
            .expect("create_terminal");

        client
            .pty_write(
                &term.thread_id,
                b"printf '\\n'; i=1; while [ \"$i\" -le 50 ]; do printf \"DAEMON_L_%02d\\n\" \"$i\"; i=$((i+1)); done; printf \"DAEMON_%s\" \"READY\"; read -r grein_pause; exit 7\n".to_vec(),
            )
            .await
            .expect("pty_write");

        let mut saw_50 = false;
        for _ in 0..150 {
            let snap = client
                .pty_snapshot(&term.thread_id, 24, 80)
                .await
                .expect("pty_snapshot");
            let text = snap.plain_text();
            if text.lines().any(|l| l.trim() == "DAEMON_L_50")
                && text.lines().any(|l| l.trim() == "DAEMON_READY")
            {
                saw_50 = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(saw_50, "expected DAEMON_L_50 in daemon PTY snapshot");

        let scroll_rpc = raw_client
            .session_pty_scroll(&term.thread_id, 10)
            .await
            .expect("session_pty_scroll rpc");
        let scroll_snap: grein_core::port::dto::PtyScreenSnapshot =
            serde_json::from_value(scroll_rpc["result"].clone()).expect("decode scroll snapshot");
        assert_eq!(scroll_snap.scrollback_offset, 10);

        let scroll_back = client
            .pty_scroll(&term.thread_id, -10)
            .await
            .expect("UnixSocketEngineClient::pty_scroll -10");
        assert_eq!(scroll_back.scrollback_offset, 0);

        client
            .pty_write(&term.thread_id, b"\n".to_vec())
            .await
            .expect("pty_write exit 7");

        let mut saw_exited = false;
        for _ in 0..150 {
            let snap = client
                .pty_snapshot(&term.thread_id, 24, 80)
                .await
                .expect("pty_snapshot exited");
            if snap.exited && snap.exit_code == Some(7) {
                saw_exited = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(saw_exited, "expected exited == true, exit_code == Some(7)");

        let restart_rpc = raw_client
            .session_pty_restart(&term.thread_id, 28, 100)
            .await
            .expect("session_pty_restart rpc");
        let restart_snap: grein_core::port::dto::PtyScreenSnapshot =
            serde_json::from_value(restart_rpc["result"].clone()).expect("decode restart snapshot");
        assert!(!restart_snap.exited);
        assert_eq!(restart_snap.exit_code, None);
        assert_eq!((restart_snap.rows, restart_snap.cols), (28, 100));

        let restart_via_client = client
            .pty_restart(&term.thread_id, 30, 240)
            .await
            .expect("UnixSocketEngineClient::pty_restart");
        assert!(!restart_via_client.exited);
        assert_eq!(restart_via_client.exit_code, None);
        assert_eq!((restart_via_client.rows, restart_via_client.cols), (30, 240));

        client
            .pty_write(
                &term.thread_id,
                b"printf '\\nDAEMON_RESTART_PWD=%s\\n' \"$(pwd)\"\n".to_vec(),
            )
            .await
            .expect("pty_write after restart");
        let expected_pwd = format!("DAEMON_RESTART_PWD={}", canonical_work.to_string_lossy());
        let mut saw_pwd = false;
        for _ in 0..150 {
            let snap = client
                .pty_snapshot(&term.thread_id, 30, 240)
                .await
                .expect("pty_snapshot after restart");
            if !snap.exited && snap.plain_text().lines().any(|l| l.trim() == expected_pwd) {
                saw_pwd = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(saw_pwd, "restarted daemon PTY must execute commands in persisted cwd");

        server.abort();
    }

    #[tokio::test]
    async fn test_daemon_socket_worktree_rpc_round_trip() {
        use crate::engine::InProcessEngine;

        let temp = tempfile::tempdir().expect("tempdir");
        let repo_dir = temp.path().join("repo");
        std::fs::create_dir_all(&repo_dir).expect("mkdir repo");
        for args in [
            vec!["init"],
            vec!["config", "user.name", "Grein Test"],
            vec!["config", "user.email", "test@grein.dev"],
        ] {
            let status = std::process::Command::new("git")
                .args(&args)
                .current_dir(&repo_dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .status()
                .expect("run git setup");
            assert!(status.success(), "git setup failed: {args:?}");
        }
        std::fs::write(repo_dir.join("README.md"), "base line\n").expect("write README");
        for args in [vec!["add", "."], vec!["commit", "-m", "Initial commit"]] {
            let status = std::process::Command::new("git")
                .args(&args)
                .current_dir(&repo_dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .status()
                .expect("run git commit");
            assert!(status.success(), "git commit failed: {args:?}");
        }
        let canonical_repo = std::fs::canonicalize(&repo_dir).expect("canonical repo");
        let canonical_repo_str = canonical_repo.to_string_lossy().to_string();

        let socket_path = temp.path().join("worktree_rpc.sock");
        let db_path = temp.path().join("worktree_rpc.db");
        let engine = std::sync::Arc::new(
            InProcessEngine::open_with_config(&db_path, canonical_repo.clone(), None)
                .expect("open engine"),
        );

        let sock_clone = socket_path.clone();
        let engine_clone = std::sync::Arc::clone(&engine);
        let server = tokio::spawn(async move {
            let _ = run_daemon_with_engine(sock_clone, engine_clone).await;
        });

        for _ in 0..50 {
            if socket_path.exists() && UnixStream::connect(&socket_path).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let client = UnixSocketEngineClient::connect(&socket_path)
            .await
            .expect("connect UnixSocketEngineClient");
        let mut raw_client = connect_to_daemon(&socket_path)
            .await
            .expect("connect DaemonClient");

        let main = client.ensure_main_thread().await.expect("ensure main");
        let branch = client
            .fork_thread_with_title(Some(&main.thread_id), Some("feat/socket-wt"))
            .await
            .expect("fork branch");
        let wt_path = client
            .ensure_thread_worktree(&branch.thread_id)
            .await
            .expect("ensure_thread_worktree");
        assert!(std::path::Path::new(&wt_path).join(".git").exists());

        let worktrees = client.list_worktrees().await.expect("list_worktrees");
        assert!(worktrees.len() >= 2);
        let primary = worktrees
            .iter()
            .find(|w| w.path == canonical_repo_str)
            .expect("primary entry");
        assert!(primary.is_primary);
        assert!(primary.attached_thread_ids.contains(&main.thread_id));
        assert!(!primary.attached_thread_ids.contains(&branch.thread_id));

        let wt_entry = worktrees
            .iter()
            .find(|w| w.path == wt_path)
            .expect("worktree entry");
        assert!(wt_entry.is_grein_managed);
        assert_eq!(wt_entry.attached_thread_ids, vec![branch.thread_id.clone()]);

        let raw_list = raw_client
            .session_list_worktrees()
            .await
            .expect("session_list_worktrees rpc");
        assert_eq!(
            raw_list["result"].as_array().expect("result array").len(),
            worktrees.len()
        );

        std::fs::write(
            std::path::Path::new(&wt_path).join("README.md"),
            "modified via socket\n",
        )
        .expect("modify README");
        let diff_client = client
            .worktree_diff_preview(&wt_path)
            .await
            .expect("worktree_diff_preview");
        assert!(
            diff_client.contains("modified via socket"),
            "unexpected diff preview: {diff_client}"
        );
        let diff_rpc = raw_client
            .session_worktree_diff_preview(&wt_path)
            .await
            .expect("session_worktree_diff_preview rpc");
        assert!(diff_rpc["result"]["diff"]
            .as_str()
            .expect("diff string")
            .contains("modified via socket"));

        let switched_rpc = raw_client
            .session_switch_thread_worktree(&branch.thread_id, &canonical_repo_str)
            .await
            .expect("session_switch_thread_worktree rpc");
        assert_eq!(switched_rpc["result"]["cwd"], canonical_repo_str);

        let switched_back = client
            .switch_thread_worktree(&branch.thread_id, &wt_path)
            .await
            .expect("switch_thread_worktree back");
        assert_eq!(switched_back, wt_path);

        let dirty_remove_rpc = raw_client
            .session_remove_worktree(&wt_path, false)
            .await
            .expect("dirty session_remove_worktree rpc");
        assert_eq!(dirty_remove_rpc["error"]["code"], -32602);
        assert!(std::path::Path::new(&wt_path).exists());

        client
            .remove_worktree(&wt_path, true)
            .await
            .expect("force remove_worktree");
        assert!(!std::path::Path::new(&wt_path).exists());

        let after_remove = client.list_worktrees().await.expect("list after remove");
        assert!(!after_remove.iter().any(|w| w.path == wt_path));
        let primary_after = after_remove
            .iter()
            .find(|w| w.path == canonical_repo_str)
            .expect("primary after");
        assert!(primary_after.attached_thread_ids.contains(&branch.thread_id));

        let prune_rpc = raw_client
            .session_prune_worktrees()
            .await
            .expect("session_prune_worktrees rpc");
        assert!(prune_rpc["result"]["summary"].is_string());
        let _ = client.prune_worktrees().await.expect("prune_worktrees");

        server.abort();
    }

    #[tokio::test]
    async fn test_daemon_socket_rename_thread_dedicated_and_blank_clear() {
        use crate::engine::{AcpBackendConfig, InProcessEngine};
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let script_path = temp.path().join("slow_acp.sh");
        let script = r#"#!/bin/sh
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
            sleep 0.4
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-slow","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"slow reply"}}}}\n'
            printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            ;;
    esac
done
"#;
        std::fs::write(&script_path, script).expect("write slow script");
        let mut perms = std::fs::metadata(&script_path).expect("meta").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).expect("chmod");

        let socket_path = temp.path().join("dedicated_rename.sock");
        let db_path = temp.path().join("dedicated_rename.db");
        let cfg = AcpBackendConfig {
            command: script_path.to_string_lossy().into_owned(),
            args: Vec::new(),
            auth_method: None,
            cwd: temp.path().to_path_buf(),
        };
        let engine = Arc::new(
            InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
                .expect("open engine"),
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
            .expect("receiver");

        let main = client.ensure_main_thread().await.expect("ensure main");

        // Hold the shared socket `io` lock with an in-flight slow turn.
        let turn_client = Arc::clone(&client);
        let turn_tid = main.thread_id.clone();
        let turn_handle = tokio::spawn(async move {
            turn_client
                .send_turn_for_thread(&turn_tid, "slow prompt")
                .await
        });
        tokio::time::sleep(Duration::from_millis(40)).await;

        // `rename_thread` must bypass `self.io` via `call_dedicated_typed` and finish promptly.
        let renamed = timeout(
            Duration::from_millis(200),
            client.rename_thread(&main.thread_id, "Terminal"),
        )
        .await
        .expect("dedicated rename must not block behind in-flight turn")
        .expect("rename_thread");
        assert_eq!(renamed.custom_name.as_deref(), Some("Terminal"));
        assert_eq!(renamed.title, "Main");

        turn_handle
            .await
            .expect("turn join")
            .expect("slow turn completes");

        let cleared = client
            .rename_thread(&main.thread_id, "")
            .await
            .expect("blank rename clears custom_name");
        assert_eq!(cleared.custom_name, None);
        assert_eq!(cleared.title, "Main");

        let mut events = Vec::new();
        while let Ok(Some(ev)) = timeout(Duration::from_millis(150), rx.recv()).await {
            events.push(ev);
        }
        assert!(events.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ThreadCustomNameUpdated { thread_id, custom_name }
                if thread_id == &main.thread_id && custom_name.as_deref() == Some("Terminal")
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ThreadCustomNameUpdated { thread_id, custom_name }
                if thread_id == &main.thread_id && custom_name.is_none()
        )));

        server.abort();
    }
}
