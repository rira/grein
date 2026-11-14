use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use grein_core::model::{AuthorRole, ConversationNode, NodeId};
use grein_core::storage::sqlite::SqliteStorage;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

#[allow(dead_code)]
pub fn default_socket_path() -> PathBuf {
    std::env::temp_dir().join("grein.sock")
}

pub async fn run_daemon_with_socket(
    socket_path: PathBuf,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let storage = Arc::new(SqliteStorage::in_memory()?);
    let entry_count = Arc::new(AtomicUsize::new(0));
    let listener = UnixListener::bind(&socket_path)?;

    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, _addr)) => {
                        let store = Arc::clone(&storage);
                        let entries = Arc::clone(&entry_count);
                        tokio::spawn(async move {
                            let (reader, mut writer) = stream.into_split();
                            let mut lines = BufReader::new(reader).lines();
                            while let Ok(Some(line)) = lines.next_line().await {
                                let trimmed = line.trim();
                                if trimmed == "ping" {
                                    let _ = writer.write_all(b"pong\n").await;
                                    let _ = writer.flush().await;
                                } else if let Some(prompt) = trimmed.strip_prefix("send_turn:") {
                                    let node_id = NodeId::new();
                                    let node = ConversationNode::new(
                                        node_id,
                                        None::<NodeId>,
                                        AuthorRole::User,
                                        prompt.to_string(),
                                    );
                                    if store.insert_node(&node).is_ok() {
                                        entries.fetch_add(1, Ordering::Relaxed);
                                    }
                                    let resp = format!("ok:{}\n", node_id);
                                    let _ = writer.write_all(resp.as_bytes()).await;
                                    let _ = writer.flush().await;
                                } else if trimmed == "list_entries" {
                                    let db_count = store
                                        .list_blackboard_entries()
                                        .map(|v| v.len())
                                        .unwrap_or(0);
                                    let count = db_count + entries.load(Ordering::Relaxed);
                                    let resp = format!("entries:{}\n", count);
                                    let _ = writer.write_all(resp.as_bytes()).await;
                                    let _ = writer.flush().await;
                                } else {
                                    let _ = writer.write_all(b"ok\n").await;
                                    let _ = writer.flush().await;
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
