pub mod generic;

use std::sync::Arc;

use crate::terminal::env::TerminalEnv;
use crate::terminal::port::TerminalAdapter;
use crate::terminal::registry::TerminalRegistry;

pub use generic::GenericXtermProvider;

pub fn default_registry() -> TerminalRegistry {
    let mut registry = TerminalRegistry::new();
    registry.register(Arc::new(GenericXtermProvider));
    registry
}

pub fn default_adapter(env: &TerminalEnv) -> Arc<dyn TerminalAdapter> {
    default_registry()
        .resolve(env)
        .unwrap_or_else(|| Arc::new(GenericXtermProvider))
}
