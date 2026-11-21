use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use grein_core::model::{AnchorId, NodeId, ThreadId};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use crate::engine::{dispatch_json_rpc, DaemonEngine, InProcessEngine};

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
    JsonRpc {
        request: String,
        responder: oneshot::Sender<String>,
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

fn last_sender() -> Option<mpsc::Sender<InProcessRequest>> {
    ACTIVE_DAEMONS
        .lock()
        .ok()
        .and_then(|lock| lock.last().map(|(_, tx)| tx.clone()))
}

fn parse_id<T: DeserializeOwned>(raw: &str) -> Result<T, StandaloneError> {
    let normalized = raw
        .strip_prefix("ThreadId(")
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(raw);
    serde_json::from_value(Value::String(normalized.to_string()))
        .map_err(|e| StandaloneError::Storage(e.to_string()))
}

pub async fn run_standalone_with_channel(
    mut rx: mpsc::Receiver<InProcessRequest>,
    mut shutdown_rx: oneshot::Receiver<()>,
) -> Result<(), StandaloneError> {
    let engine = InProcessEngine::in_memory().map_err(|e| StandaloneError::Storage(e.to_string()))?;

    loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                break;
            }
            request = rx.recv() => {
                match request {
                    Some(InProcessRequest::Ping(responder)) => {
                        let res = engine
                            .ping()
                            .await
                            .map_err(|e| StandaloneError::Storage(e.to_string()));
                        let _ = responder.send(res);
                    }
                    Some(InProcessRequest::SendTurn { prompt, responder }) => {
                        let res = match engine.send_turn(&prompt).await {
                            Ok(turn) => parse_id::<NodeId>(&turn.node_id),
                            Err(e) => Err(StandaloneError::Storage(e.to_string())),
                        };
                        let _ = responder.send(res);
                    }
                    Some(InProcessRequest::ForkThread {
                        parent_node_id,
                        anchor_id: _,
                        responder,
                    }) => {
                        let parent = parent_node_id.to_string();
                        let res = match engine.fork_thread(Some(&parent)).await {
                            Ok(summary) => parse_id::<ThreadId>(&summary.thread_id),
                            Err(e) => Err(StandaloneError::Storage(e.to_string())),
                        };
                        let _ = responder.send(res);
                    }
                    Some(InProcessRequest::ListThreads { responder }) => {
                        let res = match engine.list_threads().await {
                            Ok(summaries) => summaries
                                .into_iter()
                                .map(|summary| parse_id::<ThreadId>(&summary.thread_id))
                                .collect::<Result<Vec<_>, _>>(),
                            Err(e) => Err(StandaloneError::Storage(e.to_string())),
                        };
                        let _ = responder.send(res);
                    }
                    Some(InProcessRequest::JsonRpc { request, responder }) => {
                        let response = dispatch_json_rpc(&engine, &request).await;
                        let _ = responder.send(response);
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

pub async fn run_standalone(shutdown_rx: oneshot::Receiver<()>) -> Result<(), StandaloneError> {
    let (tx, rx) = mpsc::channel(32);
    let daemon_id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

    {
        let mut lock = ACTIVE_DAEMONS
            .lock()
            .map_err(|e| StandaloneError::Storage(e.to_string()))?;
        lock.push((daemon_id, tx));
    }
    let _guard = DaemonGuard(daemon_id);

    run_standalone_with_channel(rx, shutdown_rx).await
}

pub fn spawn_standalone_pair() -> (
    InProcessClient,
    oneshot::Sender<()>,
    tokio::task::JoinHandle<Result<(), StandaloneError>>,
) {
    let (tx, rx) = mpsc::channel(32);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let handle = tokio::spawn(async move { run_standalone_with_channel(rx, shutdown_rx).await });
    (InProcessClient::new(tx), shutdown_tx, handle)
}

#[derive(Clone, Debug)]
pub struct InProcessClient {
    tx: mpsc::Sender<InProcessRequest>,
}

impl InProcessClient {
    pub fn new(tx: mpsc::Sender<InProcessRequest>) -> Self {
        Self { tx }
    }

    pub async fn ping(&mut self) -> Result<(), StandaloneError> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.tx
            .send(InProcessRequest::Ping(resp_tx))
            .await
            .map_err(|_| StandaloneError::DaemonTerminated)?;
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

    /// Performs a JSON-RPC 2.0 round-trip against the embedded standalone engine.
    pub async fn json_rpc(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Value, StandaloneError> {
        let request = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let (resp_tx, resp_rx) = oneshot::channel();
        self.tx
            .send(InProcessRequest::JsonRpc {
                request: request.to_string(),
                responder: resp_tx,
            })
            .await
            .map_err(|_| StandaloneError::DaemonTerminated)?;
        let raw = resp_rx
            .await
            .map_err(|_| StandaloneError::DaemonTerminated)?;
        serde_json::from_str(&raw).map_err(|e| StandaloneError::Storage(e.to_string()))
    }
}

pub async fn create_in_process_client() -> Result<InProcessClient, StandaloneError> {
    for _ in 0..50 {
        if let Some(tx) = last_sender() {
            return Ok(InProcessClient::new(tx));
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
        let (mut client, shutdown_tx, handle) = spawn_standalone_pair();
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
        handle
            .await
            .expect("standalone task join")
            .expect("standalone exits cleanly");
    }

    #[tokio::test]
    async fn test_standalone_json_rpc_ping() {
        let (mut client, shutdown_tx, handle) = spawn_standalone_pair();
        let response = client
            .json_rpc("session.ping", json!({}))
            .await
            .expect("json-rpc ping");
        assert_eq!(response["jsonrpc"], "2.0");
        assert!(response.get("result").is_some());

        let _ = shutdown_tx.send(());
        handle
            .await
            .expect("standalone task join")
            .expect("standalone exits cleanly");
    }

    #[tokio::test]
    async fn test_standalone_typed_and_json_rpc_share_single_engine() {
        let (mut client, shutdown_tx, handle) = spawn_standalone_pair();

        let root_node = client
            .send_turn("Shared state turn")
            .await
            .expect("send_turn succeeds");
        let thread_id = client
            .fork_thread(root_node, AnchorId::new())
            .await
            .expect("fork_thread succeeds");

        let transcript = client
            .json_rpc("session.get_transcript", json!({}))
            .await
            .expect("json-rpc get_transcript");
        let transcript_entries = transcript["result"]
            .as_array()
            .expect("transcript result array");
        assert_eq!(transcript_entries.len(), 1);
        assert_eq!(transcript_entries[0]["content"], "Shared state turn");

        let rpc_threads = client
            .json_rpc("session.list_threads", json!({}))
            .await
            .expect("json-rpc list_threads");
        let rpc_thread_entries = rpc_threads["result"]
            .as_array()
            .expect("threads result array");
        assert_eq!(rpc_thread_entries.len(), 1);

        let rpc_turn = client
            .json_rpc("session.send_turn", json!({"prompt": "RPC turn"}))
            .await
            .expect("json-rpc send_turn");
        let rpc_node_id = rpc_turn["result"]["node_id"]
            .as_str()
            .expect("node_id string");
        let rpc_fork = client
            .json_rpc("session.fork_thread", json!({"parent_node_id": rpc_node_id}))
            .await
            .expect("json-rpc fork_thread");
        let rpc_thread_raw = rpc_fork["result"]["thread_id"]
            .as_str()
            .expect("thread_id string");
        let rpc_thread_id: ThreadId = parse_id(rpc_thread_raw).expect("parse rpc thread_id");

        let threads = client.list_threads().await.expect("list_threads succeeds");
        assert_eq!(threads.len(), 2);
        assert!(threads.contains(&thread_id));
        assert!(threads.contains(&rpc_thread_id));

        let _ = shutdown_tx.send(());
        handle
            .await
            .expect("standalone task join")
            .expect("standalone exits cleanly");
    }

    #[tokio::test]
    async fn test_standalone_paired_client_reports_terminated_after_shutdown() {
        let (mut client, shutdown_tx, handle) = spawn_standalone_pair();
        client.ping().await.expect("initial ping succeeds");

        let _ = shutdown_tx.send(());
        handle
            .await
            .expect("standalone task join")
            .expect("standalone exits cleanly");

        let err = client
            .ping()
            .await
            .expect_err("ping after shutdown must fail");
        assert!(matches!(err, StandaloneError::DaemonTerminated));
    }
}
