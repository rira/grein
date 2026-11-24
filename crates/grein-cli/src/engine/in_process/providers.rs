use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use grein_acp::{AcpSpawnConfig, AgentDriver, AgentDriverFactory};
use grein_core::model::{Thread, ThreadId, ThreadKind};
use grein_core::port::dto::{AgentProviderInfo, EngineStreamEvent};
use tokio::sync::Mutex as AsyncMutex;

use super::super::types::{
    parse_typed_id, storage_err, AcpBackendConfig, EngineError, RegisteredAgentProvider,
    CLOSE_SESSION_TIMEOUT,
};
use super::{DriverSlot, EngineDependencies, EngineRuntime, EngineState, InProcessEngine};
use crate::subagent_watch::SubagentObserverPort;

pub(super) struct AcpProviderRuntime {
    pub(super) id: String,
    pub(super) plugin_id: String,
    pub(super) display_name: String,
    pub(super) kind: String,
    pub(super) available: bool,
    pub(super) executable: Option<String>,
    pub(super) args: Vec<String>,
    pub(super) auth_method: Option<String>,
    pub(super) install_hint: Option<String>,
    pub(super) driver_factory: Arc<dyn AgentDriverFactory>,
    pub(super) subagent_observer: Arc<dyn SubagentObserverPort>,
    pub(super) acp_driver: AsyncMutex<Option<DriverSlot>>,
}

impl AcpProviderRuntime {
    pub(super) fn from_registered(reg: RegisteredAgentProvider) -> Self {
        Self {
            id: reg.id,
            plugin_id: reg.plugin_id,
            display_name: reg.display_name,
            kind: reg.kind,
            available: reg.available,
            executable: reg.executable,
            args: reg.args,
            auth_method: reg.auth_method,
            install_hint: reg.install_hint,
            driver_factory: reg.driver_factory,
            subagent_observer: reg.subagent_observer,
            acp_driver: AsyncMutex::new(None),
        }
    }

    pub(super) fn acp_config(&self, cwd: &Path) -> Option<AcpBackendConfig> {
        if !self.available {
            return None;
        }
        let cmd = self
            .executable
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())?;
        Some(AcpBackendConfig {
            command: cmd.to_string(),
            args: self.args.clone(),
            auth_method: self.auth_method.clone(),
            cwd: cwd.to_path_buf(),
        })
    }

    pub(super) fn to_info(&self, is_default: bool) -> AgentProviderInfo {
        AgentProviderInfo {
            id: self.id.clone(),
            plugin_id: self.plugin_id.clone(),
            display_name: self.display_name.clone(),
            kind: self.kind.clone(),
            available: self.available,
            executable: self.executable.clone(),
            is_default,
            install_hint: self.install_hint.clone(),
        }
    }

    pub(super) async fn current_driver_if_running(
        &self,
        state: &Mutex<EngineState>,
    ) -> Option<(Arc<dyn AgentDriver>, u64)> {
        let mut guard = self.acp_driver.lock().await;
        if let Some(slot) = guard.as_ref() {
            if slot.driver.is_running() {
                return Some((Arc::clone(&slot.driver), slot.generation));
            }
            let dead_gen = slot.generation;
            *guard = None;
            if let Ok(mut locked) = state.lock() {
                locked
                    .active_acp_sessions
                    .retain(|_, s| !(s.provider_id == self.id && s.driver_gen == dead_gen));
            }
        }
        None
    }

    pub(super) async fn ensure_driver(
        &self,
        cfg: &AcpBackendConfig,
        gen_seq: &AtomicU64,
    ) -> Result<(Arc<dyn AgentDriver>, u64), EngineError> {
        let mut guard = self.acp_driver.lock().await;
        if let Some(slot) = guard.as_ref() {
            if slot.driver.is_running() {
                return Ok((Arc::clone(&slot.driver), slot.generation));
            }
            *guard = None;
        }

        let mut spawn_cfg = AcpSpawnConfig::new(&cfg.command, cfg.args.clone()).with_cwd(&cfg.cwd);
        if let Some(method) = cfg.auth_method.as_deref().filter(|m| !m.is_empty()) {
            spawn_cfg = spawn_cfg.with_auth_method(method);
        }
        let driver = self.driver_factory.spawn(&spawn_cfg).await.map_err(|e| {
            EngineError::Transport(format!("failed to spawn ACP agent `{}`: {e}", cfg.command))
        })?;
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
        let generation = gen_seq.fetch_add(1, Ordering::Relaxed);
        *guard = Some(DriverSlot {
            driver: Arc::clone(&driver),
            generation,
        });
        Ok((driver, generation))
    }

    pub(super) async fn invalidate_driver_if_dead(
        &self,
        driver: &Arc<dyn AgentDriver>,
        generation: u64,
        state: &Mutex<EngineState>,
    ) {
        if driver.is_running() {
            return;
        }
        let mut guard = self.acp_driver.lock().await;
        if guard.as_ref().is_some_and(|s| s.generation == generation) {
            *guard = None;
        }
        if let Ok(mut locked) = state.lock() {
            locked
                .active_acp_sessions
                .retain(|_, s| !(s.provider_id == self.id && s.driver_gen == generation));
        }
    }

    pub(super) async fn close_session_if_running(&self, session_id: &str, driver_gen: u64) {
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
                driver.close_session(session_id),
            )
            .await;
        }
    }

    pub(super) async fn cancel_session(&self, session_id: &str) -> Result<(), EngineError> {
        let driver = {
            let guard = self.acp_driver.lock().await;
            guard.as_ref().map(|s| Arc::clone(&s.driver))
        };
        if let Some(driver) = driver {
            driver
                .cancel_session(session_id)
                .await
                .map_err(|e| EngineError::Transport(format!("cancel_session failed: {e}")))?;
        }
        Ok(())
    }

    pub(super) async fn shutdown_driver(&self) {
        let slot = {
            let mut guard = self.acp_driver.lock().await;
            guard.take()
        };
        if let Some(slot) = slot {
            if slot.driver.is_running() {
                let _ = slot.driver.shutdown().await;
            }
        }
    }
}

