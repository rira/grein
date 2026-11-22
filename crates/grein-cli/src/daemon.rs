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
            Ok(snapshot) => format!(
                "entries:{}\n",
                snapshot.blackboard_entries + snapshot.node_count
            ),
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
    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }
    if let Some(parent) = socket_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let engine = Arc::new(InProcessEngine::open(&db_path)?);
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

        client
            .blackboard_upsert("mode", "daemon")
            .await
            .expect("upsert");
        assert!(client.blackboard_delete("mode").await.expect("delete"));

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
            TuiEngineEvent::BlackboardUpdated { key, value } if key == "mode" && value == "daemon"
        )));
        assert!(seen.iter().any(|e| matches!(
            e,
            TuiEngineEvent::BlackboardDeleted { key } if key == "mode"
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

        server.abort();
    }
}
