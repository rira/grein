pub mod alacritty;
pub mod apple_terminal;
pub mod generic;
pub mod ghostty;
pub mod iterm2;
pub mod kitty;
pub mod tmux;
pub mod wezterm;
pub mod zellij;

use std::sync::Arc;

use crate::terminal::env::TerminalEnv;
use crate::terminal::port::{TerminalAdapter, TerminalProtocolPlan, TerminalSessionOptions};
use crate::terminal::registry::TerminalRegistry;

pub use alacritty::AlacrittyProvider;
pub use apple_terminal::AppleTerminalProvider;
pub use generic::GenericXtermProvider;
pub use ghostty::GhosttyProvider;
pub use iterm2::ITerm2Provider;
pub use kitty::KittyProvider;
pub use tmux::TmuxProvider;
pub use wezterm::WezTermProvider;
pub use zellij::ZellijProvider;

pub(super) const ZELLIJ_KITTY_PROBE_TIMEOUT_MS: u64 = 120;
pub(super) const WEZTERM_KITTY_PROBE_TIMEOUT_MS: u64 = 120;

pub(super) fn has_non_empty(value: &Option<String>) -> bool {
    value.as_deref().is_some_and(|v| !v.trim().is_empty())
}

pub(super) fn matches_term_program(env: &TerminalEnv, expected: &str) -> bool {
    env.term_program
        .as_deref()
        .is_some_and(|tp| tp.trim().eq_ignore_ascii_case(expected))
}

pub(super) fn term_contains(env: &TerminalEnv, needle: &str) -> bool {
    env.term
        .as_deref()
        .is_some_and(|t| t.to_ascii_lowercase().contains(&needle.to_ascii_lowercase()))
}

pub(super) fn standard_osc52(payload_b64: &str) -> String {
    format!("\x1b]52;c;{payload_b64}\x07")
}

pub(super) fn kitty_disambiguate_plan(opts: &TerminalSessionOptions) -> TerminalProtocolPlan {
    if opts.enhance_keyboard {
        TerminalProtocolPlan {
            enter_bytes: b"\x1b[>1u".to_vec(),
            probe_kitty_timeout_ms: None,
            expects_kitty_pop: true,
            leave_bytes: b"\x1b[<u".to_vec(),
        }
    } else {
        TerminalProtocolPlan::default()
    }
}

pub(super) fn bounded_probe_kitty_plan(
    opts: &TerminalSessionOptions,
    timeout_ms: u64,
) -> TerminalProtocolPlan {
    if opts.enhance_keyboard {
        TerminalProtocolPlan {
            enter_bytes: b"\x1b[>1u".to_vec(),
            probe_kitty_timeout_ms: Some(timeout_ms),
            expects_kitty_pop: true,
            leave_bytes: b"\x1b[<u".to_vec(),
        }
    } else {
        TerminalProtocolPlan::default()
    }
}

pub fn default_registry() -> TerminalRegistry {
    let mut registry = TerminalRegistry::new();
    registry.register(Arc::new(TmuxProvider));
    registry.register(Arc::new(ZellijProvider));
    registry.register(Arc::new(AppleTerminalProvider));
    registry.register(Arc::new(ITerm2Provider));
    registry.register(Arc::new(GhosttyProvider));
    registry.register(Arc::new(KittyProvider));
    registry.register(Arc::new(WezTermProvider));
    registry.register(Arc::new(AlacrittyProvider));
    registry.register(Arc::new(GenericXtermProvider));
    registry
}

pub fn default_adapter(env: &TerminalEnv) -> Arc<dyn TerminalAdapter> {
    default_registry()
        .resolve(env)
        .unwrap_or_else(|| Arc::new(GenericXtermProvider))
}
