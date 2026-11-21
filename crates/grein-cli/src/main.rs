use std::io;
use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use clap::Parser;
use grein_cli::args::{AttachArgs, Command, DaemonArgs};
use grein_cli::engine::{
    DaemonEngine, InProcessEngine, PermissionDecisionPayload, UnixSocketEngineClient,
};
use grein_tui::engine_port::{
    HydratedBlackboardEntry, HydratedSession, HydratedThread, HydratedTurn, TuiEngineEvent,
    TuiEnginePort,
};
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
    has_live_stream: AtomicBool,
}

impl<E> EngineClientPort<E> {
    fn new(client: E) -> Self {
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        Self {
            client,
            events_tx,
            events_rx: Mutex::new(Some(events_rx)),
            has_live_stream: AtomicBool::new(false),
        }
    }

    fn emit(&self, event: TuiEngineEvent) {
        let _ = self.events_tx.send(event);
    }
}

fn to_io<E: std::fmt::Display>(err: E) -> io::Error {
    io::Error::other(err.to_string())
}

#[async_trait::async_trait]
impl<E> TuiEnginePort for EngineClientPort<E>
where
    E: DaemonEngine + Send + Sync + 'static,
{
    async fn hydrate_session(&self) -> io::Result<HydratedSession> {
        let main = self.client.ensure_main_thread().await.map_err(to_io)?;
        let summaries = self.client.list_threads().await.map_err(to_io)?;
        let mut threads = Vec::with_capacity(summaries.len());
        for summary in summaries {
            let entries = self
                .client
                .get_thread_transcript(&summary.thread_id)
                .await
                .map_err(to_io)?;
            let turns = entries
                .into_iter()
                .map(|entry| HydratedTurn {
                    id: entry.node_id,
                    role: entry.role,
                    content: entry.content,
                })
                .collect();
            threads.push(HydratedThread {
                id: summary.thread_id,
                title: summary.title,
                turns,
            });
        }
        let board = self.client.blackboard_list().await.map_err(to_io)?;
        let blackboard = board
            .into_iter()
            .map(|entry| HydratedBlackboardEntry {
                key: entry.key,
                value: entry.value,
            })
            .collect();
        let active_thread_id = threads
            .first()
            .map(|t| t.id.clone())
            .unwrap_or(main.thread_id);
        Ok(HydratedSession {
            active_thread_id,
            threads,
            blackboard,
        })
    }

    async fn send_turn(&self, thread_id: &str, prompt: &str) -> io::Result<()> {
        if self.has_live_stream.load(Ordering::SeqCst) {
            self.client
                .send_turn_for_thread(thread_id, prompt)
                .await
                .map_err(to_io)?;
            return Ok(());
        }

        self.emit(TuiEngineEvent::ThreadStatusChanged {
            thread_id: thread_id.to_string(),
            status: "streaming".to_string(),
        });
        self.client
            .send_turn_for_thread(thread_id, prompt)
            .await
            .map_err(to_io)?;
        if let Some(last) = self
            .client
            .get_thread_transcript(thread_id)
            .await
            .map_err(to_io)?
            .pop()
        {
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

    async fn fork_thread(&self, thread_id: &str, anchor_id: &str) -> io::Result<String> {
        let trimmed_anchor = anchor_id.trim();
        let (source, title) = if trimmed_anchor.is_empty() {
            (Some(thread_id), None)
        } else if trimmed_anchor.len() == 36
            && trimmed_anchor
                .as_bytes()
                .iter()
                .enumerate()
                .all(|(i, b)| match i {
                    8 | 13 | 18 | 23 => *b == b'-',
                    _ => b.is_ascii_hexdigit(),
                })
        {
            (Some(trimmed_anchor), None)
        } else {
            (Some(thread_id), Some(trimmed_anchor))
        };
        let forked = self
            .client
            .fork_thread_with_title(source, title)
            .await
            .map_err(to_io)?;
        Ok(forked.thread_id)
    }

    async fn fork_branch(
        &self,
        source_thread_id: &str,
        title: Option<&str>,
    ) -> io::Result<HydratedThread> {
        let forked = self
            .client
            .fork_thread_with_title(Some(source_thread_id), title)
            .await
            .map_err(to_io)?;
        let entries = self
            .client
            .get_thread_transcript(&forked.thread_id)
            .await
            .map_err(to_io)?;
        let turns = entries
            .into_iter()
            .map(|entry| HydratedTurn {
                id: entry.node_id,
                role: entry.role,
                content: entry.content,
            })
            .collect();
        Ok(HydratedThread {
            id: forked.thread_id,
            title: forked.title,
            turns,
        })
    }

    async fn merge_thread(&self, source_id: &str, target_id: &str) -> io::Result<()> {
        tracing::debug!(target_id, "merging thread into active trunk");
        self.client.merge_thread(source_id).await.map_err(to_io)?;
        Ok(())
    }

    async fn resolve_permission(&self, request_id: &str, approved: bool) -> io::Result<()> {
        let payload = PermissionDecisionPayload {
            request_id: request_id.to_string(),
            allow: approved,
        };
        self.client
            .resolve_permission(payload)
            .await
            .map_err(to_io)?;
        Ok(())
    }

    async fn cancel_turn(&self, thread_id: &str) -> io::Result<()> {
        self.client.cancel_turn(thread_id).await.map_err(to_io)
    }

    async fn upsert_blackboard(&self, key: &str, value: &str) -> io::Result<()> {
        self.client
            .blackboard_upsert(key, value)
            .await
            .map_err(to_io)?;
        Ok(())
    }

    async fn delete_blackboard(&self, key: &str) -> io::Result<()> {
        self.client.blackboard_delete(key).await.map_err(to_io)?;
        Ok(())
    }

    async fn run_shell_command(&self, thread_id: &str, command: &str) -> io::Result<()> {
        let result = self
            .client
            .run_shell_command(thread_id, command)
            .await
            .map_err(to_io)?;
        if !self.has_live_stream.load(Ordering::SeqCst) {
            self.emit(TuiEngineEvent::ShellOutput {
                thread_id: thread_id.to_string(),
                command: result.command,
                stdout: result.stdout,
                stderr: result.stderr,
                exit_code: result.exit_code,
            });
        }
        Ok(())
    }

    async fn subscribe_events(&self) -> io::Result<mpsc::UnboundedReceiver<TuiEngineEvent>> {
        let rx = self
            .events_rx
            .lock()
            .await
            .take()
            .ok_or_else(|| io::Error::other("event stream already subscribed"))?;
        if let Some(mut engine_rx) = self.client.subscribe_events().await.map_err(to_io)? {
            self.has_live_stream.store(true, Ordering::SeqCst);
            let tx = self.events_tx.clone();
            tokio::spawn(async move {
                while let Some(ev) = engine_rx.recv().await {
                    // Synchronous Err returns from TuiEnginePort already surface errors via runner.rs.
                    if matches!(ev, TuiEngineEvent::Error { .. }) {
                        continue;
                    }
                    if tx.send(ev).is_err() {
                        break;
                    }
                }
            });
        }
        Ok(rx)
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
            let transport = UnixSocketEngineClient::connect(&socket_path).await?;
            if std::io::stdout().is_terminal() {
                grein_tui::run_with_engine(Arc::new(EngineClientPort::new(transport))).await?;
            }
        }
        Some(Command::Threads(args)) => {
            grein_cli::headless::handle_threads(&args).await?;
        }
        Some(Command::Status(args)) => {
            grein_cli::headless::handle_status(&args).await?;
        }
        Some(Command::Agents(args)) => {
            grein_cli::headless::handle_agents(&args).await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_engine_client_port_hydrates_main_thread_blackboard_and_no_duplicate_errors() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("session.db");

        let engine =
            InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
                .expect("open");
        let port = EngineClientPort::new(engine);

        let session = port.hydrate_session().await.expect("hydrate");
        assert_eq!(session.threads.len(), 1);
        assert_eq!(session.active_thread_id, session.threads[0].id);
        assert_eq!(session.threads[0].title, "Main");
        assert!(session.threads[0].turns.is_empty());

        let mut rx = port.subscribe_events().await.expect("subscribe");

        port.upsert_blackboard("arch", "two-tier")
            .await
            .expect("upsert");
        let forked = port
            .fork_branch(&session.active_thread_id, Some("auth"))
            .await
            .expect("fork_branch");
        assert_eq!(forked.title, "auth");

        let err = port
            .send_turn(&session.active_thread_id, "hello")
            .await
            .expect_err("offline send_turn must fail");
        assert!(!err.to_string().is_empty());

        port.run_shell_command(&session.active_thread_id, "printf 'hi'")
            .await
            .expect("shell");

        let mut events = Vec::new();
        for _ in 0..20 {
            while let Ok(ev) = rx.try_recv() {
                events.push(ev);
            }
            if events
                .iter()
                .any(|e| matches!(e, TuiEngineEvent::ShellOutput { .. }))
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, TuiEngineEvent::Error { .. })),
            "EngineClientPort must not emit duplicate Error event when send_turn returns Err"
        );
        assert!(events.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ShellOutput { stdout, exit_code, .. } if stdout == "hi" && *exit_code == 0
        )));

        port.delete_blackboard("arch").await.expect("delete");
        drop(port);

        let reopened =
            InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
                .expect("reopen");
        let port2 = EngineClientPort::new(reopened);
        let session2 = port2.hydrate_session().await.expect("rehydrate");
        assert_eq!(session2.threads.len(), 2);
        assert_eq!(session2.threads[0].turns.len(), 1);
        assert_eq!(session2.threads[0].turns[0].role, "user");
        assert_eq!(session2.threads[0].turns[0].content, "hello");
        assert!(session2.blackboard.is_empty());
    }

    #[tokio::test]
    async fn test_engine_client_port_live_acp_streams_permissions_and_rehydrates() {
        use grein_cli::engine::AcpBackendConfig;
        use std::os::unix::fs::PermissionsExt;
        use std::time::Duration;
        use tokio::time::timeout;

        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("session.db");
        let script_path = temp.path().join("mock_tui_acp.sh");
        let script = r#"#!/bin/sh
while IFS= read -r line; do
    rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
    case "$line" in
        *'"method":"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"mock","version":"1.0"},"authMethods":[],"agentCapabilities":{}}}\n' "$id"
            ;;
        *'"method":"session/new"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sess-tui"}}\n' "$id"
            ;;
        *'"method":"session/prompt"'*)
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-tui","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Mocked TUI Reply"}}}}\n'
            printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            ;;
    esac
