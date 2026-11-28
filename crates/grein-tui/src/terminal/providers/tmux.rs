use crate::terminal::env::TerminalEnv;
use crate::terminal::port::{
    ColorCapability, HintLevel, KeyboardProtocolPolicy, TerminalAdapter, TerminalCapabilities,
    TerminalHint, TerminalKind, TerminalProtocolPlan, TerminalSessionOptions,
};
use crate::terminal::providers::has_non_empty;

#[derive(Debug, Clone, Copy, Default)]
pub struct TmuxProvider;

impl TerminalAdapter for TmuxProvider {
    fn kind(&self) -> TerminalKind {
        TerminalKind::Tmux
    }

    fn display_name(&self) -> &'static str {
        TerminalKind::Tmux.display_name()
    }

    fn matches(&self, env: &TerminalEnv) -> Option<u8> {
        has_non_empty(&env.tmux).then_some(90)
    }

    fn capabilities(&self, env: &TerminalEnv) -> TerminalCapabilities {
        let color = if env.supports_truecolor_hint() {
            ColorCapability::TrueColor
        } else {
            ColorCapability::Xterm256
        };
        TerminalCapabilities {
            color,
            keyboard_protocol: KeyboardProtocolPolicy::ModifyOtherKeys,
            osc52_clipboard: true,
            focus_events: true,
            bracketed_paste: true,
            notes: vec![
                "tmux multiplexer adapter (modifyOtherKeys + DCS OSC 52 passthrough)".to_string(),
            ],
        }
    }

    fn protocol_plan(
        &self,
        _env: &TerminalEnv,
        opts: &TerminalSessionOptions,
    ) -> TerminalProtocolPlan {
        if opts.enhance_keyboard {
            TerminalProtocolPlan {
                enter_bytes: b"\x1b[>4;2m\x1b[>1;2m".to_vec(),
                probe_kitty_timeout_ms: None,
                expects_kitty_pop: false,
                leave_bytes: b"\x1b[>4m\x1b[>1m".to_vec(),
            }
        } else {
            TerminalProtocolPlan::default()
        }
    }

    fn encode_osc52(&self, payload_b64: &str) -> String {
        format!("\x1bPtmux;\x1b\x1b]52;c;{payload_b64}\x07\x1b\\")
    }

    fn diagnostic_hints(&self, env: &TerminalEnv) -> Vec<TerminalHint> {
        let mut hints = vec![TerminalHint {
            level: HintLevel::Info,
            message: "tmux detected: run `grein terminal setup tmux` to configure `xterm-keys on`, `extended-keys always`, and `csi-u` / allow-passthrough.".to_string(),
        }];
        if !env.supports_truecolor_hint() {
            hints.push(TerminalHint {
                level: HintLevel::Info,
                message: "tmux truecolor hint not detected; enable terminal-features RGB or COLORTERM=truecolor.".to_string(),
            });
        }
        hints
    }
}
