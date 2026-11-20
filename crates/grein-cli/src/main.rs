use std::io;
use std::sync::Arc;

use clap::Parser;
use grein_cli::args::{AttachArgs, Command, DaemonArgs};
use grein_cli::engine::{DaemonEngine, InProcessEngine, UnixSocketEngineClient};
use grein_tui::engine_port::{
    HydratedBlackboardEntry, HydratedSession, HydratedThread, HydratedTurn, TuiEngineEvent,
    TuiEnginePort,
};
use serde_json::Value;
use std::io::IsTerminal;
use tokio::sync::{mpsc, Mutex};

#[derive(Debug, Parser, Clone, PartialEq, Eq)]
#[command(name = "grein", version, about = "Grein autonomous engineering CLI")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

/// Adapts an engine client (any `DaemonEngine`) to the TUI engine port.
struct EngineClientPort<E> {
    client: E,
    events_tx: mpsc::UnboundedSender<TuiEngineEvent>,
    events_rx: Mutex<Option<mpsc::UnboundedReceiver<TuiEngineEvent>>>,
}

impl<E> EngineClientPort<E> {
    fn new(client: E) -> Self {
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        Self {
            client,
            events_tx,
            events_rx: Mutex::new(Some(events_rx)),
        }
    }

    fn emit(&self, event: TuiEngineEvent) {
        let _ = self.events_tx.send(event);
    }
}

fn to_io<E: std::fmt::Debug>(err: E) -> io::Error {
    io::Error::other(format!("{err:?}"))
}

fn str_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_default()
}

fn turns_from(transcript: &Value) -> Vec<HydratedTurn> {
    transcript
        .as_array()
        .map(|nodes| {
            nodes
                .iter()
                .enumerate()
                .map(|(i, node)| HydratedTurn {
                    id: match str_field(node, "id") {
                        id if id.is_empty() => format!("turn-{i}"),
                        id => id,
                    },
                    role: str_field(node, "role"),
                    content: str_field(node, "content"),
                })
                .collect()
        })
        .unwrap_or_default()
}

#[async_trait::async_trait]
impl<E> TuiEnginePort for EngineClientPort<E>
where
    E: DaemonEngine + Send + Sync + 'static,
{
    async fn hydrate_session(&self) -> io::Result<HydratedSession> {
        let threads_value = serde_json::to_value(self.client.list_threads().await.map_err(to_io)?)
            .map_err(to_io)?;
        // The engine exposes the transcript of its active thread only.
        let active_turns = turns_from(
            &serde_json::to_value(self.client.get_transcript().await.map_err(to_io)?)
                .map_err(to_io)?,
        );
        let mut active_turns = Some(active_turns);
        let mut threads = Vec::new();
        for item in threads_value.as_array().map(Vec::as_slice).unwrap_or(&[]) {
            let id = str_field(item, "id");
            let title = match str_field(item, "title") {
                t if t.is_empty() => id.clone(),
                t => t,
            };
            threads.push(HydratedThread {
                id,
                title,
                turns: active_turns.take().unwrap_or_default(),
            });
        }
        let board = serde_json::to_value(self.client.blackboard_list().await.map_err(to_io)?)
            .map_err(to_io)?;
        let blackboard = board
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .map(|e| HydratedBlackboardEntry {
                        key: str_field(e, "key"),
                        value: str_field(e, "value"),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let active_thread_id = threads.first().map(|t| t.id.clone()).unwrap_or_default();
        Ok(HydratedSession {
            active_thread_id,
            threads,
            blackboard,
        })
    }

    async fn send_turn(&self, thread_id: &str, prompt: &str) -> io::Result<()> {
        self.emit(TuiEngineEvent::ThreadStatusChanged {
            thread_id: thread_id.to_string(),
            status: "streaming".to_string(),
        });
        if let Err(err) = self.client.send_turn(prompt).await {
            let err = to_io(err);
            self.emit(TuiEngineEvent::Error {
                message: err.to_string(),
            });
            return Err(err);
        }
        let transcript = serde_json::to_value(self.client.get_transcript().await.map_err(to_io)?)
            .map_err(to_io)?;
        if let Some(last) = turns_from(&transcript).pop() {
            if !last.role.eq_ignore_ascii_case("user") {
                self.emit(TuiEngineEvent::StreamChunk {
                    thread_id: thread_id.to_string(),
                    content: last.content,
                });
            }
        }
        self.emit(TuiEngineEvent::TurnCompleted {
            thread_id: thread_id.to_string(),
        });
        self.emit(TuiEngineEvent::ThreadStatusChanged {
            thread_id: thread_id.to_string(),
            status: "idle".to_string(),
        });
        Ok(())
    }

    async fn fork_thread(&self, _thread_id: &str, anchor_id: &str) -> io::Result<String> {
        let forked = serde_json::to_value(
            self.client
                .fork_thread(Some(anchor_id))
                .await
                .map_err(to_io)?,
        )
        .map_err(to_io)?;
        Ok(match forked {
            Value::String(s) => s,
            other => str_field(&other, "id"),
        })
    }

    async fn merge_thread(&self, source_id: &str, target_id: &str) -> io::Result<()> {
        // The engine merges a source thread into its active trunk; the target is implicit.
        tracing::debug!(target_id, "merging thread into active trunk");
        self.client.merge_thread(source_id).await.map_err(to_io)?;
        Ok(())
    }

    async fn resolve_permission(&self, request_id: &str, approved: bool) -> io::Result<()> {
        let payload = serde_json::from_value(serde_json::json!({
            "request_id": request_id,
            "id": request_id,
            "approved": approved,
            "allow": approved,
            "granted": approved,
        }))
        .map_err(to_io)?;
        self.client
            .resolve_permission(payload)
            .await
            .map_err(to_io)?;
        Ok(())
    }

    async fn subscribe_events(&self) -> io::Result<mpsc::UnboundedReceiver<TuiEngineEvent>> {
        self.events_rx
            .lock()
            .await
            .take()
            .ok_or_else(|| io::Error::other("event stream already subscribed"))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();

    match cli.command {
        None | Some(Command::Standalone {}) => {
            if std::io::stdout().is_terminal() {
                let engine = InProcessEngine::open(&grein_cli::daemon::default_db_path())?;
                grein_tui::run_with_engine(Arc::new(EngineClientPort::new(engine))).await?;
            }
        }
        Some(Command::Daemon(DaemonArgs { socket })) => {
            let socket_path = socket.unwrap_or_else(grein_cli::daemon::default_socket_path);
            grein_cli::daemon::run_daemon_with_socket(socket_path).await?;
        }
        Some(Command::Attach(AttachArgs { socket })) => {
            let socket_path = socket.unwrap_or_else(grein_cli::daemon::default_socket_path);
            let _probe = grein_cli::client::connect_to_daemon(&socket_path).await?;
            if std::io::stdout().is_terminal() {
                let transport = UnixSocketEngineClient::connect(&socket_path).await?;
                grein_tui::run_with_engine(Arc::new(EngineClientPort::new(transport))).await?;
            }
        }
    }

    Ok(())
}