pub(super) fn build_provider_registry(
    deps: &EngineDependencies,
) -> (
    HashMap<String, Arc<AcpProviderRuntime>>,
    Vec<String>,
    Option<String>,
) {
    let mut providers = HashMap::new();
    let mut provider_order = Vec::new();

    let source_list: Vec<RegisteredAgentProvider> = if !deps.providers.is_empty() {
        deps.providers.clone()
    } else if let Some(cfg) = deps.acp_config.as_ref() {
        let display_name = Path::new(&cfg.command)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("default")
            .to_string();
        vec![RegisteredAgentProvider {
            id: "default".to_string(),
            plugin_id: "agent.default".to_string(),
            display_name,
            kind: "acp".to_string(),
            available: true,
            executable: Some(cfg.command.clone()),
            args: cfg.args.clone(),
            auth_method: cfg.auth_method.clone(),
            install_hint: None,
            driver_factory: Arc::clone(&deps.agent_factory),
            subagent_observer: Arc::clone(&deps.subagent_observer),
        }]
    } else {
        Vec::new()
    };

    for mut reg in source_list {
        let key = if !reg.id.trim().is_empty() && !providers.contains_key(&reg.id) {
            reg.id.clone()
        } else if !reg.plugin_id.trim().is_empty() && !providers.contains_key(&reg.plugin_id) {
            reg.plugin_id.clone()
        } else {
            continue;
        };
        reg.id = key.clone();
        provider_order.push(key.clone());
        providers.insert(key, Arc::new(AcpProviderRuntime::from_registered(reg)));
    }

    let default_provider_id = provider_order
        .iter()
        .find(|id| providers.get(*id).is_some_and(|p| p.available))
        .cloned();

    (providers, provider_order, default_provider_id)
}

impl EngineRuntime {
    pub(super) fn find_provider(&self, query: &str) -> Option<Arc<AcpProviderRuntime>> {
        let needle = query.trim();
        if needle.is_empty() {
            return None;
        }
        for id in &self.provider_order {
            if let Some(p) = self.providers.get(id) {
                if p.id.eq_ignore_ascii_case(needle) {
                    return Some(Arc::clone(p));
                }
            }
        }
        for id in &self.provider_order {
            if let Some(p) = self.providers.get(id) {
                if p.plugin_id.eq_ignore_ascii_case(needle) {
                    return Some(Arc::clone(p));
                }
            }
        }
        for id in &self.provider_order {
            if let Some(p) = self.providers.get(id) {
                if p.display_name.eq_ignore_ascii_case(needle) {
                    return Some(Arc::clone(p));
                }
            }
        }
        None
    }

