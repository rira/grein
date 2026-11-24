use std::sync::Arc;

use grein_acp::{PermissionDecision, SessionId as AcpSessionId, ThreadId as AcpThreadId};
use grein_core::model::{AuthorRole, ConversationNode, NodeId, ThreadId, ThreadKind, ThreadStatus};
use grein_core::port::dto::EngineStreamEvent;
use tokio::sync::oneshot;

use super::super::ports::{AgentTurnPort, ThreadUseCasePort};
use super::super::types::{
    parse_typed_id, select_permission_option, storage_err, EngineError, EngineEvent,
    PermissionDecisionPayload, ShellCommandResult, TurnDispatchResult, SHELL_COMMAND_TIMEOUT,
    SHELL_MAX_OUTPUT_BYTES,
};
use super::{ActiveSessionHandle, InProcessEngine, SessionListenerCmd};

impl InProcessEngine {
    pub(super) async fn send_turn_impl(
        &self,
        prompt: &str,
    ) -> Result<TurnDispatchResult, EngineError> {
        if !self.runtime.has_any_available_provider() {
            let mut state = self.runtime.lock()?;
            let threads = self.runtime.storage.list_threads().map_err(storage_err)?;
            let target_agent = threads.iter().find(|t| t.kind == ThreadKind::Agent);
            if target_agent.is_none() && !threads.is_empty() {
                return Err(EngineError::InvalidParams(
                    "cannot send agent prompt to a terminal thread".to_string(),
                ));
            }
            let parent = target_agent.map(|t| t.active_leaf_id).or(state.last_node);
            let node_id = NodeId::new();
            let node = ConversationNode::new(node_id, parent, AuthorRole::User, prompt.to_string());
            self.runtime
                .storage
                .insert_node(&node)
                .map_err(storage_err)?;
            if let Some(agent_thread) = target_agent {
                self.runtime
                    .storage
                    .update_thread_leaf(&agent_thread.id, &node_id)
                    .map_err(storage_err)?;
            }
            state.last_node = Some(node_id);
            return Ok(TurnDispatchResult {
                node_id: node_id.0.to_string(),
            });
        }

        let main = ThreadUseCasePort::ensure_main_thread(self).await?;
        AgentTurnPort::send_turn_for_thread(self, &main.thread_id, prompt).await
    }

