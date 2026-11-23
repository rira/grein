use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use grein_acp::{AcpStdioDriver, StdioProcessConfig, StdioProcessRunner};
use grein_core::model::{NodeId, ThreadId, ThreadKind};
use grein_core::sync::ContextCompiler;
use grein_tui::engine_port::{AgentConfigOption, AgentOptionsSnapshot, TuiEngineEvent};
use tokio::sync::mpsc;

use super::super::types::{
    acp_config_option_to_tui, extract_model_and_mode_from_options, is_internal_root_node,
    is_unsupported_method_error, merge_updated_config_options, normalize_session_config_options,
    parse_typed_id, stdio_err_to_engine, storage_err, AcpBackendConfig, AgentMetaEntry,
    EngineError, ACP_SYSTEM_PROMPT_MAX_CHARS, CLOSE_SESSION_TIMEOUT,
};
use super::listener::spawn_session_listener;
use super::{
    ActiveAcpSession, ActiveSessionHandle, ConfigTarget, DriverSlot, EngineRuntime, InProcessEngine,
};

impl EngineRuntime {
    pub(super) async fn current_driver_if_running(&self) -> Option<(Arc<AcpStdioDriver>, u64)> {
        let mut guard = self.acp_driver.lock().await;
        if let Some(slot) = guard.as_ref() {
            if slot.driver.is_running() {
                return Some((Arc::clone(&slot.driver), slot.generation));
            }
            let dead_gen = slot.generation;
            *guard = None;
            if let Ok(mut state) = self.state.lock() {
                state
                    .active_acp_sessions
                    .retain(|_, s| s.driver_gen != dead_gen);
            }
        }
        None
    }

