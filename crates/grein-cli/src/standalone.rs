use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

#[derive(Debug)]
pub enum StandaloneError {
    DaemonNotRunning,
    DaemonTerminated,
}

impl std::fmt::Display for StandaloneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DaemonNotRunning => write!(f, "Embedded standalone daemon is not running"),
            Self::DaemonTerminated => {
                write!(f, "Embedded standalone daemon terminated unexpectedly")
            }
        }
    }
}

impl std::error::Error for StandaloneError {}

enum InProcessRequest {
    Ping(oneshot::Sender<Result<(), StandaloneError>>),
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

/// Runs the embedded standalone daemon service as an in-process Tokio task.
///
/// Guarantees zero socket allocation or external IPC bindings on the filesystem.
pub async fn run_standalone(mut shutdown_rx: oneshot::Receiver<()>) -> Result<(), StandaloneError> {
    let (tx, mut rx) = mpsc::channel(32);
    let daemon_id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

    {
        let mut lock = ACTIVE_DAEMONS.lock().unwrap();
        lock.push((daemon_id, tx));
    }
    let _guard = DaemonGuard(daemon_id);

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
                    None => {
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

/// Client handle for interacting with the embedded in-process daemon.
#[derive(Clone, Debug)]
pub struct InProcessClient {
    tx: mpsc::Sender<InProcessRequest>,
}

impl InProcessClient {
    /// Sends an in-process health ping to the active standalone daemon.
    pub async fn ping(&mut self) -> Result<(), StandaloneError> {
        let (resp_tx, resp_rx) = oneshot::channel();
        if self.tx.send(InProcessRequest::Ping(resp_tx)).await.is_err() {
            // Fall back to any active in-process daemon if current channel closed
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
}

/// Creates a client connection to the running embedded in-process daemon.
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
