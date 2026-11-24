use std::io;
use std::sync::atomic::Ordering;

use grein_tui::engine_port::{AgentProviderInfo, HydratedThread, TuiEngineEvent};

use super::tui_to_engine::{is_unsupported_engine_error, to_io, EngineClientPort};
use crate::engine::{GreinEngine, ThreadSummary};

impl<E: GreinEngine> EngineClientPort<E> {
    pub(super) async fn finish_forked_thread(
        &self,
        forked: ThreadSummary,
    ) -> io::Result<HydratedThread> {
        let turns = self.load_hydrated_turns(&forked.thread_id).await?;
        let hydrated = HydratedThread {
            id: forked.thread_id,
            title: forked.title,
            custom_name: forked.custom_name,
            agent_provider: forked.agent_provider,
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

    pub(super) async fn list_agent_providers_impl(&self) -> io::Result<Vec<AgentProviderInfo>> {
        match self.client.list_agent_providers().await {
            Ok(providers) => Ok(providers),
            Err(err) if is_unsupported_engine_error(&err) => Ok(Vec::new()),
            Err(err) => Err(to_io(err)),
        }
    }

    pub(super) async fn create_agent_thread_impl(
        &self,
        title: Option<&str>,
        cwd: Option<&str>,
        parent_thread_id: Option<&str>,
        agent_provider: Option<&str>,
    ) -> io::Result<HydratedThread> {
        let created = match self
            .client
            .create_agent_thread(title, cwd, parent_thread_id, agent_provider)
            .await
        {
            Ok(created) => created,
            Err(err)
                if is_unsupported_engine_error(&err)
                    && parent_thread_id.is_none()
                    && agent_provider.is_none() =>
            {
                match self.client.create_root_thread_in_cwd(title, cwd).await {
                    Ok(created) => created,
                    Err(e2) if is_unsupported_engine_error(&e2) => {
                        self.client.create_root_thread(title).await.map_err(to_io)?
                    }
                    Err(e2) => return Err(to_io(e2)),
                }
            }
            Err(err) => return Err(to_io(err)),
        };
        Ok(self.finish_created_root(created).await)
    }

    pub(super) async fn fork_branch_with_provider_impl(
        &self,
        source_thread_id: &str,
        title: Option<&str>,
        agent_provider: Option<&str>,
    ) -> io::Result<HydratedThread> {
        let forked = match self
            .client
            .fork_thread_with_provider(Some(source_thread_id), title, agent_provider)
            .await
        {
            Ok(forked) => forked,
            Err(err) if is_unsupported_engine_error(&err) && agent_provider.is_none() => self
                .client
                .fork_thread_with_title(Some(source_thread_id), title)
                .await
                .map_err(to_io)?,
            Err(err) => return Err(to_io(err)),
        };
        self.finish_forked_thread(forked).await
    }

    pub(super) async fn switch_thread_provider_impl(
        &self,
        thread_id: &str,
        provider: &str,
    ) -> io::Result<String> {
        let resolved = self
            .client
            .switch_thread_provider(thread_id, provider)
            .await
            .map_err(|err| {
                if is_unsupported_engine_error(&err) {
                    io::Error::new(io::ErrorKind::Unsupported, err.to_string())
                } else {
                    to_io(err)
                }
            })?;
        if !self.has_live_stream.load(Ordering::SeqCst) {
            let _ = self.fetch_and_emit_thread_metadata().await;
            if let Ok(opts) = self.client.agent_options(thread_id).await {
                self.emit(TuiEngineEvent::AgentConfigUpdated {
                    thread_id: thread_id.to_string(),
                    config_options: opts.config_options,
                });
                self.emit(TuiEngineEvent::AgentCommandsUpdated {
                    thread_id: thread_id.to_string(),
                    commands: opts.available_commands,
                });
            }
        }
        Ok(resolved)
    }
}