    pub(super) async fn apply_config_to_driver(
        driver: &AcpStdioDriver,
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
                    Ok(Some(opts.iter().map(acp_config_option_to_tui).collect()))
                }
            }
            Err(err) if is_unsupported_method_error(&err) => {
                if target.config_id == "model" || target.is_model_hint {
                    driver
                        .set_session_model(session_id, target.value)
                        .await
                        .map_err(stdio_err_to_engine)?;
                    Ok(None)
                } else if target.config_id == "mode" || target.is_mode_hint {
                    driver
                        .set_session_mode(session_id, target.value)
                        .await
                        .map_err(stdio_err_to_engine)?;
                    Ok(None)
                } else {
                    Err(EngineError::Protocol(format!(
                        "ACP config option `{}` not supported: {err}",
                        target.config_id
                    )))
                }
            }
            Err(err) => Err(stdio_err_to_engine(err)),
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
        cfg: &AcpBackendConfig,
        tid: ThreadId,
        thread_id: &str,
        initial_leaf_id: NodeId,
        exclude_node_id: Option<NodeId>,
        emit_initial_meta: bool,
    ) -> Result<(Arc<AcpStdioDriver>, ActiveSessionHandle), EngineError> {
        let (driver, driver_gen) = self.ensure_driver(cfg).await?;

        {
            let mut state = self.lock()?;
            let stale = state
                .active_acp_sessions
                .get(thread_id)
                .is_some_and(|s| s.driver_gen != driver_gen || s.cmd_tx.is_closed());
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
            .compile_thread_context(&self.storage, tid)
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
            .get_thread(tid)
            .map_err(storage_err)?
            .and_then(|t| t.cwd)
            .filter(|c| !c.trim().is_empty())
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| cfg.cwd.clone());

        let info = match driver.new_session(&session_cwd, append_opt).await {
            Ok(info) => info,
            Err(err) => {
                self.invalidate_driver_if_dead(&driver, driver_gen).await;
                return Err(EngineError::Transport(format!("ACP new_session failed: {err}")));
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
        let listener_handle = spawn_session_listener(
            Arc::clone(self),
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
            let mut merged = state.default_config.clone();
            if let Some(pending) = state.pending_thread_config.get(thread_id) {
                for (k, v) in pending {
                    merged.insert(k.clone(), v.clone());
                }
            }
            state.active_acp_sessions.insert(
                thread_id.to_string(),
                ActiveAcpSession {
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
                Self::apply_config_to_driver(&driver, &info.session_id, &target).await
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
                self.emit_tui(TuiEngineEvent::AgentMetaUpdated {
                    thread_id: thread_id.to_string(),
                    model: final_model,
                    mode: final_mode,
                });
            }
            if !final_opts.is_empty() {
                self.emit_tui(TuiEngineEvent::AgentConfigUpdated {
                    thread_id: thread_id.to_string(),
                    config_options: final_opts,
                });
            }
        }

        Ok((driver, handle))
    }

    pub(super) async fn ensure_driver(
        &self,
        cfg: &AcpBackendConfig,
    ) -> Result<(Arc<AcpStdioDriver>, u64), EngineError> {
        let mut guard = self.acp_driver.lock().await;
        if let Some(slot) = guard.as_ref() {
            if slot.driver.is_running() {
                return Ok((Arc::clone(&slot.driver), slot.generation));
            }
            *guard = None;
        }

        let spawn_cfg =
            StdioProcessConfig::new(&cfg.command, cfg.args.clone()).with_current_dir(&cfg.cwd);
        let runner = StdioProcessRunner::spawn(spawn_cfg).await.map_err(|e| {
            EngineError::Transport(format!("failed to spawn ACP agent `{}`: {e}", cfg.command))
        })?;
        let driver = Arc::new(AcpStdioDriver::new(runner));
        if let Err(e) = driver.initialize().await {
            let _ = driver.shutdown().await;
            *guard = None;
            return Err(EngineError::Transport(format!("ACP initialize failed: {e}")));
        }
        if let Some(method) = cfg.auth_method.as_deref().filter(|m| !m.is_empty()) {
            if let Err(e) = driver.authenticate(method).await {
                let _ = driver.shutdown().await;
                *guard = None;
                return Err(EngineError::Transport(format!(
                    "ACP authenticate ({method}) failed: {e}"
                )));
            }
        }
        let generation = self.driver_gen_seq.fetch_add(1, Ordering::Relaxed);
        *guard = Some(DriverSlot {
            driver: Arc::clone(&driver),
            generation,
        });
        Ok((driver, generation))
    }

    pub(super) async fn invalidate_driver_if_dead(
        &self,
        driver: &Arc<AcpStdioDriver>,
        generation: u64,
    ) {
        if driver.is_running() {
            return;
        }
        let mut guard = self.acp_driver.lock().await;
        if guard.as_ref().is_some_and(|s| s.generation == generation) {
            *guard = None;
        }
        if let Ok(mut state) = self.state.lock() {
            state
                .active_acp_sessions
                .retain(|_, s| s.driver_gen != generation);
        }
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
                .map(|s| (s.session_id.clone(), s.driver_gen))
        };
        if let Some((session_id, driver_gen)) = old_session {
            let driver_opt = {
                let guard = self.acp_driver.lock().await;
                guard
                    .as_ref()
                    .filter(|s| s.generation == driver_gen && s.driver.is_running())
                    .map(|s| Arc::clone(&s.driver))
            };
            if let Some(driver) = driver_opt {
                let _ = tokio::time::timeout(
                    CLOSE_SESSION_TIMEOUT,
                    driver.close_session(&session_id),
                )
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
            .get_thread(tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        if thread.kind == ThreadKind::Terminal {
            return Ok(AgentOptionsSnapshot {
                thread_id: thread_id.to_string(),
                config_options: Vec::new(),
                available_commands: Vec::new(),
            });
        }

        let Some(cfg) = self.runtime.acp_config.clone() else {
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

        if let Some((_, current_gen)) = self.runtime.current_driver_if_running().await {
            let state = self.runtime.lock()?;
            if let Some(s) = state.active_acp_sessions.get(thread_id) {
                if s.driver_gen == current_gen && !s.cmd_tx.is_closed() {
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
            .get_thread(tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;

        self.runtime
            .ensure_acp_session_for_thread(
                &cfg,
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
            .get_thread(tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        if thread.kind == ThreadKind::Terminal {
            return Err(EngineError::InvalidParams(
                "cannot set agent config on a terminal thread".to_string(),
            ));
        }

        let Some(cfg) = self.runtime.acp_config.clone() else {
            return Err(EngineError::Transport(
                "no ACP agent configured; install agy or set GREIN_ACP_BIN".to_string(),
            ));
        };

        let existing = if let Some((driver, current_gen)) =
            self.runtime.current_driver_if_running().await
        {
            let state = self.runtime.lock()?;
            state
                .active_acp_sessions
                .get(thread_id)
                .filter(|s| s.driver_gen == current_gen && !s.cmd_tx.is_closed())
                .map(|s| {
                    let opt = s.config_options.iter().find(|o| o.id == config_id);
                    (
                        driver,
                        s.session_id.clone(),
                        opt.is_some_and(|o| o.category.as_deref() == Some("model")),
                        opt.is_some_and(|o| o.category.as_deref() == Some("mode")),
                    )
                })
        } else {
            None
        };

        let (driver, session_id, is_model_hint, is_mode_hint) = match existing {
            Some(found) => found,
            None => {
                let _turn_guard = self.runtime.acquire_thread_turn_lock(thread_id).await?;
                let fresh_thread = self
                    .runtime
                    .storage
                    .get_thread(tid)
                    .map_err(storage_err)?
                    .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
                let (driver, handle) = self
                    .runtime
                    .ensure_acp_session_for_thread(
                        &cfg,
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
            EngineRuntime::apply_config_to_driver(&driver, &session_id, &target).await?;

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
                    .insert(config_id.to_string(), value.to_string());
            }
        }

        self.runtime.emit_tui(TuiEngineEvent::AgentConfigUpdated {
            thread_id: thread_id.to_string(),
            config_options: snapshot.config_options.clone(),
        });
        self.runtime.emit_tui(TuiEngineEvent::AgentMetaUpdated {
            thread_id: thread_id.to_string(),
            model,
            mode,
        });

        Ok(snapshot)
    }
}
