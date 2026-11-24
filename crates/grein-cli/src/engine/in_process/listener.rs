use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use grein_acp::{
    AgentDriver, DriverEvent, PolicyDecision, SessionId as AcpSessionId,
    ThreadId as AcpThreadId, ToolInvocationRequest,
};
use grein_core::model::{NodeId, ThreadId, ThreadStatus};
use grein_core::port::dto::{
    AgentConfigOption, AgentSlashCommand, EngineStreamEvent, SubagentSnapshot, SubagentStatus,
};
use tokio::sync::{mpsc, oneshot};

use super::super::types::{
    acp_config_option_to_core, extract_model_and_mode_from_options, extract_subagent_role,
    extract_subagent_type, extract_tool_call_summary, format_permission_description,
    merge_updated_config_options, now_epoch_ms, select_permission_option, EngineError,
};
use super::{EngineRuntime, PendingPermissionRecord, SessionListenerCmd, TrackedLayer1Subagent};

fn finalize_eligible_subagents(
    runtime: &EngineRuntime,
    thread_id: &str,
    active_subagents: &mut HashMap<String, TrackedLayer1Subagent>,
    current_turn_seq: u64,
    current_wakeup_seq: u64,
    is_unsolicited: bool,
) {
    let watch = runtime.subagent_observer.as_ref();
    let eligible_ids: Vec<String> = active_subagents
        .iter()
        .filter(|(_, sa)| {
            if is_unsolicited {
                !(sa.launched_unsolicited && sa.launched_wakeup_seq == current_wakeup_seq)
            } else {
                sa.launched_unsolicited || sa.launched_turn_seq < current_turn_seq
            }
        })
        .map(|(id, _)| id.clone())
        .collect();

    for id in eligible_ids {
        if !watch.is_confirmed_running(thread_id, &id) {
            watch.mark_terminal(
                thread_id,
                &id,
                SubagentStatus::Completed,
                Some("done".to_string()),
            );
            active_subagents.remove(&id);
        }
    }
}

fn is_thread_in_isolated_worktree(runtime: &EngineRuntime, tid: ThreadId) -> bool {
    let Ok(Some(thread)) = runtime.storage.get_thread(&tid) else {
        return false;
    };
    let Some(raw) = thread.cwd.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    let path = Path::new(raw);
    if let Some(status) = runtime.git_inspector.inspect_git_status(path) {
        if status.is_worktree {
            return true;
        }
    }
    raw.contains("/.grein/worktrees/")
        || raw.contains("\\.grein\\worktrees\\")
        || raw.contains("/.grein/shadow/")
        || raw.contains("\\.grein\\shadow\\")
}