done
"#;
        std::fs::write(&script_path, script).expect("write");
        let mut perms = std::fs::metadata(&script_path)
            .expect("meta")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).expect("chmod");

        let cfg = AcpBackendConfig {
            command: script_path.to_string_lossy().into_owned(),
            args: Vec::new(),
            auth_method: None,
            cwd: temp.path().to_path_buf(),
        };
        let engine =
            InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
                .expect("open");
        let port = Arc::new(EngineClientPort::new(engine));

        let session = port.hydrate_session().await.expect("hydrate");
        let main_id = session.active_thread_id.clone();
        let mut rx = port.subscribe_events().await.expect("subscribe");

        port.send_turn(&main_id, "ping acp")
            .await
            .expect("send_turn");

        let mut chunks = String::new();
        let mut saw_completed = false;
        while let Ok(Some(ev)) = timeout(Duration::from_millis(200), rx.recv()).await {
            match ev {
                TuiEngineEvent::StreamChunk { content, .. } => chunks.push_str(&content),
                TuiEngineEvent::TurnCompleted { .. } => {
                    saw_completed = true;
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(chunks, "Mocked TUI Reply");
        assert!(saw_completed);

        port.cancel_turn(&main_id).await.expect("cancel_turn");
        let forked = port
            .fork_branch(&main_id, Some("side-branch"))
            .await
            .expect("fork_branch");
        assert_eq!(forked.turns.len(), 2);
        port.merge_thread(&forked.id, &main_id)
            .await
            .expect("merge_thread");
        port.resolve_permission("nonexistent-perm", true)
            .await
            .expect("resolve_permission");

        let reopened =
            InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
                .expect("reopen");
        let port2 = EngineClientPort::new(reopened);
        let hydrated = port2.hydrate_session().await.expect("rehydrate");
        assert_eq!(hydrated.threads.len(), 2);
        assert_eq!(hydrated.threads[0].turns.len(), 2);
        assert_eq!(hydrated.threads[0].turns[0].content, "ping acp");
        assert_eq!(hydrated.threads[0].turns[1].content, "Mocked TUI Reply");
    }
}
