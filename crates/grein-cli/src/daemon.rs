use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

use crate::engine::{dispatch_json_rpc, DaemonEngine, InProcessEngine};

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

async fn handle_legacy(engine: &InProcessEngine, line: &str) -> String {
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
    engine: Arc<InProcessEngine>,
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
                                        let mut rx = engine.subscribe_tui_events();
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
    use grein_tui::engine_port::TuiEngineEvent;
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
        assert_eq!(forked.title, "socket-branch");

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
        assert_eq!(root2_val["result"]["title"], "Second Root");

        let renamed_val = raw_client
            .session_rename_thread(&root2_id, "Renamed Root")
            .await
            .expect("session_rename_thread");
        assert_eq!(renamed_val["result"]["title"], "Renamed Root");

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
            TuiEngineEvent::ThreadTitleUpdated { thread_id, title }
                if thread_id == &root2_id && title == "Renamed Root"
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
        assert_eq!(term_rpc["result"]["title"], "Daemon Term");

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
            let snap: grein_tui::engine_port::PtyScreenSnapshot =
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
        let scroll_snap: grein_tui::engine_port::PtyScreenSnapshot =
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
        let restart_snap: grein_tui::engine_port::PtyScreenSnapshot =
            serde_json::from_value(restart_rpc["result"].clone()).expect("decode restart snapshot");
        assert!(!restart_snap.exited);
        assert_eq!(restart_snap.exit_code, None);
        assert_eq!((restart_snap.rows, restart_snap.cols), (28, 100));

        let restart_via_client = client
            .pty_restart(&term.thread_id, 30, 105)
            .await
            .expect("UnixSocketEngineClient::pty_restart");
        assert!(!restart_via_client.exited);
        assert_eq!(restart_via_client.exit_code, None);
        assert_eq!((restart_via_client.rows, restart_via_client.cols), (30, 105));

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
                .pty_snapshot(&term.thread_id, 30, 105)
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
}
