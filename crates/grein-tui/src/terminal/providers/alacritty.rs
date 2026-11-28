use crate::terminal::env::TerminalEnv;
use crate::terminal::port::{
    ColorCapability, HintLevel, KeyboardProtocolPolicy, TerminalAdapter, TerminalCapabilities,
    TerminalHint, TerminalKind, TerminalProtocolPlan, TerminalSessionOptions,
};
use crate::terminal::providers::{
    kitty_disambiguate_plan, matches_term_program, standard_osc52, term_contains,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct AlacrittyProvider;

impl TerminalAdapter for AlacrittyProvider {
    fn kind(&self) -> TerminalKind {
        TerminalKind::Alacritty
    }

    fn display_name(&self) -> &'static str {
        TerminalKind::Alacritty.display_name()
    }

    fn matches(&self, env: &TerminalEnv) -> Option<u8> {
        (matches_term_program(env, "Alacritty") || term_contains(env, "alacritty")).then_some(65)
    }

    fn capabilities(&self, _env: &TerminalEnv) -> TerminalCapabilities {
        TerminalCapabilities {
            color: ColorCapability::TrueColor,
            keyboard_protocol: KeyboardProtocolPolicy::KittyDisambiguate,
            osc52_clipboard: true,
            focus_events: true,
            bracketed_paste: true,
            notes: vec!["Alacritty adapter (TrueColor, Kitty disambiguate, OSC 52)".to_string()],
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
            message: "Alacritty detected: run `grein terminal setup alacritty` for Option/Alt key binding tips.".to_string(),
        }]
    }
}
