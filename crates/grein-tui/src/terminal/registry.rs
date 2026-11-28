use std::str::FromStr;
use std::sync::Arc;

use crate::terminal::env::TerminalEnv;
use crate::terminal::port::{TerminalAdapter, TerminalError, TerminalKind};

#[derive(Default)]
pub struct TerminalRegistry {
    adapters: Vec<Arc<dyn TerminalAdapter>>,
}

impl TerminalRegistry {
    pub fn new() -> Self {
        Self {
            adapters: Vec::new(),
        }
    }

    pub fn register(&mut self, adapter: Arc<dyn TerminalAdapter>) {
        self.adapters.push(adapter);
    }

    pub fn adapters(&self) -> &[Arc<dyn TerminalAdapter>] {
        &self.adapters
    }

    pub fn resolve_by_kind(&self, kind: TerminalKind) -> Option<Arc<dyn TerminalAdapter>> {
        self.adapters
            .iter()
            .find(|adapter| adapter.kind() == kind)
            .cloned()
    }

    pub fn resolve(&self, env: &TerminalEnv) -> Option<Arc<dyn TerminalAdapter>> {
        if let Some(override_raw) = env.grein_terminal_override.as_deref() {
            if let Ok(kind) = TerminalKind::from_str(override_raw) {
                if let Some(matched) = self.resolve_by_kind(kind) {
                    return Some(matched);
                }
            }
        }

        let mut best: Option<(u8, &Arc<dyn TerminalAdapter>)> = None;
        for adapter in &self.adapters {
            if let Some(score) = adapter.matches(env) {
                match best {
                    Some((best_score, _)) if score <= best_score => {}
                    _ => best = Some((score, adapter)),
                }
            }
        }
        best.map(|(_, adapter)| Arc::clone(adapter))
    }

    pub fn resolve_or_err(
        &self,
        env: &TerminalEnv,
    ) -> Result<Arc<dyn TerminalAdapter>, TerminalError> {
        self.resolve(env).ok_or(TerminalError::EmptyRegistry)
    }
}
