use std::io;
use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use clap::Parser;
use grein_cli::args::{AttachArgs, Command, DaemonArgs};
use grein_cli::engine::{
    truncate_chars_with_ellipsis, DaemonEngine, InProcessEngine, PermissionDecisionPayload,
    UnixSocketEngineClient,
};
use grein_tui::engine_port::{
    HydratedSession, HydratedThread, HydratedTurn, ThreadMeta, TuiEngineEvent, TuiEnginePort,
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
    in_flight_turns: Arc<AtomicUsize>,
    buffered_sync_errors: Arc<StdMutex<Vec<String>>>,
    suppressed_sync_errors: Arc<StdMutex<Vec<(String, Instant)>>>,
}

impl<E> EngineClientPort<E> {
    fn new(client: E) -> Self {
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        Self {
            client,
            events_tx,
            events_rx: Mutex::new(Some(events_rx)),
            has_live_stream: AtomicBool::new(false),
            in_flight_turns: Arc::new(AtomicUsize::new(0)),
            buffered_sync_errors: Arc::new(StdMutex::new(Vec::new())),
            suppressed_sync_errors: Arc::new(StdMutex::new(Vec::new())),
        }
    }

    fn emit(&self, event: TuiEngineEvent) {
        let _ = self.events_tx.send(event);
    }
}

impl<E: DaemonEngine> EngineClientPort<E> {
    async fn load_hydrated_turns(&self, thread_id: &str) -> io::Result<Vec<HydratedTurn>> {
        let entries = self
            .client
            .get_thread_transcript(thread_id)
            .await
            .map_err(to_io)?;
        Ok(entries
            .into_iter()
            .map(|entry| HydratedTurn {
                id: entry.node_id,
                role: entry.role,
                content: entry.content,
            })
            .collect())
    }

    async fn fetch_and_emit_thread_metadata(&self) -> Option<Vec<ThreadMeta>> {
        match self.client.thread_metadata().await {
            Ok(meta) => {
                if !meta.is_empty() {
                    self.emit(TuiEngineEvent::ThreadMetadataUpdated { meta: meta.clone() });
                }
                Some(meta)
            }
            Err(err) => {
                self.emit(TuiEngineEvent::Error {
                    message: err.to_string(),
                });
                None
            }
        }
    }
}