    pub(super) async fn send_turn_for_thread_impl(
        &self,
        thread_id: &str,
        prompt: &str,
    ) -> Result<TurnDispatchResult, EngineError> {
        if prompt.is_empty() {
            return Err(EngineError::InvalidParams(
                "prompt must not be empty".to_string(),
            ));
        }

        let tid: ThreadId = parse_typed_id(thread_id)
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        let existing_thread = self
            .runtime
            .storage
            .get_thread(&tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        if existing_thread.kind == ThreadKind::Terminal {
            return Err(EngineError::InvalidParams(
                "cannot send agent prompt to a terminal thread".to_string(),
            ));
        }

        let _turn_guard = self.runtime.acquire_thread_turn_lock(thread_id).await?;

        let existing_cmd_tx = {
            let state = self.runtime.lock()?;
            state
                .active_acp_sessions
                .get(thread_id)
                .map(|s| s.cmd_tx.clone())
        };
        if let Some(cmd_tx) = existing_cmd_tx {
            let (reply_tx, reply_rx) = oneshot::channel();
            if cmd_tx
                .send(SessionListenerCmd::PrepareSolicited { reply_tx })
                .is_ok()
            {
                let _ = reply_rx.await;
            }
        }

        let thread = self
            .runtime
            .storage
            .get_thread(&tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;

        let user_node_id = NodeId::new();
        let user_node = ConversationNode::new(
            user_node_id,
            Some(thread.active_leaf_id),
            AuthorRole::User,
            prompt.to_string(),
        );
        self.runtime
            .storage
            .insert_node(&user_node)
            .map_err(storage_err)?;
        self.runtime
            .storage
            .update_thread_leaf(&tid, &user_node_id)
            .map_err(storage_err)?;
        self.runtime
            .storage
            .update_thread_status(&tid, ThreadStatus::Streaming)
            .map_err(storage_err)?;
        {
            let mut state = self.runtime.lock()?;
            state.last_node = Some(user_node_id);
        }

        self.runtime.emit_tui(EngineStreamEvent::ThreadStatusChanged {
            thread_id: thread_id.to_string(),
            status: "streaming".to_string(),
        });

        let provider_opt = match self.runtime.resolve_runtime_for_thread(&thread) {
            Ok(Some(provider)) => provider
                .acp_config(&self.runtime.cwd)
                .map(|cfg| (provider, cfg)),
            Ok(None) => None,
            Err(err) => {
                let msg = err.to_string();
                self.runtime.mark_thread_error(tid, thread_id, &msg);
                return Err(err);
            }
        };
        let Some((provider, cfg)) = provider_opt else {
            let msg = "no ACP agent configured".to_string();
            self.runtime.mark_thread_error(tid, thread_id, &msg);
            return Err(EngineError::Transport(msg));
        };

        let (driver, driver_gen) = match provider
            .ensure_driver(&cfg, &self.runtime.driver_gen_seq)
            .await
        {
            Ok(pair) => pair,
            Err(err) => {
                self.runtime
                    .mark_thread_error(tid, thread_id, &err.to_string());
                return Err(err);
            }
        };

        let existing_handle = {
            let mut state = self.runtime.lock()?;
            let stale = state.active_acp_sessions.get(thread_id).is_some_and(|s| {
                s.provider_id != provider.id
                    || s.driver_gen != driver_gen
                    || s.cmd_tx.is_closed()
            });
            if stale {
                state.active_acp_sessions.remove(thread_id);
            }
            state
                .active_acp_sessions
                .get(thread_id)
                .map(|s| ActiveSessionHandle {
                    session_id: s.session_id.clone(),
                    driver_gen: s.driver_gen,
                    cmd_tx: s.cmd_tx.clone(),
                })
        };

        let (session_handle, outgoing_prompt) = match existing_handle {
            Some(handle) => {
                let prompt_text = {
                    let mut state = self.runtime.lock()?;
                    let landmarks = state
                        .active_acp_sessions
                        .get_mut(thread_id)
                        .map(|entry| std::mem::take(&mut entry.pending_landmarks))
                        .unwrap_or_default();
                    if landmarks.is_empty() {
                        prompt.to_string()
                    } else {
                        format!(
                            "[Joined Context]\n{}\n\n---\n\n{prompt}",
                            landmarks.join("\n\n")
                        )
                    }
                };
                (handle, prompt_text)
            }
            None => {
                match self
                    .runtime
                    .ensure_acp_session_for_thread(
                        &provider,
                        &cfg,
                        tid,
                        thread_id,
                        user_node_id,
                        Some(user_node_id),
                        true,
                    )
                    .await
                {
                    Ok((_, handle)) => (handle, prompt.to_string()),
                    Err(err) => {
                        self.runtime
                            .mark_thread_error(tid, thread_id, &err.to_string());
                        return Err(err);
                    }
                }
            }
        };

        let (done_tx, mut done_rx) = oneshot::channel();
        let (ack_tx, ack_rx) = oneshot::channel();
        if session_handle
            .cmd_tx
            .send(SessionListenerCmd::StartSolicited {
                user_node_id,
                done_tx,
                ack_tx,
            })
            .is_err()
            || ack_rx.await.is_err()
        {
            let msg = "ACP session listener stopped before prompt dispatch".to_string();
            self.runtime.mark_thread_error(tid, thread_id, &msg);
            return Err(EngineError::Transport(msg));
        }

        let submit_res = tokio::select! {
            res = driver.submit_prompt(&session_handle.session_id, &outgoing_prompt) => Some(res),
            done = &mut done_rx => {
                match done {
                    Ok(Ok(leaf_id)) => {
                        return Ok(TurnDispatchResult {
                            node_id: leaf_id.0.to_string(),
                        });
                    }
                    Ok(Err(err)) => return Err(err),
                    Err(_) => {
                        let msg = "ACP session listener closed before PromptCompleted".to_string();
                        self.runtime.mark_thread_error(tid, thread_id, &msg);
                        return Err(EngineError::Transport(msg));
                    }
                }
            }
        };

        if let Some(Err(err)) = submit_res {
            provider
                .invalidate_driver_if_dead(&driver, session_handle.driver_gen, &self.runtime.state)
                .await;
            let (abort_ack_tx, abort_ack_rx) = oneshot::channel();
            let already_emitted = if session_handle
                .cmd_tx
                .send(SessionListenerCmd::AbortSolicited {
                    ack_tx: abort_ack_tx,
                })
                .is_ok()
            {
                abort_ack_rx.await.unwrap_or(false)
            } else {
                false
            };
            let msg = format!("ACP submit_prompt failed: {err}");
            if !already_emitted {
                self.runtime.mark_thread_error(tid, thread_id, &msg);
            }
            return Err(EngineError::Transport(msg));
        }

        match done_rx.await {
            Ok(Ok(leaf_id)) => Ok(TurnDispatchResult {
                node_id: leaf_id.0.to_string(),
            }),
            Ok(Err(err)) => Err(err),
            Err(_) => {
                provider
                    .invalidate_driver_if_dead(
                        &driver,
                        session_handle.driver_gen,
                        &self.runtime.state,
                    )
                    .await;
                let msg = "ACP stream closed before PromptCompleted".to_string();
                self.runtime.mark_thread_error(tid, thread_id, &msg);
                Err(EngineError::Transport(msg))
            }
        }
    }

    pub(super) async fn resolve_permission_impl(
        &self,
        decision: PermissionDecisionPayload,
    ) -> Result<EngineEvent, EngineError> {
        let pending = {
            let mut state = self.runtime.lock()?;
            state.decisions.push(decision.clone());
            state.pending_permissions.remove(&decision.request_id)
        };
        if let Some(record) = pending {
            if let Some(policy) = self.runtime.policy.as_ref() {
                let recorded = if decision.allow {
                    PermissionDecision::AllowOnce
                } else {
                    PermissionDecision::Deny
                };
                policy.record_decision(
                    &AcpSessionId::new(&record.session_id),
                    &AcpThreadId::new(&record.thread_id),
                    &record.request,
                    recorded,
                );
            }
            let option_id = select_permission_option(&record.options, decision.allow);
            record
                .driver
                .respond_permission(record.json_rpc_id, Some(&option_id))
                .await
                .map_err(|e| EngineError::Transport(format!("respond_permission failed: {e}")))?;
        }
        Ok(EngineEvent::PermissionResolved {
            request_id: decision.request_id,
            allow: decision.allow,
        })
    }

    pub(super) async fn cancel_turn_impl(&self, thread_id: &str) -> Result<(), EngineError> {
        let session_info = {
            let state = self.runtime.lock()?;
            state
                .active_acp_sessions
                .get(thread_id)
                .map(|s| (s.session_id.clone(), s.provider_id.clone()))
        };
        let Some((sid, provider_id)) = session_info else {
            return Ok(());
        };
        if let Some(provider) = self.runtime.providers.get(&provider_id) {
            provider.cancel_session(&sid).await?;
        }
        Ok(())
    }

    pub(super) async fn run_shell_command_impl(
        &self,
        thread_id: &str,
        command: &str,
    ) -> Result<ShellCommandResult, EngineError> {
        if command.trim().is_empty() {
            return Err(EngineError::InvalidParams(
                "command must not be empty".to_string(),
            ));
        }
        let cwd = parse_typed_id::<ThreadId>(thread_id)
            .as_ref()
            .and_then(|tid| self.runtime.storage.get_thread(tid).ok().flatten())
            .map(|t| self.runtime.resolve_thread_cwd(&t))
            .unwrap_or_else(|| self.runtime.cwd.clone());
        let cmd = command.to_string();
        let shell = Arc::clone(&self.runtime.shell);
        let result = tokio::task::spawn_blocking(move || {
            shell.run_shell_command(&cwd, &cmd, SHELL_COMMAND_TIMEOUT, SHELL_MAX_OUTPUT_BYTES)
        })
        .await
        .map_err(|e| EngineError::Transport(format!("shell task panicked: {e}")))?
        .map_err(|e| EngineError::Transport(format!("shell execution failed: {e}")))?;

        self.runtime.emit_tui(EngineStreamEvent::ShellOutput {
            thread_id: thread_id.to_string(),
            command: result.command.clone(),
            stdout: result.stdout.clone(),
            stderr: result.stderr.clone(),
            exit_code: result.exit_code,
        });
        Ok(result)
    }
}
