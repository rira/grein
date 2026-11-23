use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};

use super::lock;
use super::parser::{
    build_prompt_completed, format_rpc_error, parse_config_options_result, parse_initialize_result,
    parse_permission_request, parse_session_info, translate_session_update,
};
use super::process::StdioProcessRunner;
use super::protocol::{
    AcpConfigOption, AcpInitializeResult, AcpSessionInfo, AgentDriver, DriverEvent, JsonRpcMessage,
    JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, StdioRunnerError, JSONRPC_VERSION,
};

const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

struct PendingRpc {
    tx: oneshot::Sender<Result<serde_json::Value, String>>,
    persistent_prompt_session: Option<String>,
}

type PendingRpcMap = Arc<Mutex<HashMap<u64, PendingRpc>>>;
type SessionSinkMap = Arc<Mutex<HashMap<String, mpsc::UnboundedSender<DriverEvent>>>>;

/// AgentDriver over an ACP v1 child process speaking NDJSON JSON-RPC 2.0 on stdio.
/// Must be constructed inside a tokio runtime.
pub struct AcpStdioDriver {
    runner: StdioProcessRunner,
    sender: Arc<Mutex<Option<mpsc::Sender<JsonRpcMessage>>>>,
    pending_rpc: PendingRpcMap,
    session_sinks: SessionSinkMap,
    persistent_sinks: SessionSinkMap,
    next_id: AtomicU64,
}

impl AcpStdioDriver {
    pub fn new(mut runner: StdioProcessRunner) -> Self {
        let messages = runner
            .take_raw_rx()
            .unwrap_or_else(|| mpsc::unbounded_channel().1);
        let sender = Arc::new(Mutex::new(runner.sender()));
        let pending_rpc: PendingRpcMap = Arc::new(Mutex::new(HashMap::new()));
        let session_sinks: SessionSinkMap = Arc::new(Mutex::new(HashMap::new()));
        let persistent_sinks: SessionSinkMap = Arc::new(Mutex::new(HashMap::new()));

        tokio::spawn(pump(
            messages,
            Arc::clone(&sender),
            Arc::clone(&pending_rpc),
            Arc::clone(&session_sinks),
            Arc::clone(&persistent_sinks),
        ));

        Self {
            runner,
            sender,
            pending_rpc,
            session_sinks,
            persistent_sinks,
            next_id: AtomicU64::new(1),
        }
    }

    pub fn is_running(&self) -> bool {
        self.runner.is_running()
    }

    pub fn recent_stderr(&self) -> Vec<String> {
        self.runner.recent_stderr()
    }

    fn current_sender(&self) -> Result<mpsc::Sender<JsonRpcMessage>, StdioRunnerError> {
        lock(&self.sender)
            .clone()
            .ok_or_else(|| StdioRunnerError::ProcessTerminated("driver is shut down".into()))
    }

