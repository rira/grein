use std::path::{Path, PathBuf};
use std::sync::Arc;

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

pub async fn run_daemon_with_socket_and_db(
    socket_path: PathBuf,
    db_path: PathBuf,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
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
                                let response = if trimmed.starts_with('{') {
                                    format!("{}\n", dispatch_json_rpc(engine.as_ref(), trimmed).await)
                                } else {
                                    handle_legacy(engine.as_ref(), trimmed).await
                                };
                                if writer.write_all(response.as_bytes()).await.is_err() {
                                    break;
                                }
                                if writer.flush().await.is_err() {
                                    break;
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
