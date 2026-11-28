use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalEnv {
    pub term: Option<String>,
    pub term_program: Option<String>,
    pub term_program_version: Option<String>,
    pub colorterm: Option<String>,
    pub kitty_window_id: Option<String>,
    pub ghostty_resources_dir: Option<String>,
    pub wezterm_executable: Option<String>,
    pub tmux: Option<String>,
    pub zellij: Option<String>,
    pub zellij_session_name: Option<String>,
    pub sty: Option<String>,
    pub locale: Option<String>,
    pub grein_terminal_override: Option<String>,
}

fn read_non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn normalize_opt(value: impl Into<String>) -> Option<String> {
    let trimmed = value.into().trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

impl TerminalEnv {
    pub fn from_env() -> Self {
        let locale = read_non_empty_env("LC_ALL")
            .or_else(|| read_non_empty_env("LC_CTYPE"))
            .or_else(|| read_non_empty_env("LANG"));

        Self {
            term: read_non_empty_env("TERM"),
            term_program: read_non_empty_env("TERM_PROGRAM"),
            term_program_version: read_non_empty_env("TERM_PROGRAM_VERSION"),
            colorterm: read_non_empty_env("COLORTERM"),
            kitty_window_id: read_non_empty_env("KITTY_WINDOW_ID"),
            ghostty_resources_dir: read_non_empty_env("GHOSTTY_RESOURCES_DIR"),
            wezterm_executable: read_non_empty_env("WEZTERM_EXECUTABLE"),
            tmux: read_non_empty_env("TMUX"),
            zellij: read_non_empty_env("ZELLIJ"),
            zellij_session_name: read_non_empty_env("ZELLIJ_SESSION_NAME"),
            sty: read_non_empty_env("STY"),
            locale,
            grein_terminal_override: read_non_empty_env("GREIN_TERMINAL"),
        }
    }

    pub fn with_term(mut self, value: impl Into<String>) -> Self {
        self.term = normalize_opt(value);
        self
    }

    pub fn with_term_program(mut self, value: impl Into<String>) -> Self {
        self.term_program = normalize_opt(value);
        self
    }

    pub fn with_term_program_version(mut self, value: impl Into<String>) -> Self {
        self.term_program_version = normalize_opt(value);
        self
    }

    pub fn with_colorterm(mut self, value: impl Into<String>) -> Self {
        self.colorterm = normalize_opt(value);
        self
    }

    pub fn with_kitty_window_id(mut self, value: impl Into<String>) -> Self {
        self.kitty_window_id = normalize_opt(value);
        self
    }

    pub fn with_ghostty_resources_dir(mut self, value: impl Into<String>) -> Self {
        self.ghostty_resources_dir = normalize_opt(value);
        self
    }

    pub fn with_wezterm_executable(mut self, value: impl Into<String>) -> Self {
        self.wezterm_executable = normalize_opt(value);
        self
    }

    pub fn with_tmux(mut self, value: impl Into<String>) -> Self {
        self.tmux = normalize_opt(value);
        self
    }

    pub fn with_zellij(mut self, value: impl Into<String>) -> Self {
        self.zellij = normalize_opt(value);
        self
    }

    pub fn with_zellij_session_name(mut self, value: impl Into<String>) -> Self {
        self.zellij_session_name = normalize_opt(value);
        self
    }

    pub fn with_sty(mut self, value: impl Into<String>) -> Self {
        self.sty = normalize_opt(value);
        self
    }

    pub fn with_locale(mut self, value: impl Into<String>) -> Self {
        self.locale = normalize_opt(value);
        self
    }

    pub fn with_grein_terminal_override(mut self, value: impl Into<String>) -> Self {
        self.grein_terminal_override = normalize_opt(value);
        self
    }

    pub fn supports_truecolor_hint(&self) -> bool {
        let colorterm_truecolor = self.colorterm.as_deref().is_some_and(|v| {
            v.eq_ignore_ascii_case("truecolor") || v.eq_ignore_ascii_case("24bit")
        });
        let term_truecolor = self.term.as_deref().is_some_and(|t| {
            let lower = t.to_ascii_lowercase();
            lower.contains("truecolor") || lower.contains("direct")
        });
        colorterm_truecolor || term_truecolor
    }
}