    fn register_pending_rpc(
        &self,
        persistent_prompt_session: Option<String>,
    ) -> (u64, oneshot::Receiver<Result<serde_json::Value, String>>) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        lock(&self.pending_rpc).insert(
            id,
            PendingRpc {
                tx,
                persistent_prompt_session,
            },
        );
        (id, rx)
    }

    async fn rpc_call_inner(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
        persistent_prompt_session: Option<String>,
    ) -> Result<serde_json::Value, StdioRunnerError> {
        let sender = self.current_sender()?;
        let (id, rx) = self.register_pending_rpc(persistent_prompt_session);

        let request = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: serde_json::json!(id),
            method: method.to_string(),
            params,
        });

        if sender.send(request).await.is_err() {
            lock(&self.pending_rpc).remove(&id);
            return Err(StdioRunnerError::CommunicationError(
                "agent input channel closed".into(),
            ));
        }

        match rx.await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(message)) => {
                if !self.runner.is_running() {
                    Err(StdioRunnerError::ProcessTerminated(message))
                } else {
                    Err(StdioRunnerError::CommunicationError(message))
                }
            }
            Err(_) => Err(StdioRunnerError::ProcessTerminated(
                "agent process closed before replying".into(),
            )),
        }
    }

    async fn rpc_call(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, StdioRunnerError> {
        self.rpc_call_inner(method, params, None).await
    }

    pub async fn initialize(&self) -> Result<AcpInitializeResult, StdioRunnerError> {
        let params = serde_json::json!({
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": {
                "name": "grein",
                "version": env!("CARGO_PKG_VERSION"),
            }
        });
        let result = self.rpc_call("initialize", Some(params)).await?;
        parse_initialize_result(&result)
    }

    pub async fn authenticate(&self, method_id: &str) -> Result<(), StdioRunnerError> {
        let params = serde_json::json!({ "methodId": method_id });
        self.rpc_call("authenticate", Some(params)).await?;
        Ok(())
    }

    pub async fn new_session(
        &self,
        cwd: impl AsRef<Path>,
        system_prompt_append: Option<&str>,
    ) -> Result<AcpSessionInfo, StdioRunnerError> {
        let cwd_str = cwd.as_ref().to_string_lossy().into_owned();
        let mut params = serde_json::json!({
            "cwd": cwd_str,
            "mcpServers": [],
        });
        if let Some(append) = system_prompt_append.filter(|s| !s.is_empty()) {
            if let Some(obj) = params.as_object_mut() {
                obj.insert(
                    "_meta".to_string(),
                    serde_json::json!({
                        "antigravity": {
                            "systemPrompt": {
                                "append": append,
                            }
                        }
                    }),
                );
            }
        }
        let result = self.rpc_call("session/new", Some(params)).await?;
        parse_session_info(&result)
    }

    pub fn subscribe_session(&self, session_id: &str) -> mpsc::UnboundedReceiver<DriverEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        let sender_guard = lock(&self.sender);
        if sender_guard.is_some() {
            lock(&self.persistent_sinks).insert(session_id.to_string(), tx);
        }
        rx
    }

    pub async fn submit_prompt(
        &self,
        session_id: &str,
        prompt: &str,
    ) -> Result<(), StdioRunnerError> {
        let params = serde_json::json!({
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": prompt}],
        });
        self.rpc_call_inner("session/prompt", Some(params), Some(session_id.to_string()))
            .await?;
        Ok(())
    }

    pub async fn send_prompt(
        &self,
        session_id: &str,
        prompt: &str,
    ) -> Result<mpsc::UnboundedReceiver<DriverEvent>, StdioRunnerError> {
        let sender = self.current_sender()?;
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let session_id_owned = session_id.to_string();
        lock(&self.session_sinks).insert(session_id_owned.clone(), event_tx.clone());
        let (id, rpc_rx) = self.register_pending_rpc(None);

        let request = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: serde_json::json!(id),
            method: "session/prompt".to_string(),
            params: Some(serde_json::json!({
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": prompt}],
            })),
        });

        if sender.send(request).await.is_err() {
            lock(&self.pending_rpc).remove(&id);
            remove_sink_if_same(&self.session_sinks, &session_id_owned, &event_tx);
            return Err(StdioRunnerError::CommunicationError(
                "agent input channel closed".into(),
            ));
        }

        let sinks_for_waiter = Arc::clone(&self.session_sinks);
        tokio::spawn(async move {
            let terminal_event = match rpc_rx.await {
                Ok(Ok(result)) => build_prompt_completed(&session_id_owned, &result),
                Ok(Err(message)) => DriverEvent::Error {
                    session_id: Some(session_id_owned.clone()),
                    message,
                },
                Err(_) => DriverEvent::Error {
                    session_id: Some(session_id_owned.clone()),
                    message: "agent process closed before completing prompt".to_string(),
                },
            };
            let _ = event_tx.send(terminal_event);
            remove_sink_if_same(&sinks_for_waiter, &session_id_owned, &event_tx);
        });

        Ok(event_rx)
    }

    pub async fn respond_permission(
        &self,
        request_id: serde_json::Value,
        option_id: Option<&str>,
    ) -> Result<(), StdioRunnerError> {
        let sender = self.current_sender()?;
        let result = match option_id {
            Some(opt) => serde_json::json!({
                "outcome": {
                    "outcome": "selected",
                    "optionId": opt,
                }
            }),
            None => serde_json::json!({
                "outcome": {
                    "outcome": "cancelled",
                }
            }),
        };
        let response = JsonRpcMessage::Response(JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: request_id,
            result,
        });
        sender
            .send(response)
            .await
            .map_err(|_| StdioRunnerError::CommunicationError("agent input channel closed".into()))
    }

    pub async fn cancel_session(&self, session_id: &str) -> Result<(), StdioRunnerError> {
        let sender = self.current_sender()?;
        let notification = JsonRpcMessage::Notification(JsonRpcNotification {
            jsonrpc: JSONRPC_VERSION.to_string(),
            method: "session/cancel".to_string(),
            params: Some(serde_json::json!({ "sessionId": session_id })),
        });
        sender
            .send(notification)
            .await
            .map_err(|_| StdioRunnerError::CommunicationError("agent input channel closed".into()))
    }

    pub async fn close_session(&self, session_id: &str) -> Result<(), StdioRunnerError> {
        let params = serde_json::json!({ "sessionId": session_id });
        let res = self.rpc_call("session/close", Some(params)).await;
        lock(&self.persistent_sinks).remove(session_id);
        res?;
        Ok(())
    }

    pub async fn set_session_model(
        &self,
        session_id: &str,
        model_id: &str,
    ) -> Result<(), StdioRunnerError> {
        let params = serde_json::json!({
            "sessionId": session_id,
            "modelId": model_id,
        });
        self.rpc_call("session/set_model", Some(params)).await?;
        Ok(())
    }

    pub async fn set_session_mode(
        &self,
        session_id: &str,
        mode_id: &str,
    ) -> Result<(), StdioRunnerError> {
        let params = serde_json::json!({
            "sessionId": session_id,
            "modeId": mode_id,
        });
        self.rpc_call("session/set_mode", Some(params)).await?;
        Ok(())
    }

    pub async fn set_config_option(
        &self,
        session_id: &str,
        config_id: &str,
        value: &str,
    ) -> Result<Vec<AcpConfigOption>, StdioRunnerError> {
        let params = serde_json::json!({
            "sessionId": session_id,
            "configId": config_id,
            "value": value,
        });
        let result = self
            .rpc_call("session/set_config_option", Some(params))
            .await?;
        parse_config_options_result(&result)
    }

    pub async fn shutdown(&self) -> Result<(), StdioRunnerError> {
        {
            let mut sender_guard = lock(&self.sender);
            sender_guard.take();
            lock(&self.persistent_sinks).clear();
        }
        let res = match self.runner.terminate(SHUTDOWN_GRACE).await {
            Ok(_) => Ok(()),
            Err(StdioRunnerError::ProcessTerminated(_)) => Ok(()),
            Err(err) => Err(err),
        };
        fail_pending_rpcs(&self.pending_rpc, "driver is shut down");
        res
    }
}