    pub(super) fn unavailable_provider_error(p: &AcpProviderRuntime) -> EngineError {
        let mut msg = format!(
            "agent provider `{}` ({}) is not available (executable not found)",
            p.id, p.display_name
        );
        if let Some(hint) = p
            .install_hint
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            msg.push_str(&format!("; install with: {hint}"));
        }
        EngineError::InvalidParams(msg)
    }

    pub(super) fn require_available_provider(
        &self,
        query: &str,
    ) -> Result<Arc<AcpProviderRuntime>, EngineError> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Err(EngineError::InvalidParams(
                "agent provider must not be empty".to_string(),
            ));
        }
        let Some(provider) = self.find_provider(trimmed) else {
            return Err(EngineError::InvalidParams(format!(
                "unknown agent provider `{trimmed}`"
            )));
        };
        if !provider.available {
            return Err(Self::unavailable_provider_error(&provider));
        }
        Ok(provider)
    }

    pub(super) fn effective_available_default_provider(&self) -> Option<Arc<AcpProviderRuntime>> {
        if let Some(prefs) = self.prefs.as_ref() {
            if let Ok(Some(pref_val)) = prefs.get("ui", "default_agent_provider") {
                if let Some(p) = self.find_provider(&pref_val).filter(|p| p.available) {
                    return Some(p);
                }
            }
        }
        if let Some(def_id) = self.default_provider_id.as_deref() {
            if let Some(p) = self.providers.get(def_id).filter(|p| p.available) {
                return Some(Arc::clone(p));
            }
        }
        self.provider_order
            .iter()
            .find_map(|id| self.providers.get(id).filter(|p| p.available).cloned())
    }

    pub(super) fn effective_available_default_provider_id(&self) -> Option<String> {
        self.effective_available_default_provider()
            .map(|p| p.id.clone())
    }

    pub(super) fn effective_display_default_id(&self) -> Option<String> {
        self.effective_available_default_provider_id()
            .or_else(|| self.provider_order.first().cloned())
    }

    pub(super) fn has_any_available_provider(&self) -> bool {
        self.provider_order
            .iter()
            .any(|id| self.providers.get(id).is_some_and(|p| p.available))
    }

    pub(super) fn resolve_runtime_for_thread(
        &self,
        thread: &Thread,
    ) -> Result<Option<Arc<AcpProviderRuntime>>, EngineError> {
        if let Some(raw) = thread
            .agent_provider
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if let Some(p) = self.find_provider(raw) {
                if p.available {
                    return Ok(Some(p));
                }
                return Err(Self::unavailable_provider_error(&p));
            }
            if self.providers.is_empty() {
                return Ok(None);
            }
            return Err(EngineError::InvalidParams(format!(
                "unknown agent provider `{raw}`"
            )));
        }
        Ok(self.effective_available_default_provider())
    }

    pub(super) fn clear_thread_from_all_observers(&self, thread_id: &str) {
        for id in &self.provider_order {
            if let Some(p) = self.providers.get(id) {
                p.subagent_observer.clear_thread(thread_id);
            }
        }
        self.subagent_observer.clear_thread(thread_id);
    }

    pub(super) fn shutdown_all_observers(&self) {
        for id in &self.provider_order {
            if let Some(p) = self.providers.get(id) {
                p.subagent_observer.shutdown();
            }
        }
        self.subagent_observer.shutdown();
    }

    pub(super) async fn shutdown_all_drivers(&self) {
        for id in &self.provider_order {
            if let Some(p) = self.providers.get(id) {
                p.shutdown_driver().await;
            }
        }
    }

    pub(super) fn shutdown_all_drivers_on_drop(&self) {
        let mut live_drivers = Vec::new();
        for id in &self.provider_order {
            if let Some(p) = self.providers.get(id) {
                if let Ok(mut guard) = p.acp_driver.try_lock() {
                    if let Some(slot) = guard.take() {
                        if slot.driver.is_running() {
                            live_drivers.push(slot.driver);
                        }
                    }
                }
            }
        }
        if !live_drivers.is_empty() {
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    for driver in live_drivers {
                        let _ = driver.shutdown().await;
                    }
                });
            }
        }
    }
}

impl InProcessEngine {
    pub(super) async fn list_agent_providers_impl(
        &self,
    ) -> Result<Vec<AgentProviderInfo>, EngineError> {
        let default_id = self.runtime.effective_display_default_id();
        let mut out: Vec<AgentProviderInfo> = self
            .runtime
            .provider_order
            .iter()
            .filter_map(|id| self.runtime.providers.get(id))
            .map(|p| {
                let is_default = default_id.as_deref() == Some(p.id.as_str());
                p.to_info(is_default)
            })
            .collect();
        out.sort_by_key(|p| !p.is_default);
        Ok(out)
    }

    pub(super) async fn switch_thread_provider_impl(
        &self,
        thread_id: &str,
        provider: &str,
    ) -> Result<String, EngineError> {
        let tid: ThreadId = parse_typed_id(thread_id)
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        let initial_thread = self
            .runtime
            .storage
            .get_thread(&tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        if initial_thread.kind == ThreadKind::Terminal {
            return Err(EngineError::InvalidParams(
                "cannot switch agent provider on a terminal thread".to_string(),
            ));
        }

        let target = self.runtime.require_available_provider(provider)?;
        let _turn_guard = self.runtime.acquire_thread_turn_lock(thread_id).await?;
        let fresh_thread = self
            .runtime
            .storage
            .get_thread(&tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;

        let provider_changed = fresh_thread.agent_provider.as_deref() != Some(target.id.as_str());
        let active_mismatched = {
            let state = self.runtime.lock()?;
            state
                .active_acp_sessions
                .get(thread_id)
                .is_some_and(|s| s.provider_id != target.id)
        };

        self.runtime
            .storage
            .update_thread_agent_provider(&tid, Some(&target.id))
            .map_err(storage_err)?;

        if provider_changed {
            let mut state = self.runtime.lock()?;
            state.pending_thread_config.remove(thread_id);
        }

        if provider_changed || active_mismatched {
            self.runtime
                .reset_thread_acp_session(tid, thread_id)
                .await?;
        }

        let meta = self.runtime.compute_thread_metadata()?;
        self.runtime
            .emit_tui(EngineStreamEvent::ThreadMetadataUpdated { meta });

        Ok(target.id.clone())
    }
}
