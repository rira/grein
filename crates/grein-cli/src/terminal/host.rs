use std::str::FromStr;
use std::sync::Arc;

use grein_tui::terminal::{TerminalEnv, TerminalKind, TerminalRegistry};

use crate::terminal::port::{
    ConfigFsPort, RevertReceipt, SetupPlan, SetupReceipt, TerminalSetupError,
    TerminalSetupProvider, TerminalStatusEntry,
};

pub struct TerminalSetupHost {
    installers: Vec<Box<dyn TerminalSetupProvider>>,
    fs: Arc<dyn ConfigFsPort>,
    registry: Option<TerminalRegistry>,
}

impl TerminalSetupHost {
    pub fn new(installers: Vec<Box<dyn TerminalSetupProvider>>, fs: Arc<dyn ConfigFsPort>) -> Self {
        Self {
            installers,
            fs,
            registry: None,
        }
    }

    pub fn with_registry(mut self, registry: TerminalRegistry) -> Self {
        self.registry = Some(registry);
        self
    }

    pub fn active_terminal(&self, env: &TerminalEnv) -> TerminalKind {
        detect_active_terminal_kind(env, self.registry.as_ref())
    }

    pub fn registered_kinds(&self) -> Vec<TerminalKind> {
        self.installers.iter().map(|p| p.kind()).collect()
    }

    pub fn status(&self, env: &TerminalEnv) -> Result<Vec<TerminalStatusEntry>, TerminalSetupError> {
        let active = self.active_terminal(env);
        let mut entries = Vec::with_capacity(self.installers.len());
        for installer in &self.installers {
            let mut entry = installer.detect_status(env, self.fs.as_ref())?;
            entry.active_detected = entry.kind == active;
            entries.push(entry);
        }
        Ok(entries)
    }

    pub fn resolve_target(
        &self,
        target_or_auto: &str,
        env: &TerminalEnv,
    ) -> Result<TerminalKind, TerminalSetupError> {
        let trimmed = target_or_auto.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
            return Ok(self.active_terminal(env));
        }
        TerminalKind::from_str(trimmed)
            .map_err(|_| TerminalSetupError::UnknownTerminal(trimmed.to_string()))
    }

    pub fn plan(
        &self,
        target_or_auto: &str,
        env: &TerminalEnv,
    ) -> Result<SetupPlan, TerminalSetupError> {
        let kind = self.resolve_target(target_or_auto, env)?;
        let installer = self.find_installer(kind)?;
        installer.plan_setup(env, self.fs.as_ref())
    }

    pub fn apply(
        &self,
        target_or_auto: &str,
        env: &TerminalEnv,
    ) -> Result<SetupReceipt, TerminalSetupError> {
        let kind = self.resolve_target(target_or_auto, env)?;
        let installer = self.find_installer(kind)?;
        installer.apply_setup(env, self.fs.as_ref())
    }

    pub fn revert(
        &self,
        target_or_auto: &str,
        env: &TerminalEnv,
    ) -> Result<RevertReceipt, TerminalSetupError> {
        let kind = self.resolve_target(target_or_auto, env)?;
        let installer = self.find_installer(kind)?;
        installer.revert_setup(env, self.fs.as_ref())
    }

    fn find_installer(
        &self,
        kind: TerminalKind,
    ) -> Result<&dyn TerminalSetupProvider, TerminalSetupError> {
        self.installers
            .iter()
            .find(|installer| installer.kind() == kind)
            .map(Box::as_ref)
            .ok_or(TerminalSetupError::UnsupportedTerminal(kind))
    }
}

fn detect_active_terminal_kind(
    env: &TerminalEnv,
    registry: Option<&TerminalRegistry>,
) -> TerminalKind {
    if let Some(raw) = env.grein_terminal_override.as_deref() {
        if let Ok(kind) = TerminalKind::from_str(raw) {
            return kind;
        }
    }

    if let Some(reg) = registry {
        if let Some(adapter) = reg.resolve(env) {
            if adapter.kind() != TerminalKind::GenericXterm {
                return adapter.kind();
            }
        }
    }

    if env.tmux.is_some() {
        return TerminalKind::Tmux;
    }
    if env.zellij.is_some() || env.zellij_session_name.is_some() {
        return TerminalKind::Zellij;
    }
    if env.kitty_window_id.is_some()
        || env
            .term
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case("xterm-kitty"))
    {
        return TerminalKind::Kitty;
    }
    if env.ghostty_resources_dir.is_some()
        || env
            .term
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case("xterm-ghostty"))
    {
        return TerminalKind::Ghostty;
    }
    if env.wezterm_executable.is_some() {
        return TerminalKind::WezTerm;
    }
    if let Some(tp) = env.term_program.as_deref() {
        let lower = tp.to_ascii_lowercase();
        if lower == "apple_terminal" {
            return TerminalKind::AppleTerminal;
        }
        if lower == "iterm.app" || lower == "iterm2" {
            return TerminalKind::ITerm2;
        }
        if lower == "ghostty" {
            return TerminalKind::Ghostty;
        }
        if lower == "wezterm" {
            return TerminalKind::WezTerm;
        }
        if lower == "alacritty" {
            return TerminalKind::Alacritty;
        }
        if lower == "kitty" {
            return TerminalKind::Kitty;
        }
        if lower == "tmux" {
            return TerminalKind::Tmux;
        }
        if lower == "zellij" {
            return TerminalKind::Zellij;
        }
    }
    if env
        .term
        .as_deref()
        .is_some_and(|t| t.to_ascii_lowercase().contains("alacritty"))
    {
        return TerminalKind::Alacritty;
    }

    TerminalKind::GenericXterm
}