#[async_trait]
impl AgentDriver for AcpStdioDriver {
    async fn initialize(&self) -> Result<AcpInitializeResult, StdioRunnerError> {
        AcpStdioDriver::initialize(self).await
    }

    async fn authenticate(&self, method_id: &str) -> Result<(), StdioRunnerError> {
        AcpStdioDriver::authenticate(self, method_id).await
    }

    async fn new_session(
        &self,
        cwd: &Path,
        system_prompt_append: Option<&str>,
    ) -> Result<AcpSessionInfo, StdioRunnerError> {
        AcpStdioDriver::new_session(self, cwd, system_prompt_append).await
    }

    async fn send_prompt(
        &self,
        session_id: &str,
        prompt: &str,
    ) -> Result<mpsc::UnboundedReceiver<DriverEvent>, StdioRunnerError> {
        AcpStdioDriver::send_prompt(self, session_id, prompt).await
    }

    async fn respond_permission(
        &self,
        request_id: serde_json::Value,
        option_id: Option<&str>,
    ) -> Result<(), StdioRunnerError> {
        AcpStdioDriver::respond_permission(self, request_id, option_id).await
    }

    async fn cancel_session(&self, session_id: &str) -> Result<(), StdioRunnerError> {
        AcpStdioDriver::cancel_session(self, session_id).await
    }

    async fn close_session(&self, session_id: &str) -> Result<(), StdioRunnerError> {
        AcpStdioDriver::close_session(self, session_id).await
    }

    fn recent_stderr(&self) -> Vec<String> {
        AcpStdioDriver::recent_stderr(self)
    }

    fn subscribe_session(&self, session_id: &str) -> mpsc::UnboundedReceiver<DriverEvent> {
        AcpStdioDriver::subscribe_session(self, session_id)
    }

    async fn submit_prompt(&self, session_id: &str, prompt: &str) -> Result<(), StdioRunnerError> {
        AcpStdioDriver::submit_prompt(self, session_id, prompt).await
    }

    async fn set_session_model(
        &self,
        session_id: &str,
        model_id: &str,
    ) -> Result<(), StdioRunnerError> {
        AcpStdioDriver::set_session_model(self, session_id, model_id).await
    }

    async fn set_session_mode(
        &self,
        session_id: &str,
        mode_id: &str,
    ) -> Result<(), StdioRunnerError> {
        AcpStdioDriver::set_session_mode(self, session_id, mode_id).await
    }

    async fn set_config_option(
        &self,
        session_id: &str,
        config_id: &str,
        value: &str,
    ) -> Result<Vec<AcpConfigOption>, StdioRunnerError> {
        AcpStdioDriver::set_config_option(self, session_id, config_id, value).await
    }

