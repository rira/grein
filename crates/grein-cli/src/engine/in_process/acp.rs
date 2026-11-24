use std::path::PathBuf;
use std::sync::Arc;

use grein_acp::AgentDriver;
use grein_core::model::{NodeId, ThreadId, ThreadKind};
use grein_core::port::dto::{AgentConfigOption, AgentOptionsSnapshot, EngineStreamEvent};
use grein_core::sync::ContextCompiler;
use tokio::sync::mpsc;

use super::super::types::{
    acp_config_option_to_core, driver_err_to_engine, extract_model_and_mode_from_options,
    format_driver_failure, is_internal_root_node, is_unsupported_method_error,
    merge_updated_config_options, normalize_session_config_options, parse_typed_id, storage_err,
    AcpBackendConfig, AgentMetaEntry, EngineError, ACP_SYSTEM_PROMPT_MAX_CHARS,
};
use super::listener::spawn_session_listener_with_observer;
use super::providers::AcpProviderRuntime;
use super::{
    ActiveAcpSession, ActiveSessionHandle, ConfigTarget, EngineRuntime, InProcessEngine,
};

impl EngineRuntime {

    pub(super) async fn apply_config_to_driver(
        driver: &dyn AgentDriver,
        session_id: &str,
        target: &ConfigTarget<'_>,
    ) -> Result<Option<Vec<AgentConfigOption>>, EngineError> {
        match driver
            .set_config_option(session_id, target.config_id, target.value)
            .await
        {
            Ok(opts) => {
                if opts.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(opts.iter().map(acp_config_option_to_core).collect()))
                }
            }
            Err(err) if is_unsupported_method_error(&err) => {
                if target.config_id == "model" || target.is_model_hint {
                    driver
                        .set_session_model(session_id, target.value)
                        .await
                        .map_err(driver_err_to_engine)?;
                    Ok(None)
                } else if target.config_id == "mode" || target.is_mode_hint {
                    driver
                        .set_session_mode(session_id, target.value)
                        .await
                        .map_err(driver_err_to_engine)?;
                    Ok(None)
                } else {
                    Err(EngineError::Protocol(format!(
                        "ACP config option `{}` not supported: {err}",
                        target.config_id
                    )))
                }
            }
            Err(err) => Err(driver_err_to_engine(err)),
        }
    }

    pub(super) fn apply_config_update_to_state(
        &self,
        thread_id: &str,
        session_id: &str,
        target: &ConfigTarget<'_>,
        updated_opts: Option<Vec<AgentConfigOption>>,
    ) -> Result<(AgentOptionsSnapshot, Option<String>, Option<String>), EngineError> {
        let mut state = self.lock()?;
        let entry = state
            .active_acp_sessions
            .get_mut(thread_id)
            .filter(|e| e.session_id == session_id)
            .ok_or_else(|| EngineError::NotFound(format!("active ACP session for {thread_id}")))?;

        match updated_opts {
            Some(incoming) => {
                let (mut merged, next_legacy_model, next_legacy_mode) = merge_updated_config_options(
                    &entry.config_options,
                    incoming,
                    entry.has_legacy_model,
                    entry.has_legacy_mode,
                );
                entry.has_legacy_model = next_legacy_model;
                entry.has_legacy_mode = next_legacy_mode;
                for opt in &mut merged {
                    if opt.id == target.config_id && opt.current_value != target.value {
                        opt.current_value = target.value.to_string();
                    }
                }
                entry.config_options = merged;
            }
            None => {
                for opt in &mut entry.config_options {
                    if opt.id == target.config_id {
                        opt.current_value = target.value.to_string();
                    }
                }
            }
        }

        if target.config_id == "model" || target.is_model_hint {
            entry.model = Some(target.value.to_string());
        }
        if target.config_id == "mode" || target.is_mode_hint {
            entry.mode = Some(target.value.to_string());
        }
        let (model, mode) = extract_model_and_mode_from_options(
            &entry.config_options,
            entry.model.clone(),
            entry.mode.clone(),
        );
        entry.model = model.clone();
        entry.mode = mode.clone();

        let snapshot = AgentOptionsSnapshot {
            thread_id: thread_id.to_string(),
            config_options: entry.config_options.clone(),
            available_commands: entry.available_commands.clone(),
        };
        Ok((snapshot, model, mode))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn ensure_acp_session_for_thread(
        self: &Arc<Self>,
        provider: &Arc<AcpProviderRuntime>,
        cfg: &AcpBackendConfig,
        tid: ThreadId,
        thread_id: &str,
        initial_leaf_id: NodeId,
        exclude_node_id: Option<NodeId>,
        emit_initial_meta: bool,
    ) -> Result<(Arc<dyn AgentDriver>, ActiveSessionHandle), EngineError> {
        let (driver, driver_gen) = provider.ensure_driver(cfg, &self.driver_gen_seq).await?;

        {
            let mut state = self.lock()?;
            let stale = state.active_acp_sessions.get(thread_id).is_some_and(|s| {
                s.provider_id != provider.id
                    || s.driver_gen != driver_gen
                    || s.cmd_tx.is_closed()
            });
            if stale {
                state.active_acp_sessions.remove(thread_id);
            }
            if let Some(existing) = state.active_acp_sessions.get(thread_id) {
                return Ok((
                    driver,
                    ActiveSessionHandle {
                        session_id: existing.session_id.clone(),
                        driver_gen: existing.driver_gen,
                        cmd_tx: existing.cmd_tx.clone(),
                    },
                ));
            }
        }

        let mut payload = ContextCompiler::new("")
            .compile_thread_context(self.storage.as_ref(), tid)
            .map_err(storage_err)?;
        payload.trunk_path.retain(|n| !is_internal_root_node(n));
        if let Some(ex_id) = exclude_node_id {
            payload.branch_turns.retain(|n| n.id != ex_id);
        }
        let include_branch_turns = !payload.branch_turns.is_empty();
        let append =
            payload.build_acp_system_prompt_append(include_branch_turns, ACP_SYSTEM_PROMPT_MAX_CHARS);
        let append_opt = (!append.trim().is_empty()).then_some(append.as_str());

        let session_cwd = self
            .storage
            .get_thread(&tid)
            .map_err(storage_err)?
            .and_then(|t| t.cwd)
            .filter(|c| !c.trim().is_empty())
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| cfg.cwd.clone());

        let info = match driver.new_session(&session_cwd, append_opt).await {
            Ok(info) => info,
            Err(err) => {
                let msg = format_driver_failure("ACP new_session failed", &err, driver.as_ref());
                provider
                    .invalidate_driver_if_dead(&driver, driver_gen, &self.state)
                    .await;
                return Err(EngineError::Transport(msg));
            }
        };

        self.storage
            .update_thread_acp_session_id(&tid, Some(&info.session_id))
            .map_err(storage_err)?;

        let config_options = normalize_session_config_options(&info);
        let has_legacy_model = !info
            .config_options
            .iter()
            .any(|o| o.id == "model" || o.category.as_deref() == Some("model"))
            && !info.available_models.is_empty();
        let has_legacy_mode = !info
            .config_options
            .iter()
            .any(|o| o.id == "mode" || o.category.as_deref() == Some("mode"))
            && !info.available_modes.is_empty();
        let (model, mode) = extract_model_and_mode_from_options(
            &config_options,
            info.current_model_id.clone(),
            info.current_mode_id.clone(),
        );

        let events_rx = driver.subscribe_session(&info.session_id);
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let listener_handle = spawn_session_listener_with_observer(
            Arc::clone(self),
            Arc::clone(&provider.subagent_observer),
            tid,
            thread_id.to_string(),
            info.session_id.clone(),
            Arc::clone(&driver),
            initial_leaf_id,
            events_rx,
            cmd_rx,
        );

        let handle = ActiveSessionHandle {
            session_id: info.session_id.clone(),
            driver_gen,
            cmd_tx: cmd_tx.clone(),
        };

        let mut desired_config: Vec<(String, String)> = {
            let mut state = self.lock()?;
            let mut merged = state
                .default_config
                .get(&provider.id)
                .cloned()
                .unwrap_or_default();
            if let Some(pending) = state.pending_thread_config.get(thread_id) {
                for (k, v) in pending {
                    merged.insert(k.clone(), v.clone());
                }
            }
            state.active_acp_sessions.insert(
                thread_id.to_string(),
                ActiveAcpSession {
                    provider_id: provider.id.clone(),
                    session_id: info.session_id.clone(),
                    driver_gen,
                    model: model.clone(),
                    mode: mode.clone(),
                    config_options: config_options.clone(),
                    available_commands: Vec::new(),
                    has_legacy_model,
                    has_legacy_mode,
                    pending_landmarks: Vec::new(),
                    cmd_tx,
                    listener_handle,
                },
            );
            merged.into_iter().collect()
        };

        desired_config.sort_by(|(a, _), (b, _)| {
            let rank = |k: &str| match k {
                "model" => 0,
                "mode" => 1,
                _ => 2,
            };
            rank(a).cmp(&rank(b)).then_with(|| a.cmp(b))
        });

        for (cfg_id, cfg_val) in desired_config {
            let (already_matches, is_model_hint, is_mode_hint) = {
                let state = self.lock()?;
                if let Some(entry) = state.active_acp_sessions.get(thread_id) {
                    let matching_opt = entry.config_options.iter().find(|o| o.id == cfg_id);
                    let current = matching_opt.map(|o| o.current_value.as_str()).or_else(|| {
                        if cfg_id == "model" {
                            entry.model.as_deref()
                        } else if cfg_id == "mode" {
                            entry.mode.as_deref()
                        } else {
                            None
                        }
                    });
                    let is_model = matching_opt
                        .is_some_and(|o| o.category.as_deref() == Some("model"));
                    let is_mode = matching_opt
                        .is_some_and(|o| o.category.as_deref() == Some("mode"));
                    (current == Some(cfg_val.as_str()), is_model, is_mode)
                } else {
                    (false, false, false)
                }
            };
            if already_matches {
                continue;
            }
            let target = ConfigTarget {
                config_id: &cfg_id,
                value: &cfg_val,
                is_model_hint,
                is_mode_hint,
            };
            if let Ok(updated_opts) =
                Self::apply_config_to_driver(driver.as_ref(), &info.session_id, &target).await
            {
                let _ = self.apply_config_update_to_state(
                    thread_id,
                    &info.session_id,
                    &target,
                    updated_opts,
                );
            }
        }

        if emit_initial_meta {
            let (final_model, final_mode, final_opts) = {
                let state = self.lock()?;
                state
                    .active_acp_sessions
                    .get(thread_id)
                    .map(|e| (e.model.clone(), e.mode.clone(), e.config_options.clone()))
                    .unwrap_or((model, mode, config_options))
            };
            if final_model.is_some() || final_mode.is_some() {
                self.emit_tui(EngineStreamEvent::AgentMetaUpdated {
                    thread_id: thread_id.to_string(),
                    model: final_model,
                    mode: final_mode,
                });
            }
            if !final_opts.is_empty() {
                self.emit_tui(EngineStreamEvent::AgentConfigUpdated {
                    thread_id: thread_id.to_string(),
                    config_options: final_opts,
                });
            }
        }

        Ok((driver, handle))
    }

    pub(super) async fn reset_thread_acp_session(
        &self,
        tid: ThreadId,
        thread_id: &str,
    ) -> Result<(), EngineError> {
        self.storage
            .update_thread_acp_session_id(&tid, None)
            .map_err(storage_err)?;
        let old_session = {
            let mut state = self.lock()?;
            state
                .active_acp_sessions
                .remove(thread_id)
                .map(|s| (s.session_id.clone(), s.driver_gen, s.provider_id.clone()))
        };
        if let Some((session_id, driver_gen, provider_id)) = old_session {
            if let Some(provider) = self.providers.get(&provider_id) {
                provider
                    .close_session_if_running(&session_id, driver_gen)
                    .await;
            }
        }
        Ok(())
    }
}

