use crate::terminal::env::TerminalEnv;
use crate::terminal::port::{
    ColorCapability, HintLevel, KeyboardProtocolPolicy, TerminalAdapter, TerminalCapabilities,
    TerminalHint, TerminalKind, TerminalProtocolPlan, TerminalSessionOptions,
};
use crate::terminal::providers::{
    has_non_empty, kitty_disambiguate_plan, matches_term_program, standard_osc52,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttyProvider;

impl TerminalAdapter for GhosttyProvider {
    fn kind(&self) -> TerminalKind {
        TerminalKind::Ghostty
    }

    fn display_name(&self) -> &'static str {
        TerminalKind::Ghostty.display_name()
    }

    fn matches(&self, env: &TerminalEnv) -> Option<u8> {
        (matches_term_program(env, "ghostty") || has_non_empty(&env.ghostty_resources_dir))
            .then_some(70)
    }

    fn capabilities(&self, _env: &TerminalEnv) -> TerminalCapabilities {
        TerminalCapabilities {
            color: ColorCapability::TrueColor,
            keyboard_protocol: KeyboardProtocolPolicy::KittyDisambiguate,
            osc52_clipboard: true,
            focus_events: true,
            bracketed_paste: true,
            notes: vec!["Ghostty adapter (TrueColor, Kitty disambiguate, OSC 52)".to_string()],
        }
    }

    fn protocol_plan(
        &self,
        _env: &TerminalEnv,
        opts: &TerminalSessionOptions,
    ) -> TerminalProtocolPlan {
        kitty_disambiguate_plan(opts)
    }

    fn encode_osc52(&self, payload_b64: &str) -> String {
        standard_osc52(payload_b64)
    }

    fn diagnostic_hints(&self, _env: &TerminalEnv) -> Vec<TerminalHint> {
        vec![TerminalHint {
            level: HintLevel::Info,
            message: "Ghostty detected: run `grein terminal setup ghostty` for recommended `macos-option-as-alt` configuration.".to_string(),
        }]
    }
}