fn error_matches_sync(err_msg: &str, event_msg: &str) -> bool {
    !event_msg.is_empty() && (err_msg == event_msg || err_msg.ends_with(event_msg))
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
        let active_thread_id = threads
            .first()
            .map(|t| t.id.clone())
            .unwrap_or(main.thread_id);
        let _ = self.fetch_and_emit_thread_metadata().await;
        if let Ok(agent_metas) = self.client.agent_metadata().await {
            for entry in agent_metas {
                if entry.model.is_some() || entry.mode.is_some() {
                    self.emit(TuiEngineEvent::AgentMetaUpdated {
                        thread_id: entry.thread_id,
                        model: entry.model,
                        mode: entry.mode,
                    });
                }
            }
        }
        Ok(HydratedSession {
            active_thread_id,
            threads,
        })
    }

    async fn send_turn(&self, thread_id: &str, prompt: &str) -> io::Result<()> {
        if self.has_live_stream.load(Ordering::SeqCst) {
            self.in_flight_turns.fetch_add(1, Ordering::SeqCst);
            let res = self.client.send_turn_for_thread(thread_id, prompt).await;
            if let Err(ref err) = res {
                let err_msg = err.to_string();
                let consumed_from_buffer = {
                    let mut buf = self
                        .buffered_sync_errors
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    if let Some(pos) = buf.iter().position(|m| error_matches_sync(&err_msg, m)) {
                        buf.remove(pos);
                        true
                    } else {
                        false
                    }
                };
                if !consumed_from_buffer {
                    let mut sup = self
                        .suppressed_sync_errors
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    sup.push((err_msg, Instant::now()));
                }
            }
            let remaining = self.in_flight_turns.fetch_sub(1, Ordering::SeqCst) - 1;
            if remaining == 0 {
                let leftover: Vec<String> = {
                    let mut buf = self
                        .buffered_sync_errors
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    std::mem::take(&mut *buf)
                };
                for msg in leftover {
                    self.emit(TuiEngineEvent::Error { message: msg });
                }
            }
            res.map(|_| ()).map_err(to_io)?;
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
        if !self.has_live_stream.load(Ordering::SeqCst) {
            let turns = self.load_hydrated_turns(&forked.thread_id).await?;
            self.emit(TuiEngineEvent::ThreadCreated {
                thread: HydratedThread {
                    id: forked.thread_id.clone(),
                    title: forked.title,
                    turns,
                },
            });
            let _ = self.fetch_and_emit_thread_metadata().await;
        }
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
        let turns = self.load_hydrated_turns(&forked.thread_id).await?;
        let hydrated = HydratedThread {
            id: forked.thread_id,
            title: forked.title,
            turns,
        };
        if !self.has_live_stream.load(Ordering::SeqCst) {
            self.emit(TuiEngineEvent::ThreadCreated {
                thread: hydrated.clone(),
            });
            let _ = self.fetch_and_emit_thread_metadata().await;
        }
        Ok(hydrated)
    }

    async fn create_root(&self, title: Option<&str>) -> io::Result<HydratedThread> {
        let created = self.client.create_root_thread(title).await.map_err(to_io)?;
        let hydrated = HydratedThread {
            id: created.thread_id,
            title: created.title,
            turns: Vec::new(),
        };
        if !self.has_live_stream.load(Ordering::SeqCst) {
            self.emit(TuiEngineEvent::ThreadCreated {
                thread: hydrated.clone(),
            });
            let _ = self.fetch_and_emit_thread_metadata().await;
        }
        Ok(hydrated)
    }

    async fn rename_thread(&self, thread_id: &str, title: &str) -> io::Result<()> {
        let renamed = self
            .client
            .rename_thread(thread_id, title)
            .await
            .map_err(to_io)?;
        if !self.has_live_stream.load(Ordering::SeqCst) {
            self.emit(TuiEngineEvent::ThreadTitleUpdated {
                thread_id: renamed.thread_id,
                title: renamed.title,
            });
        }
        Ok(())
    }

    async fn merge_thread(&self, source_id: &str, target_id: &str) -> io::Result<()> {
        self.merge_thread_with_summary(source_id, target_id, None)
            .await
    }

    async fn merge_thread_with_summary(
        &self,
        source_id: &str,
        target_id: &str,
        summary: Option<&str>,
    ) -> io::Result<()> {
        let target_opt = (!target_id.trim().is_empty()).then_some(target_id.trim());
        let merged = self
            .client
            .merge_thread_with_summary(source_id, target_opt, summary)
            .await
            .map_err(to_io)?;
        if !self.has_live_stream.load(Ordering::SeqCst) {
            let resolved_summary = summary
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|explicit| truncate_chars_with_ellipsis(explicit, 400))
                .unwrap_or_default();
            let meta_opt = self.fetch_and_emit_thread_metadata().await;
            let resolved_target = target_opt
                .filter(|t| *t != merged.thread_id)
                .map(str::to_string)
                .or_else(|| {
                    meta_opt.and_then(|meta| {
                        meta.into_iter()
                            .find(|m| m.thread_id == merged.thread_id)
                            .and_then(|m| m.parent_thread_id)
                    })
                })
                .unwrap_or_default();
            self.emit(TuiEngineEvent::ThreadMerged {
                thread_id: merged.thread_id,
                target_thread_id: resolved_target,
                summary: resolved_summary,
            });
        }
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

    async fn delete_thread(&self, thread_id: &str) -> Result<String, String> {
        self.client
            .delete_thread(thread_id)
            .await
            .map_err(|e| e.to_string())
    }

    async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, String> {
        self.client.thread_metadata().await.map_err(|e| {
            let msg = e.to_string();
            self.emit(TuiEngineEvent::Error {
                message: msg.clone(),
            });
            msg
        })
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
            let in_flight = Arc::clone(&self.in_flight_turns);
            let buffered = Arc::clone(&self.buffered_sync_errors);
            let suppressed = Arc::clone(&self.suppressed_sync_errors);
            tokio::spawn(async move {
                while let Some(ev) = engine_rx.recv().await {
                    if let TuiEngineEvent::Error { ref message } = ev {
                        let is_suppressed = {
                            let mut sup = suppressed.lock().unwrap_or_else(|p| p.into_inner());
                            let now = Instant::now();
                            sup.retain(|(_, ts)| now.duration_since(*ts) < Duration::from_secs(5));
                            if let Some(pos) = sup
                                .iter()
                                .position(|(err_msg, _)| error_matches_sync(err_msg, message))
                            {
                                sup.remove(pos);
                                true
                            } else {
                                false
                            }
                        };
                        if is_suppressed {
                            continue;
                        }
                        if in_flight.load(Ordering::SeqCst) > 0 {
                            let mut buf = buffered.lock().unwrap_or_else(|p| p.into_inner());
                            buf.push(message.clone());
                            continue;
                        }
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
    async fn test_engine_client_port_hydrates_main_thread_and_no_duplicate_errors() {
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
        assert!(
            forked.turns.is_empty(),
            "forked branch must hydrate only branch-local turns, got {:?}",
            forked.turns
        );
        let meta = port.thread_metadata().await.expect("thread_metadata");
        assert_eq!(meta.len(), 2);
        let fork_meta = meta
            .iter()
            .find(|m| m.thread_id == forked.id)
            .expect("fork meta");
        assert_eq!(fork_meta.parent_thread_id.as_deref(), Some(main_id.as_str()));
        assert_eq!(fork_meta.inherited_turn_count, 2);

        port.merge_thread(&forked.id, &main_id)
            .await
            .expect("merge_thread");
        port.resolve_permission("nonexistent-perm", true)
            .await
            .expect("resolve_permission");

        let next_active = port
            .delete_thread(&forked.id)
            .await
            .expect("delete_thread");
        assert_eq!(next_active, main_id);
        assert!(port.delete_thread(&main_id).await.is_err());

        let reopened =
            InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
                .expect("reopen");
        let port2 = EngineClientPort::new(reopened);
        let hydrated = port2.hydrate_session().await.expect("rehydrate");
        assert_eq!(hydrated.threads.len(), 1);
        assert_eq!(hydrated.threads[0].turns.len(), 3);
        assert_eq!(hydrated.threads[0].turns[0].content, "ping acp");
        assert_eq!(hydrated.threads[0].turns[1].content, "Mocked TUI Reply");
        assert!(hydrated.threads[0].turns[2]
            .content
            .contains("[Decision Landmark"));
    }

    #[tokio::test]
    async fn test_engine_client_port_forwards_async_background_errors() {
        use grein_cli::engine::{
            EngineError, EngineEvent, SessionSnapshot, ShellCommandResult, ThreadSummary,
            TranscriptEntry, TurnDispatchResult,
        };

        struct BgErrorEngine {
            rx: Mutex<Option<mpsc::UnboundedReceiver<TuiEngineEvent>>>,
        }

        #[async_trait::async_trait]
        impl DaemonEngine for BgErrorEngine {
            async fn ping(&self) -> Result<(), EngineError> {
                Ok(())
            }
            async fn ensure_main_thread(&self) -> Result<ThreadSummary, EngineError> {
                Ok(ThreadSummary {
                    thread_id: "t-1".into(),
                    title: "Main".into(),
                    parent_node_id: "n-1".into(),
                    active_leaf_id: "n-1".into(),
                    merged: false,
                    status: Default::default(),
                })
            }
            async fn send_turn(&self, _prompt: &str) -> Result<TurnDispatchResult, EngineError> {
                Ok(TurnDispatchResult {
                    node_id: "n-2".into(),
                })
            }
            async fn send_turn_for_thread(
                &self,
                _thread_id: &str,
                _prompt: &str,
            ) -> Result<TurnDispatchResult, EngineError> {
                Ok(TurnDispatchResult {
                    node_id: "n-2".into(),
                })
            }
            async fn fork_thread(
                &self,
                _parent_node_id: Option<&str>,
            ) -> Result<ThreadSummary, EngineError> {
                unreachable!()
            }
            async fn fork_thread_with_title(
                &self,
                _source: Option<&str>,
                _title: Option<&str>,
            ) -> Result<ThreadSummary, EngineError> {
                unreachable!()
            }
            async fn create_root_thread(
                &self,
                _title: Option<&str>,
            ) -> Result<ThreadSummary, EngineError> {
                unreachable!()
            }
            async fn rename_thread(
                &self,
                _thread_id: &str,
                _title: &str,
            ) -> Result<ThreadSummary, EngineError> {
                unreachable!()
            }
            async fn list_threads(&self) -> Result<Vec<ThreadSummary>, EngineError> {
                Ok(vec![self.ensure_main_thread().await?])
            }
            async fn get_transcript(&self) -> Result<Vec<TranscriptEntry>, EngineError> {
                Ok(Vec::new())
            }
            async fn get_thread_transcript(
                &self,
                _thread_id: &str,
            ) -> Result<Vec<TranscriptEntry>, EngineError> {
                Ok(Vec::new())
            }
            async fn merge_thread(&self, _thread_id: &str) -> Result<ThreadSummary, EngineError> {
                unreachable!()
            }
            async fn merge_thread_with_summary(
                &self,
                _source_id: &str,
                _target_id: Option<&str>,
                _summary: Option<&str>,
            ) -> Result<ThreadSummary, EngineError> {
                unreachable!()
            }
            async fn resolve_permission(
                &self,
                _decision: PermissionDecisionPayload,
            ) -> Result<EngineEvent, EngineError> {
                unreachable!()
            }
            async fn cancel_turn(&self, _thread_id: &str) -> Result<(), EngineError> {
                Ok(())
            }
            async fn run_shell_command(
                &self,
                _thread_id: &str,
                _command: &str,
            ) -> Result<ShellCommandResult, EngineError> {
                unreachable!()
            }
            async fn snapshot(&self) -> Result<SessionSnapshot, EngineError> {
                unreachable!()
            }
            async fn subscribe_events(
                &self,
            ) -> Result<Option<mpsc::UnboundedReceiver<TuiEngineEvent>>, EngineError> {
                Ok(self.rx.lock().await.take())
            }
        }

        let (tx, rx) = mpsc::unbounded_channel();
        let port = EngineClientPort::new(BgErrorEngine {
            rx: Mutex::new(Some(rx)),
        });
        let _ = port.hydrate_session().await.expect("hydrate");
        let mut sub_rx = port.subscribe_events().await.expect("subscribe");

        tx.send(TuiEngineEvent::Error {
            message: "async wakeup failed".to_string(),
        })
        .expect("send bg error");

        let mut saw_bg_err = false;
        while let Ok(Some(ev)) =
            tokio::time::timeout(std::time::Duration::from_millis(150), sub_rx.recv()).await
        {
            if matches!(ev, TuiEngineEvent::Error { ref message } if message == "async wakeup failed")
            {
                saw_bg_err = true;
                break;
            }
        }
        assert!(
            saw_bg_err,
            "async background TuiEngineEvent::Error must be forwarded to TUI"
        );
    }

    #[tokio::test]
    async fn test_engine_client_port_offline_fallbacks_and_metadata_error_forwarding() {
        use grein_cli::engine::{
            EngineError, EngineEvent, SessionSnapshot, ShellCommandResult, ThreadSummary,
            TranscriptEntry, TurnDispatchResult,
        };

        let engine = InProcessEngine::in_memory().expect("in_memory");
        let port = EngineClientPort::new(engine);
        let session = port.hydrate_session().await.expect("hydrate");
        let main_id = session.active_thread_id.clone();

        // Notice: subscribe_events NOT called -> has_live_stream == false!
        // Drain initial events via direct receiver? Actually subscribe_events on InProcessEngine
        // would flip has_live_stream to true. So test offline EngineClientPort with a no-stream
        // DaemonEngine wrapper or test create_root/rename/merge before subscribe_events!
        let root2 = port.create_root(Some("Offline Root")).await.expect("create_root");
        assert_eq!(root2.title, "Offline Root");
        port.rename_thread(&root2.id, "Renamed Offline Root")
            .await
            .expect("rename");
        let forked = port
            .fork_branch(&main_id, Some("Offline Branch"))
            .await
            .expect("fork_branch");
        port.merge_thread_with_summary(&forked.id, &main_id, Some("Offline decision"))
            .await
            .expect("merge_thread_with_summary");

        let mut rx = port.events_rx.lock().await.take().expect("events_rx");
        let mut offline_evs = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            offline_evs.push(ev);
        }
        assert!(offline_evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ThreadCreated { thread } if thread.id == root2.id
        )));
        assert!(offline_evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ThreadTitleUpdated { thread_id, title }
                if thread_id == &root2.id && title == "Renamed Offline Root"
        )));
        assert!(offline_evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ThreadCreated { thread } if thread.id == forked.id
        )));
        assert!(offline_evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::ThreadMerged { thread_id, target_thread_id, summary }
                if thread_id == &forked.id
                    && target_thread_id == &main_id
                    && summary == "Offline decision"
        )));

        struct FailingMetaEngine;
        #[async_trait::async_trait]
        impl DaemonEngine for FailingMetaEngine {
            async fn ping(&self) -> Result<(), EngineError> { Ok(()) }
            async fn ensure_main_thread(&self) -> Result<ThreadSummary, EngineError> {
                Ok(ThreadSummary {
                    thread_id: "t-1".into(),
                    title: "Main".into(),
                    parent_node_id: "n-1".into(),
                    active_leaf_id: "n-1".into(),
                    merged: false,
                    status: Default::default(),
                })
            }
            async fn send_turn(&self, _: &str) -> Result<TurnDispatchResult, EngineError> { unreachable!() }
            async fn send_turn_for_thread(&self, _: &str, _: &str) -> Result<TurnDispatchResult, EngineError> { unreachable!() }
            async fn fork_thread(&self, _: Option<&str>) -> Result<ThreadSummary, EngineError> { unreachable!() }
            async fn fork_thread_with_title(&self, _: Option<&str>, _: Option<&str>) -> Result<ThreadSummary, EngineError> { unreachable!() }
            async fn create_root_thread(&self, _: Option<&str>) -> Result<ThreadSummary, EngineError> { unreachable!() }
            async fn rename_thread(&self, _: &str, _: &str) -> Result<ThreadSummary, EngineError> { unreachable!() }
            async fn list_threads(&self) -> Result<Vec<ThreadSummary>, EngineError> {
                Ok(vec![self.ensure_main_thread().await?])
            }
            async fn get_transcript(&self) -> Result<Vec<TranscriptEntry>, EngineError> { Ok(Vec::new()) }
            async fn get_thread_transcript(&self, _: &str) -> Result<Vec<TranscriptEntry>, EngineError> { Ok(Vec::new()) }
            async fn merge_thread(&self, _: &str) -> Result<ThreadSummary, EngineError> { unreachable!() }
            async fn merge_thread_with_summary(&self, _: &str, _: Option<&str>, _: Option<&str>) -> Result<ThreadSummary, EngineError> { unreachable!() }
            async fn resolve_permission(&self, _: PermissionDecisionPayload) -> Result<EngineEvent, EngineError> { unreachable!() }
            async fn cancel_turn(&self, _: &str) -> Result<(), EngineError> { Ok(()) }
            async fn run_shell_command(&self, _: &str, _: &str) -> Result<ShellCommandResult, EngineError> { unreachable!() }
            async fn snapshot(&self) -> Result<SessionSnapshot, EngineError> { unreachable!() }
            async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, EngineError> {
                Err(EngineError::Storage("metadata db failure".to_string()))
            }
        }

        let err_port = EngineClientPort::new(FailingMetaEngine);
        let mut err_rx = err_port.subscribe_events().await.expect("subscribe");
        let _ = err_port.hydrate_session().await.expect("hydrate succeeds");
        assert!(err_port.thread_metadata().await.is_err());

        let mut err_evs = Vec::new();
        while let Ok(ev) = err_rx.try_recv() {
            err_evs.push(ev);
        }
        assert!(
            err_evs
                .iter()
                .filter(|e| matches!(e, TuiEngineEvent::Error { message } if message.contains("metadata db failure")))
                .count()
                >= 2,
            "thread_metadata Err must emit TuiEngineEvent::Error instead of ignoring: {err_evs:?}"
        );
    }
}