impl InProcessEngine {
    pub(super) async fn agent_metadata_impl(&self) -> Result<Vec<AgentMetaEntry>, EngineError> {
        let state = self.runtime.lock()?;
        Ok(state
            .active_acp_sessions
            .iter()
            .map(|(thread_id, s)| AgentMetaEntry {
                thread_id: thread_id.clone(),
                model: s.model.clone(),
                mode: s.mode.clone(),
                config_options: s.config_options.clone(),
                available_commands: s.available_commands.clone(),
            })
            .collect())
    }

    pub(super) async fn agent_options_impl(
        &self,
        thread_id: &str,
    ) -> Result<AgentOptionsSnapshot, EngineError> {
        let tid: ThreadId = parse_typed_id(thread_id)
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        let thread = self
            .runtime
            .storage
            .get_thread(&tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        if thread.kind == ThreadKind::Terminal {
            return Ok(AgentOptionsSnapshot {
                thread_id: thread_id.to_string(),
                config_options: Vec::new(),
                available_commands: Vec::new(),
            });
        }

        let provider_opt = self
            .runtime
            .resolve_runtime_for_thread(&thread)?
            .and_then(|provider| {
                provider
                    .acp_config(&self.runtime.cwd)
                    .map(|cfg| (provider, cfg))
            });
        let Some((provider, _cfg)) = provider_opt else {
            let state = self.runtime.lock()?;
            return Ok(match state.active_acp_sessions.get(thread_id) {
                Some(s) => AgentOptionsSnapshot {
                    thread_id: thread_id.to_string(),
                    config_options: s.config_options.clone(),
                    available_commands: s.available_commands.clone(),
                },
                None => AgentOptionsSnapshot {
                    thread_id: thread_id.to_string(),
                    config_options: Vec::new(),
                    available_commands: Vec::new(),
                },
            });
        };

        if let Some((_, current_gen)) = provider.current_driver_if_running(&self.runtime.state).await
        {
            let state = self.runtime.lock()?;
            if let Some(s) = state.active_acp_sessions.get(thread_id) {
                if s.provider_id == provider.id
                    && s.driver_gen == current_gen
                    && !s.cmd_tx.is_closed()
                {
                    return Ok(AgentOptionsSnapshot {
                        thread_id: thread_id.to_string(),
                        config_options: s.config_options.clone(),
                        available_commands: s.available_commands.clone(),
                    });
                }
            }
        }

        let _turn_guard = self.runtime.acquire_thread_turn_lock(thread_id).await?;
        let fresh_thread = self
            .runtime
            .storage
            .get_thread(&tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        let Some((fresh_provider, fresh_cfg)) = self
            .runtime
            .resolve_runtime_for_thread(&fresh_thread)?
            .and_then(|p| p.acp_config(&self.runtime.cwd).map(|c| (p, c)))
        else {
            let state = self.runtime.lock()?;
            return Ok(match state.active_acp_sessions.get(thread_id) {
                Some(s) => AgentOptionsSnapshot {
                    thread_id: thread_id.to_string(),
                    config_options: s.config_options.clone(),
                    available_commands: s.available_commands.clone(),
                },
                None => AgentOptionsSnapshot {
                    thread_id: thread_id.to_string(),
                    config_options: Vec::new(),
                    available_commands: Vec::new(),
                },
            });
        };

        self.runtime
            .ensure_acp_session_for_thread(
                &fresh_provider,
                &fresh_cfg,
                tid,
                thread_id,
                fresh_thread.active_leaf_id,
                None,
                true,
            )
            .await?;

        let state = self.runtime.lock()?;
        Ok(match state.active_acp_sessions.get(thread_id) {
            Some(s) => AgentOptionsSnapshot {
                thread_id: thread_id.to_string(),
                config_options: s.config_options.clone(),
                available_commands: s.available_commands.clone(),
            },
            None => AgentOptionsSnapshot {
                thread_id: thread_id.to_string(),
                config_options: Vec::new(),
                available_commands: Vec::new(),
            },
        })
    }

    pub(super) async fn set_agent_config_impl(
        &self,
        thread_id: &str,
        config_id: &str,
        value: &str,
        set_default: bool,
    ) -> Result<AgentOptionsSnapshot, EngineError> {
        if config_id.trim().is_empty() || value.trim().is_empty() {
            return Err(EngineError::InvalidParams(
                "config_id and value must not be empty".to_string(),
            ));
        }

        let tid: ThreadId = parse_typed_id(thread_id)
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        let thread = self
            .runtime
            .storage
            .get_thread(&tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        if thread.kind == ThreadKind::Terminal {
            return Err(EngineError::InvalidParams(
                "cannot set agent config on a terminal thread".to_string(),
            ));
        }

        let (provider, cfg) = self
            .runtime
            .resolve_runtime_for_thread(&thread)?
            .and_then(|p| p.acp_config(&self.runtime.cwd).map(|c| (p, c)))
            .ok_or_else(|| EngineError::Transport("no ACP agent configured".to_string()))?;

        let existing = if let Some((driver, current_gen)) =
            provider.current_driver_if_running(&self.runtime.state).await
        {
            let state = self.runtime.lock()?;
            state
                .active_acp_sessions
                .get(thread_id)
                .filter(|s| {
                    s.provider_id == provider.id
                        && s.driver_gen == current_gen
                        && !s.cmd_tx.is_closed()
                })
                .map(|s| {
                    let opt = s.config_options.iter().find(|o| o.id == config_id);
                    (
                        provider.id.clone(),
                        driver,
                        s.session_id.clone(),
                        opt.is_some_and(|o| o.category.as_deref() == Some("model")),
                        opt.is_some_and(|o| o.category.as_deref() == Some("mode")),
                    )
                })
        } else {
            None
        };

        let _ = cfg;
        let (resolved_provider_id, driver, session_id, is_model_hint, is_mode_hint) = match existing
        {
            Some(found) => found,
            None => {
                let _turn_guard = self.runtime.acquire_thread_turn_lock(thread_id).await?;
                let fresh_thread = self
                    .runtime
                    .storage
                    .get_thread(&tid)
                    .map_err(storage_err)?
                    .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
                let (fresh_provider, fresh_cfg) = self
                    .runtime
                    .resolve_runtime_for_thread(&fresh_thread)?
                    .and_then(|p| p.acp_config(&self.runtime.cwd).map(|c| (p, c)))
                    .ok_or_else(|| {
                        EngineError::Transport("no ACP agent configured".to_string())
                    })?;
                let (driver, handle) = self
                    .runtime
                    .ensure_acp_session_for_thread(
                        &fresh_provider,
                        &fresh_cfg,
                        tid,
                        thread_id,
                        fresh_thread.active_leaf_id,
                        None,
                        true,
                    )
                    .await?;
                let state = self.runtime.lock()?;
                let opt = state
                    .active_acp_sessions
                    .get(thread_id)
                    .and_then(|s| s.config_options.iter().find(|o| o.id == config_id));
                (
                    fresh_provider.id.clone(),
                    driver,
                    handle.session_id,
                    opt.is_some_and(|o| o.category.as_deref() == Some("model")),
                    opt.is_some_and(|o| o.category.as_deref() == Some("mode")),
                )
            }
        };

        let target = ConfigTarget {
            config_id,
            value,
            is_model_hint,
            is_mode_hint,
        };
        let updated_opts =
            EngineRuntime::apply_config_to_driver(driver.as_ref(), &session_id, &target).await?;

        let (snapshot, model, mode) = self.runtime.apply_config_update_to_state(
            thread_id,
            &session_id,
            &target,
            updated_opts,
        )?;

        {
            let mut state = self.runtime.lock()?;
            state
                .pending_thread_config
                .entry(thread_id.to_string())
                .or_default()
                .insert(config_id.to_string(), value.to_string());
            if set_default {
                state
                    .default_config
                    .entry(resolved_provider_id)
                    .or_default()
                    .insert(config_id.to_string(), value.to_string());
            }
        }

        self.runtime.emit_tui(EngineStreamEvent::AgentConfigUpdated {
            thread_id: thread_id.to_string(),
            config_options: snapshot.config_options.clone(),
        });
        self.runtime.emit_tui(EngineStreamEvent::AgentMetaUpdated {
            thread_id: thread_id.to_string(),
            model,
            mode,
        });

        Ok(snapshot)
    }
}