#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_session_listener(
    runtime: Arc<EngineRuntime>,
    tid: ThreadId,
    thread_id: String,
    session_id: String,
    driver: Arc<dyn AgentDriver>,
    initial_leaf_id: NodeId,
    mut events: mpsc::UnboundedReceiver<DriverEvent>,
    mut cmd_rx: mpsc::UnboundedReceiver<SessionListenerCmd>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut solicited_in_flight = false;
        let mut solicited_error_emitted = false;
        let mut solicited_turn_seq: u64 = 0;
        let mut wakeup_seq: u64 = 0;
        let mut wakeup_active = false;
        let mut solicited_done_tx: Option<oneshot::Sender<Result<NodeId, EngineError>>> = None;
        let mut accumulated_text = String::new();
        let mut current_leaf_id = initial_leaf_id;
        let mut unsolicited_streaming_emitted = false;
        let mut quiet_deadline: Option<tokio::time::Instant> = None;
        let mut active_tool_names: HashMap<String, String> = HashMap::new();
        let mut active_subagents: HashMap<String, TrackedLayer1Subagent> = HashMap::new();

        let finalize_unsolicited = |runtime: &EngineRuntime,
                                    current_leaf_id: &mut NodeId,
                                    accumulated_text: &mut String,
                                    unsolicited_streaming_emitted: &mut bool,
                                    quiet_deadline: &mut Option<tokio::time::Instant>,
                                    wakeup_active: &mut bool,
                                    active_subagents: &mut HashMap<String, TrackedLayer1Subagent>,
                                    solicited_turn_seq: u64,
                                    wakeup_seq: u64| {
            if !accumulated_text.trim().is_empty() {
                let text = std::mem::take(accumulated_text);
                if let Ok(new_leaf) = runtime.persist_assistant_node(tid, *current_leaf_id, text) {
                    *current_leaf_id = new_leaf;
                }
            } else {
                accumulated_text.clear();
            }
            let _ = runtime.storage.update_thread_status(&tid, ThreadStatus::Idle);
            runtime.emit_tui(EngineStreamEvent::TurnCompleted {
                thread_id: thread_id.clone(),
            });
            runtime.emit_tui(EngineStreamEvent::ThreadStatusChanged {
                thread_id: thread_id.clone(),
                status: "idle".to_string(),
            });
            *unsolicited_streaming_emitted = false;
            *quiet_deadline = None;
            *wakeup_active = false;
            finalize_eligible_subagents(
                runtime,
                &thread_id,
                active_subagents,
                solicited_turn_seq,
                wakeup_seq,
                true,
            );
        };

        loop {
            let sleep_fut = async {
                match quiet_deadline {
                    Some(dl) => tokio::time::sleep_until(dl).await,
                    None => std::future::pending::<()>().await,
                }
            };

            tokio::select! {
                cmd_opt = cmd_rx.recv() => {
                    let Some(cmd) = cmd_opt else {
                        break;
                    };
                    match cmd {
                        SessionListenerCmd::PrepareSolicited { reply_tx } => {
                            if !solicited_in_flight && !accumulated_text.trim().is_empty() {
                                finalize_unsolicited(
                                    &runtime,
                                    &mut current_leaf_id,
                                    &mut accumulated_text,
                                    &mut unsolicited_streaming_emitted,
                                    &mut quiet_deadline,
                                    &mut wakeup_active,
                                    &mut active_subagents,
                                    solicited_turn_seq,
                                    wakeup_seq,
                                );
                            } else {
                                quiet_deadline = None;
                                wakeup_active = false;
                            }
                            if let Ok(Some(t)) = runtime.storage.get_thread(&tid) {
                                current_leaf_id = t.active_leaf_id;
                            }
                            let _ = reply_tx.send(current_leaf_id);
                        }
                        SessionListenerCmd::StartSolicited { user_node_id, done_tx, ack_tx } => {
                            solicited_in_flight = true;
                            solicited_error_emitted = false;
                            solicited_turn_seq += 1;
                            current_leaf_id = user_node_id;
                            accumulated_text.clear();
                            unsolicited_streaming_emitted = false;
                            quiet_deadline = None;
                            wakeup_active = false;
                            solicited_done_tx = Some(done_tx);
                            let _ = ack_tx.send(());
                        }
                        SessionListenerCmd::AbortSolicited { ack_tx } => {
                            solicited_in_flight = false;
                            solicited_done_tx = None;
                            accumulated_text.clear();
                            unsolicited_streaming_emitted = false;
                            quiet_deadline = None;
                            wakeup_active = false;
                            let already_emitted = solicited_error_emitted;
                            solicited_error_emitted = false;
                            let _ = ack_tx.send(already_emitted);
                        }
                    }
                }
                ev_opt = events.recv() => {
                    let Some(ev) = ev_opt else {
                        if solicited_in_flight {
                            let msg = "ACP stream closed before PromptCompleted".to_string();
                            runtime.mark_thread_error(tid, &thread_id, &msg);
                            if let Some(tx) = solicited_done_tx.take() {
                                let _ = tx.send(Err(EngineError::Transport(msg)));
                            }
                        } else if !accumulated_text.trim().is_empty() {
                            finalize_unsolicited(
                                &runtime,
                                &mut current_leaf_id,
                                &mut accumulated_text,
                                &mut unsolicited_streaming_emitted,
                                &mut quiet_deadline,
                                &mut wakeup_active,
                                &mut active_subagents,
                                solicited_turn_seq,
                                wakeup_seq,
                            );
                        }
                        break;
                    };

                    match ev {
                        DriverEvent::StreamChunk { text, .. } => {
                            if !solicited_in_flight {
                                if !wakeup_active {
                                    wakeup_active = true;
                                    wakeup_seq += 1;
                                }
                                if !unsolicited_streaming_emitted {
                                    unsolicited_streaming_emitted = true;
                                    let _ = runtime.storage.update_thread_status(&tid, ThreadStatus::Streaming);
                                    runtime.emit_tui(EngineStreamEvent::ThreadStatusChanged {
                                        thread_id: thread_id.clone(),
                                        status: "streaming".to_string(),
                                    });
                                }
                                quiet_deadline = Some(tokio::time::Instant::now() + runtime.quiet_timeout());
                            }
                            accumulated_text.push_str(&text);
                            runtime.emit_tui(EngineStreamEvent::StreamChunk {
                                thread_id: thread_id.clone(),
                                content: text,
                            });
                        }
                        DriverEvent::ThoughtChunk { text, .. } => {
                            if !solicited_in_flight {
                                if !wakeup_active {
                                    wakeup_active = true;
                                    wakeup_seq += 1;
                                }
                                if quiet_deadline.is_some() {
                                    quiet_deadline = Some(tokio::time::Instant::now() + runtime.quiet_timeout());
                                }
                            }
                            runtime.emit_tui(EngineStreamEvent::ThoughtChunk {
                                thread_id: thread_id.clone(),
                                content: text,
                            });
                        }
                        DriverEvent::ToolCall { tool_call_id, name, title, args, .. } => {
                            if !solicited_in_flight {
                                if !wakeup_active {
                                    wakeup_active = true;
                                    wakeup_seq += 1;
                                }
                                if quiet_deadline.is_some() {
                                    quiet_deadline = Some(tokio::time::Instant::now() + runtime.quiet_timeout());
                                }
                            }
                            let tool_name = if !name.is_empty() {
                                name.clone()
                            } else if !title.is_empty() {
                                title.clone()
                            } else {
                                "tool".to_string()
                            };
                            active_tool_names.insert(tool_call_id.clone(), tool_name.clone());
                            let summary = extract_tool_call_summary(&tool_name, &title, &args);
                            runtime.emit_tui(EngineStreamEvent::ToolInvoked {
                                thread_id: thread_id.clone(),
                                tool_name: tool_name.clone(),
                                tool_call_id: (!tool_call_id.is_empty())
                                    .then(|| tool_call_id.clone()),
                                summary,
                            });
                            runtime.emit_tui(EngineStreamEvent::ToolActivityUpdated {
                                thread_id: thread_id.clone(),
                                tool_name: Some(tool_name.clone()),
                            });

                            if name == "invoke_subagent"
                                || name.contains("invoke_subagent")
                                || tool_name.contains("invoke_subagent")
                            {
                                let snap = SubagentSnapshot {
                                    id: tool_call_id.clone(),
                                    role: extract_subagent_role(&args, &title),
                                    agent_type: extract_subagent_type(&args),
                                    model: None,
                                    activity: Some("running".to_string()),
                                    status: SubagentStatus::Running,
                                    started_at_ms: now_epoch_ms(),
                                };
                                active_subagents.insert(
                                    tool_call_id.clone(),
                                    TrackedLayer1Subagent {
                                        snapshot: snap.clone(),
                                        launched_turn_seq: solicited_turn_seq,
                                        launched_unsolicited: !solicited_in_flight,
                                        launched_wakeup_seq: wakeup_seq,
                                    },
                                );
                                runtime.emit_tui(EngineStreamEvent::SubagentUpdated {
                                    thread_id: thread_id.clone(),
                                    subagent: snap.clone(),
                                });
                                runtime.subagent_observer.register_subagent(&thread_id, snap);
                            }
                        }
                        DriverEvent::ToolCallUpdate {
                            tool_call_id,
                            name,
                            status,
                            output,
                            ..
                        } => {
                            if !solicited_in_flight && quiet_deadline.is_some() {
                                quiet_deadline = Some(tokio::time::Instant::now() + runtime.quiet_timeout());
                            }
                            let is_done = status.eq_ignore_ascii_case("completed")
                                || status.eq_ignore_ascii_case("done");
                            let is_failed = status.eq_ignore_ascii_case("failed")
                                || status.eq_ignore_ascii_case("error");

                            if is_done || is_failed {
                                let resolved_name = if !name.is_empty() {
                                    active_tool_names.remove(&tool_call_id);
                                    name
                                } else {
                                    active_tool_names
                                        .remove(&tool_call_id)
                                        .unwrap_or_else(|| "tool".to_string())
                                };
                                let resolved_output = if output.is_empty() && !is_done {
                                    status.clone()
                                } else {
                                    output.clone()
                                };
                                runtime.emit_tui(EngineStreamEvent::ToolResult {
                                    thread_id: thread_id.clone(),
                                    tool_name: resolved_name,
                                    output: resolved_output.clone(),
                                    tool_call_id: (!tool_call_id.is_empty())
                                        .then(|| tool_call_id.clone()),
                                    is_error: is_failed,
                                });
                                runtime.emit_tui(EngineStreamEvent::ToolActivityUpdated {
                                    thread_id: thread_id.clone(),
                                    tool_name: None,
                                });

                                let watch = runtime.subagent_observer.as_ref();
                                if let Some(sa) = active_subagents.get_mut(&tool_call_id) {
                                    if is_failed {
                                        let reason = if resolved_output.trim().is_empty() {
                                            "failed".to_string()
                                        } else {
                                            resolved_output.trim().chars().take(60).collect()
                                        };
                                        watch.mark_terminal(
                                            &thread_id,
                                            &tool_call_id,
                                            SubagentStatus::Failed,
                                            Some(reason),
                                        );
                                        active_subagents.remove(&tool_call_id);
                                    } else {
                                        watch.ingest_tool_output(&thread_id, &tool_call_id, &output);
                                        sa.snapshot.activity = Some("background active".to_string());
                                        watch.update_activity_if_unlinked(
                                            &thread_id,
                                            &tool_call_id,
                                            "background active",
                                        );
                                    }
                                }
                            }
                        }
                        DriverEvent::SessionInfoUpdate { title, .. } => {
                            let trimmed = title.trim();
                            if !trimmed.is_empty()
                                && runtime.storage.update_thread_title(&tid, trimmed).is_ok()
                            {
                                runtime.emit_tui(EngineStreamEvent::ThreadTitleUpdated {
                                    thread_id: thread_id.clone(),
                                    title: trimmed.to_string(),
                                });
                            }
                        }
                        DriverEvent::UsageUpdate { .. } => {
                            if !solicited_in_flight && !accumulated_text.trim().is_empty() {
                                finalize_unsolicited(
                                    &runtime,
                                    &mut current_leaf_id,
                                    &mut accumulated_text,
                                    &mut unsolicited_streaming_emitted,
                                    &mut quiet_deadline,
                                    &mut wakeup_active,
                                    &mut active_subagents,
                                    solicited_turn_seq,
                                    wakeup_seq,
                                );
                            }
                        }
                        DriverEvent::PermissionRequested {
                            request_id,
                            tool_name,
                            title,
                            args,
                            options,
                            ..
                        } => {
                            if !solicited_in_flight {
                                quiet_deadline = None;
                            }
                            let tool_request = ToolInvocationRequest::new(&tool_name, args.clone())
                                .with_isolated_worktree(is_thread_in_isolated_worktree(&runtime, tid));
                            if let Some(policy) = runtime.policy.as_ref() {
                                match policy.evaluate_scoped(
                                    &AcpSessionId::new(&session_id),
                                    &AcpThreadId::new(&thread_id),
                                    &tool_request,
                                ) {
                                    PolicyDecision::Allow => {
                                        let option_id = select_permission_option(&options, true);
                                        let _ = driver
                                            .respond_permission(request_id, Some(&option_id))
                                            .await;
                                        continue;
                                    }
                                    PolicyDecision::Deny => {
                                        let option_id = select_permission_option(&options, false);
                                        let _ = driver
                                            .respond_permission(request_id, Some(&option_id))
                                            .await;
                                        continue;
                                    }
                                    PolicyDecision::PromptUser => {}
                                }
                            }
                            let seq = runtime.next_perm_seq.fetch_add(1, Ordering::Relaxed);
                            let request_key = format!("{session_id}:{seq}");
                            let description = format_permission_description(&tool_name, &title, &args);
                            if let Ok(mut state) = runtime.lock() {
                                state.pending_permissions.insert(
                                    request_key.clone(),
                                    PendingPermissionRecord {
                                        driver: Arc::clone(&driver),
                                        json_rpc_id: request_id,
                                        options,
                                        session_id: session_id.clone(),
                                        thread_id: thread_id.clone(),
                                        request: tool_request,
                                    },
                                );
                            }
                            let _ = runtime
                                .storage
                                .update_thread_status(&tid, ThreadStatus::PermissionRequired);
                            runtime.emit_tui(EngineStreamEvent::ThreadStatusChanged {
                                thread_id: thread_id.clone(),
                                status: "permission_required".to_string(),
                            });
                            runtime.emit_tui(EngineStreamEvent::PermissionRequested {
                                request_id: request_key,
                                description,
                            });
                        }
                        DriverEvent::PromptCompleted { .. } => {
                            if !accumulated_text.is_empty() {
                                let text = std::mem::take(&mut accumulated_text);
                                match runtime.persist_assistant_node(tid, current_leaf_id, text) {
                                    Ok(new_leaf) => current_leaf_id = new_leaf,
                                    Err(err) => {
                                        let msg = err.to_string();
                                        runtime.mark_thread_error(tid, &thread_id, &msg);
                                        solicited_in_flight = false;
                                        solicited_error_emitted = true;
                                        if let Some(tx) = solicited_done_tx.take() {
                                            let _ = tx.send(Err(err));
                                        }
                                        continue;
                                    }
                                }
                            }
                            let _ = runtime.storage.update_thread_status(&tid, ThreadStatus::Idle);
                            runtime.emit_tui(EngineStreamEvent::TurnCompleted {
                                thread_id: thread_id.clone(),
                            });
                            runtime.emit_tui(EngineStreamEvent::ThreadStatusChanged {
                                thread_id: thread_id.clone(),
                                status: "idle".to_string(),
                            });
                            solicited_in_flight = false;
                            solicited_error_emitted = false;
                            unsolicited_streaming_emitted = false;
                            quiet_deadline = None;
                            wakeup_active = false;
                            finalize_eligible_subagents(
                                &runtime,
                                &thread_id,
                                &mut active_subagents,
                                solicited_turn_seq,
                                wakeup_seq,
                                false,
                            );
                            if let Some(tx) = solicited_done_tx.take() {
                                let _ = tx.send(Ok(current_leaf_id));
                            }
                        }
                        DriverEvent::Error { message, .. } => {
                            runtime.mark_thread_error(tid, &thread_id, &message);
                            accumulated_text.clear();
                            unsolicited_streaming_emitted = false;
                            quiet_deadline = None;
                            wakeup_active = false;
                            if solicited_in_flight {
                                solicited_in_flight = false;
                                solicited_error_emitted = true;
                                if let Some(tx) = solicited_done_tx.take() {
                                    let _ = tx.send(Err(EngineError::Transport(message)));
                                }
                            }
                        }
                        DriverEvent::CurrentModeUpdate {
                            current_mode_id, ..
                        } => {
                            let mut updated_meta = None;
                            let mut updated_opts = None;
                            if let Ok(mut state) = runtime.lock() {
                                if let Some(entry) = state.active_acp_sessions.get_mut(&thread_id) {
                                    if entry.session_id == session_id
                                        && !current_mode_id.trim().is_empty()
                                    {
                                        entry.mode = Some(current_mode_id.clone());
                                        let mut changed_opt = false;
                                        for opt in &mut entry.config_options {
                                            if (opt.id == "mode"
                                                || opt.category.as_deref() == Some("mode"))
                                                && opt.current_value != current_mode_id
                                            {
                                                opt.current_value = current_mode_id.clone();
                                                changed_opt = true;
                                            }
                                        }
                                        updated_meta = Some((entry.model.clone(), entry.mode.clone()));
                                        if changed_opt {
                                            updated_opts = Some(entry.config_options.clone());
                                        }
                                    }
                                }
                            }
                            if let Some((model, mode)) = updated_meta {
                                runtime.emit_tui(EngineStreamEvent::AgentMetaUpdated {
                                    thread_id: thread_id.clone(),
                                    model,
                                    mode,
                                });
                            }
                            if let Some(config_options) = updated_opts {
                                runtime.emit_tui(EngineStreamEvent::AgentConfigUpdated {
                                    thread_id: thread_id.to_string(),
                                    config_options,
                                });
                            }
                        }
                        DriverEvent::ConfigOptionUpdate { config_options, .. } => {
                            let incoming: Vec<AgentConfigOption> = config_options
                                .iter()
                                .map(acp_config_option_to_core)
                                .collect();
                            let mut emit_opts = None;
                            let mut emit_meta = None;
                            if let Ok(mut state) = runtime.lock() {
                                if let Some(entry) = state.active_acp_sessions.get_mut(&thread_id) {
                                    if entry.session_id == session_id {
                                        let (merged, next_legacy_model, next_legacy_mode) =
                                            merge_updated_config_options(
                                                &entry.config_options,
                                                incoming,
                                                entry.has_legacy_model,
                                                entry.has_legacy_mode,
                                            );
                                        entry.has_legacy_model = next_legacy_model;
                                        entry.has_legacy_mode = next_legacy_mode;
                                        let (new_model, new_mode) =
                                            extract_model_and_mode_from_options(
                                                &merged,
                                                entry.model.clone(),
                                                entry.mode.clone(),
                                            );
                                        let meta_changed =
                                            new_model != entry.model || new_mode != entry.mode;
                                        entry.config_options = merged.clone();
                                        entry.model = new_model.clone();
                                        entry.mode = new_mode.clone();
                                        emit_opts = Some(merged);
                                        if meta_changed || new_model.is_some() || new_mode.is_some()
                                        {
                                            emit_meta = Some((new_model, new_mode));
                                        }
                                    }
                                }
                            }
                            if let Some(config_options) = emit_opts {
                                runtime.emit_tui(EngineStreamEvent::AgentConfigUpdated {
                                    thread_id: thread_id.clone(),
                                    config_options,
                                });
                            }
                            if let Some((model, mode)) = emit_meta {
                                runtime.emit_tui(EngineStreamEvent::AgentMetaUpdated {
                                    thread_id: thread_id.clone(),
                                    model,
                                    mode,
                                });
                            }
                        }
                        DriverEvent::AvailableCommandsUpdate { commands, .. } => {
                            let converted: Vec<AgentSlashCommand> = commands
                                .into_iter()
                                .map(|c| AgentSlashCommand {
                                    name: c.name,
                                    description: c.description,
                                })
                                .collect();
                            let mut updated = false;
                            if let Ok(mut state) = runtime.lock() {
                                if let Some(entry) = state.active_acp_sessions.get_mut(&thread_id) {
                                    if entry.session_id == session_id {
                                        entry.available_commands = converted.clone();
                                        updated = true;
                                    }
                                }
                            }
                            if updated {
                                runtime.emit_tui(EngineStreamEvent::AgentCommandsUpdated {
                                    thread_id: thread_id.clone(),
                                    commands: converted,
                                });
                            }
                        }
                    }
                }
                _ = sleep_fut, if quiet_deadline.is_some() => {
                    if !solicited_in_flight && !accumulated_text.trim().is_empty() {
                        finalize_unsolicited(
                            &runtime,
                            &mut current_leaf_id,
                            &mut accumulated_text,
                            &mut unsolicited_streaming_emitted,
                            &mut quiet_deadline,
                            &mut wakeup_active,
                            &mut active_subagents,
                            solicited_turn_seq,
                            wakeup_seq,
                        );
                    } else {
                        quiet_deadline = None;
                        wakeup_active = false;
                    }
                }
            }
        }
    })
}
