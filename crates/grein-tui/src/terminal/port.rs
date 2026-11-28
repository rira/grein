use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::terminal::env::TerminalEnv;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TerminalKind {
    AppleTerminal,
    ITerm2,
    Ghostty,
    Kitty,
    WezTerm,
    Alacritty,
    Tmux,
    Zellij,
    GenericXterm,
}

impl TerminalKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AppleTerminal => "apple-terminal",
            Self::ITerm2 => "iterm2",
            Self::Ghostty => "ghostty",
            Self::Kitty => "kitty",
            Self::WezTerm => "wezterm",
            Self::Alacritty => "alacritty",
            Self::Tmux => "tmux",
            Self::Zellij => "zellij",
            Self::GenericXterm => "generic-xterm",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::AppleTerminal => "Apple Terminal",
            Self::ITerm2 => "iTerm2",
            Self::Ghostty => "Ghostty",
            Self::Kitty => "Kitty",
            Self::WezTerm => "WezTerm",
            Self::Alacritty => "Alacritty",
            Self::Tmux => "tmux",
            Self::Zellij => "Zellij",
            Self::GenericXterm => "Generic xterm",
        }
    }
}

impl fmt::Display for TerminalKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TerminalKind {
    type Err = TerminalError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = s.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "apple-terminal" | "apple_terminal" | "appleterminal" | "terminal.app" => {
                Ok(Self::AppleTerminal)
            }
            "iterm2" | "iterm" | "iterm.app" => Ok(Self::ITerm2),
            "ghostty" => Ok(Self::Ghostty),
            "kitty" | "xterm-kitty" => Ok(Self::Kitty),
            "wezterm" => Ok(Self::WezTerm),
            "alacritty" => Ok(Self::Alacritty),
            "tmux" => Ok(Self::Tmux),
            "zellij" => Ok(Self::Zellij),
            "generic-xterm" | "generic_xterm" | "generic" | "xterm" | "xterm-256color" => {
                Ok(Self::GenericXterm)
            }
            _ => Err(TerminalError::UnknownTerminalKind(s.trim().to_string())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ColorCapability {
    TrueColor,
    Xterm256,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KeyboardProtocolPolicy {
    None,
    KittyDisambiguate,
    ModifyOtherKeys,
    BoundedProbeKitty { timeout_ms: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalCapabilities {
    pub color: ColorCapability,
    pub keyboard_protocol: KeyboardProtocolPolicy,
    pub osc52_clipboard: bool,
    pub focus_events: bool,
    pub bracketed_paste: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSessionOptions {
    pub alternate_screen: bool,
    pub mouse_capture: bool,
    pub bracketed_paste: bool,
    pub focus_change: bool,
    pub enhance_keyboard: bool,
    pub raw_bytes: bool,
}

impl Default for TerminalSessionOptions {
    fn default() -> Self {
        Self {
            alternate_screen: true,
            mouse_capture: true,
            bracketed_paste: true,
            focus_change: true,
            enhance_keyboard: true,
            raw_bytes: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveTerminalModes {
    pub raw_mode: bool,
    pub alternate_screen: bool,
    pub mouse_capture: bool,
    pub bracketed_paste: bool,
    pub focus_change: bool,
    pub keyboard_enhanced: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalProtocolPlan {
    pub enter_bytes: Vec<u8>,
    pub probe_kitty_timeout_ms: Option<u64>,
    pub expects_kitty_pop: bool,
    pub leave_bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HintLevel {
    Info,
    Warn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalHint {
    pub level: HintLevel,
    pub message: String,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TerminalError {
    #[error("unknown terminal kind: {0}")]
    UnknownTerminalKind(String),
    #[error("no terminal adapter registered")]
    EmptyRegistry,
    #[error("terminal I/O error: {0}")]
    Io(String),
}

impl From<std::io::Error> for TerminalError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

pub trait TerminalAdapter: Send + Sync {
    fn kind(&self) -> TerminalKind;
    fn display_name(&self) -> &'static str;
    fn matches(&self, env: &TerminalEnv) -> Option<u8>;
    fn capabilities(&self, env: &TerminalEnv) -> TerminalCapabilities;
    fn protocol_plan(&self, env: &TerminalEnv, opts: &TerminalSessionOptions) -> TerminalProtocolPlan;
    fn encode_osc52(&self, payload_b64: &str) -> String;
    fn diagnostic_hints(&self, env: &TerminalEnv) -> Vec<TerminalHint>;
}
