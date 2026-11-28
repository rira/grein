use crate::terminal::env::TerminalEnv;
use crate::terminal::port::{
    ColorCapability, HintLevel, KeyboardProtocolPolicy, TerminalAdapter, TerminalCapabilities,
    TerminalHint, TerminalKind, TerminalProtocolPlan, TerminalSessionOptions,
};
use crate::terminal::providers::{
    bounded_probe_kitty_plan, has_non_empty, standard_osc52, ZELLIJ_KITTY_PROBE_TIMEOUT_MS,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct ZellijProvider;

impl TerminalAdapter for ZellijProvider {
    fn kind(&self) -> TerminalKind {
        TerminalKind::Zellij
    }

    fn display_name(&self) -> &'static str {
        TerminalKind::Zellij.display_name()
    }

    fn matches(&self, env: &TerminalEnv) -> Option<u8> {
        (has_non_empty(&env.zellij) || has_non_empty(&env.zellij_session_name)).then_some(85)
    }

    fn capabilities(&self, _env: &TerminalEnv) -> TerminalCapabilities {
        TerminalCapabilities {
            color: ColorCapability::TrueColor,
            keyboard_protocol: KeyboardProtocolPolicy::BoundedProbeKitty {
                timeout_ms: ZELLIJ_KITTY_PROBE_TIMEOUT_MS,
            },
            osc52_clipboard: true,
            focus_events: true,
            bracketed_paste: true,
            notes: vec![
                "Zellij multiplexer adapter (bounded Kitty keyboard protocol probe)".to_string(),
            ],
        }
    }

    fn protocol_plan(
        &self,
        _env: &TerminalEnv,
        opts: &TerminalSessionOptions,
    ) -> TerminalProtocolPlan {
        bounded_probe_kitty_plan(opts, ZELLIJ_KITTY_PROBE_TIMEOUT_MS)
    }

    fn encode_osc52(&self, payload_b64: &str) -> String {
        standard_osc52(payload_b64)
    }

    fn diagnostic_hints(&self, _env: &TerminalEnv) -> Vec<TerminalHint> {
        vec![TerminalHint {
            level: HintLevel::Info,
            message: "Zellij detected: run `grein terminal setup zellij` to ensure `support_kitty_keyboard_protocol true` is enabled.".to_string(),
        }]
    }
}