    async fn shutdown(&self) -> Result<(), StdioRunnerError> {
        AcpStdioDriver::shutdown(self).await
    }
}

fn remove_sink_if_same(
    sinks: &SessionSinkMap,
    session_id: &str,
    expected: &mpsc::UnboundedSender<DriverEvent>,
) {
    let mut guard = lock(sinks);
    if guard
        .get(session_id)
        .is_some_and(|existing| existing.same_channel(expected))
    {
        guard.remove(session_id);
    }
}

fn fail_pending_rpcs(pending_rpc: &PendingRpcMap, reason: &str) {
    let entries: Vec<_> = lock(pending_rpc).drain().map(|(_, entry)| entry).collect();
    for entry in entries {
        let _ = entry.tx.send(Err(reason.to_string()));
    }
}

fn send_or_prune(
    sinks: &SessionSinkMap,
    session_id: &str,
    tx: &mpsc::UnboundedSender<DriverEvent>,
    event: DriverEvent,
) {
    if tx.send(event).is_err() {
        remove_sink_if_same(sinks, session_id, tx);
    }
}

fn dispatch_to_session(sinks: &SessionSinkMap, session_id: &str, event: DriverEvent) {
    let tx = lock(sinks).get(session_id).cloned();
    if let Some(tx) = tx {
        send_or_prune(sinks, session_id, &tx, event);
    }
}

fn dispatch_to_both_sinks(
    session_sinks: &SessionSinkMap,
    persistent_sinks: &SessionSinkMap,
    session_id: &str,
    event: DriverEvent,
) {
    let persistent_tx = lock(persistent_sinks).get(session_id).cloned();
    let session_tx = lock(session_sinks).get(session_id).cloned();
    match (persistent_tx, session_tx) {
        (Some(ptx), Some(stx)) => {
            send_or_prune(persistent_sinks, session_id, &ptx, event.clone());
            send_or_prune(session_sinks, session_id, &stx, event);
        }
        (Some(ptx), None) => send_or_prune(persistent_sinks, session_id, &ptx, event),
        (None, Some(stx)) => send_or_prune(session_sinks, session_id, &stx, event),
        (None, None) => {}
    }
}

async fn pump(
    mut messages: mpsc::UnboundedReceiver<JsonRpcMessage>,
    sender: Arc<Mutex<Option<mpsc::Sender<JsonRpcMessage>>>>,
    pending_rpc: PendingRpcMap,
    session_sinks: SessionSinkMap,
    persistent_sinks: SessionSinkMap,
) {
    while let Some(msg) = messages.recv().await {
        match msg {
            JsonRpcMessage::Notification(n) => {
                if let Some((session_id, event)) = translate_session_update(&n) {
                    dispatch_to_both_sinks(&session_sinks, &persistent_sinks, &session_id, event);
                }
            }
            JsonRpcMessage::Request(req) => {
                if let Some((session_id, event)) = parse_permission_request(&req) {
                    dispatch_to_both_sinks(&session_sinks, &persistent_sinks, &session_id, event);
                } else {
                    tracing::debug!("ignoring server-initiated request: {}", req.method);
                }
            }
            JsonRpcMessage::Response(resp) => {
                if let Some(id) = resp.id.as_u64() {
                    if let Some(pending) = lock(&pending_rpc).remove(&id) {
                        if let Some(session_id) = pending.persistent_prompt_session {
                            let completed = build_prompt_completed(&session_id, &resp.result);
                            dispatch_to_session(&persistent_sinks, &session_id, completed);
                        }
                        let _ = pending.tx.send(Ok(resp.result));
                    }
                }
            }
            JsonRpcMessage::Error(err) => {
                let message = format_rpc_error(&err.error);
                if let Some(id) = err.id.as_ref().and_then(|v| v.as_u64()) {
                    if let Some(pending) = lock(&pending_rpc).remove(&id) {
                        let _ = pending.tx.send(Err(message));
                        continue;
                    }
                }
                let mut sinks: Vec<_> = lock(&persistent_sinks).values().cloned().collect();
                sinks.extend(lock(&session_sinks).values().cloned());
                for tx in sinks {
                    let _ = tx.send(DriverEvent::Error {
                        session_id: None,
                        message: message.clone(),
                    });
                }
            }
        }
    }

    {
        let mut sender_guard = lock(&sender);
        sender_guard.take();
        lock(&persistent_sinks).clear();
    }
    fail_pending_rpcs(&pending_rpc, "agent process closed its output");
}
