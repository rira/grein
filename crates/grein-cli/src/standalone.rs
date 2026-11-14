use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use grein_core::model::{AnchorId, AuthorRole, ConversationNode, NodeId, ThreadId};
use grein_core::storage::sqlite::SqliteStorage;
use tokio::sync::{mpsc, oneshot};

#[derive(Debug)]
pub enum StandaloneError {
    DaemonNotRunning,
    DaemonTerminated,
    Storage(String),
}

impl std::fmt::Display for StandaloneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DaemonNotRunning => write!(f, "Embedded standalone daemon is not running"),
            Self::DaemonTerminated => {
                write!(f, "Embedded standalone daemon terminated unexpectedly")
            }
            Self::Storage(msg) => write!(f, "Storage error: {msg}"),
        }
    }
}

impl std::error::Error for StandaloneError {}

pub enum InProcessRequest {
    Ping(oneshot::Sender<Result<(), StandaloneError>>),
    SendTurn {
        prompt: String,
        responder: oneshot::Sender<Result<NodeId, StandaloneError>>,
    },
    ForkThread {
        parent_node_id: NodeId,
        anchor_id: AnchorId,
        responder: oneshot::Sender<Result<ThreadId, StandaloneError>>,
    },
    ListThreads {
        responder: oneshot::Sender<Result<Vec<ThreadId>, StandaloneError>>,
    },
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static ACTIVE_DAEMONS: Mutex<Vec<(u64, mpsc::Sender<InProcessRequest>)>> = Mutex::new(Vec::new());

struct DaemonGuard(u64);

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        if let Ok(mut lock) = ACTIVE_DAEMONS.lock() {
            lock.retain(|(id, _)| *id != self.0);
        }
    }
}

pub async fn run_standalone(mut shutdown_rx: oneshot::Receiver<()>) -> Result<(), StandaloneError> {
    let (tx, mut rx) = mpsc::channel(32);
    let daemon_id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

    {
        let mut lock = ACTIVE_DAEMONS.lock().unwrap();
        lock.push((daemon_id, tx));
    }
    let _guard = DaemonGuard(daemon_id);

    let storage =
        SqliteStorage::in_memory().map_err(|e| StandaloneError::Storage(e.to_string()))?;
    let mut thread_ids: Vec<ThreadId> = Vec::new();
    let mut last_node_id: Option<NodeId> = None;

    loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                break;
            }
            request = rx.recv() => {
                match request {
                    Some(InProcessRequest::Ping(responder)) => {
                        let _ = responder.send(Ok(()));
                    }
                    Some(InProcessRequest::SendTurn { prompt, responder }) => {
                        let node_id = NodeId::new();
                        let node = ConversationNode::new(
                            node_id,
                            last_node_id,
                            AuthorRole::User,
                            prompt,
                        );
                        let res = storage
                            .insert_node(&node)
                            .map(|()| {
                                last_node_id = Some(node_id);
                                node_id
                            })
                            .map_err(|e| StandaloneError::Storage(e.to_string()));
                        let _ = responder.send(res);
                    }
                    Some(InProcessRequest::ForkThread {
                        parent_node_id,
                        anchor_id,
                        responder,
                    }) => {
                        let res = storage
                            .create_thread(parent_node_id, anchor_id)
                            .inspect(|&tid| {
                                thread_ids.push(tid);
                            })
                            .map_err(|e| StandaloneError::Storage(e.to_string()));
                        let _ = responder.send(res);
                    }
                    Some(InProcessRequest::ListThreads { responder }) => {
                        let _ = responder.send(Ok(thread_ids.clone()));
                    }
                    None => {
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

#[derive(Clone, Debug)]
pub struct InProcessClient {
    tx: mpsc::Sender<InProcessRequest>,
}

impl InProcessClient {
    pub async fn ping(&mut self) -> Result<(), StandaloneError> {
        let (resp_tx, resp_rx) = oneshot::channel();
        if self.tx.send(InProcessRequest::Ping(resp_tx)).await.is_err() {
            let fallback_tx = {
                let lock = ACTIVE_DAEMONS.lock().unwrap();
                lock.last().map(|(_, tx)| tx.clone())
            };
            if let Some(tx) = fallback_tx {
                self.tx = tx;
                let (resp_tx2, resp_rx2) = oneshot::channel();
                self.tx
                    .send(InProcessRequest::Ping(resp_tx2))
                    .await
                    .map_err(|_| StandaloneError::DaemonTerminated)?;
                return resp_rx2
                    .await
                    .map_err(|_| StandaloneError::DaemonTerminated)?;
            } else {
                return Err(StandaloneError::DaemonTerminated);
            }
        }

        resp_rx
            .await
            .map_err(|_| StandaloneError::DaemonTerminated)?
    }

    pub async fn send_turn(
        &mut self,
        prompt: impl Into<String>,
    ) -> Result<NodeId, StandaloneError> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.tx
            .send(InProcessRequest::SendTurn {
                prompt: prompt.into(),
                responder: resp_tx,
            })
            .await
            .map_err(|_| StandaloneError::DaemonTerminated)?;
        resp_rx
            .await
            .map_err(|_| StandaloneError::DaemonTerminated)?
    }

    pub async fn fork_thread(
        &mut self,
        parent_node_id: NodeId,
        anchor_id: AnchorId,
    ) -> Result<ThreadId, StandaloneError> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.tx
            .send(InProcessRequest::ForkThread {
                parent_node_id,
                anchor_id,
                responder: resp_tx,
            })
            .await
            .map_err(|_| StandaloneError::DaemonTerminated)?;
        resp_rx
            .await
            .map_err(|_| StandaloneError::DaemonTerminated)?
    }

    pub async fn list_threads(&mut self) -> Result<Vec<ThreadId>, StandaloneError> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.tx
            .send(InProcessRequest::ListThreads { responder: resp_tx })
            .await
            .map_err(|_| StandaloneError::DaemonTerminated)?;
        resp_rx
            .await
            .map_err(|_| StandaloneError::DaemonTerminated)?
    }
}

pub async fn create_in_process_client() -> Result<InProcessClient, StandaloneError> {
    for _ in 0..50 {
        let sender = {
            let lock = ACTIVE_DAEMONS.lock().unwrap();
            lock.last().map(|(_, tx)| tx.clone())
        };
        if let Some(tx) = sender {
            return Ok(InProcessClient { tx });
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    Err(StandaloneError::DaemonNotRunning)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_standalone_send_turn_fork_and_list_threads() {
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let handle = tokio::spawn(async move { run_standalone(shutdown_rx).await });

        let mut client = create_in_process_client()
            .await
            .expect("connect standalone");
        client.ping().await.expect("ping succeeds");

        let root_node = client
            .send_turn("Design conversation DAG")
            .await
            .expect("send_turn succeeds");
        let thread_id = client
            .fork_thread(root_node, AnchorId::new())
            .await
            .expect("fork_thread succeeds");
        let threads = client.list_threads().await.expect("list_threads succeeds");
        assert!(threads.contains(&thread_id));

        let _ = shutdown_tx.send(());
        let _ = handle.await;
    }
}
