use std::path::PathBuf;

use grein_tui::terminal::{TerminalEnv, TerminalKind};

use crate::terminal::port::{
    ConfigFsPort, OsFamily, RevertReceipt, SetupPlan, SetupReceipt, TerminalSetupError,
    TerminalSetupProvider, TerminalStatusEntry,
};
use crate::terminal::providers::{
    apply_two_file_setup, detect_two_file_status, home_dir, plan_two_file_setup,
    remove_matching_line, revert_main_file_and_snippet, xdg_config_dir, TwoFileIncludeSpec,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttySetupProvider;

const GHOSTTY_SNIPPET: &str = "\
macos-option-as-alt = true
keybind = shift+up=unbind
keybind = shift+down=unbind
keybind = shift+left=unbind
keybind = shift+right=unbind
";

const GHOSTTY_INCLUDE_LINE: &str = "config-file = ?grein.conf";

fn config_dir(fs: &dyn ConfigFsPort) -> Result<PathBuf, TerminalSetupError> {
    let xdg_dir = xdg_config_dir(fs)?.join("ghostty");
    if fs.os_family() == OsFamily::MacOs {
        let app_support = home_dir(fs)?.join("Library/Application Support/com.mitchellh.ghostty");
        if fs.exists(&xdg_dir.join("config")) && !fs.exists(&app_support.join("config")) {
            return Ok(xdg_dir);
        }
        return Ok(app_support);
    }
    Ok(xdg_dir)
}

fn build_spec(fs: &dyn ConfigFsPort) -> Result<TwoFileIncludeSpec, TerminalSetupError> {
    let dir = config_dir(fs)?;
    Ok(TwoFileIncludeSpec {
        kind: TerminalKind::Ghostty,
        main_config_path: dir.join("config"),
        snippet_path: dir.join("grein.conf"),
        snippet_content: GHOSTTY_SNIPPET.to_string(),
        include_line: GHOSTTY_INCLUDE_LINE.to_string(),
        manual_steps: vec!["Reload Ghostty configuration (Cmd+Shift+, on macOS or Ctrl+Shift+, on Linux).".to_string()],
    })
}

impl TerminalSetupProvider for GhosttySetupProvider {
    fn kind(&self) -> TerminalKind {
        TerminalKind::Ghostty
    }

    fn display_name(&self) -> &'static str {
        "Ghostty"
    }

    fn detect_status(
        &self,
        env: &TerminalEnv,
        fs: &dyn ConfigFsPort,
    ) -> Result<TerminalStatusEntry, TerminalSetupError> {
        let spec = build_spec(fs)?;
        let install_status = detect_two_file_status(&spec, fs)?;
        let active_detected = env.ghostty_resources_dir.is_some()
            || env
                .term_program
                .as_deref()
                .is_some_and(|tp| tp.eq_ignore_ascii_case("ghostty"));

        Ok(TerminalStatusEntry {
            kind: self.kind(),
            display_name: self.display_name().to_string(),
            active_detected,
            install_status,
            config_paths: vec![spec.main_config_path, spec.snippet_path],
            notes: Vec::new(),
        })
    }

    fn plan_setup(
        &self,
        _env: &TerminalEnv,
        fs: &dyn ConfigFsPort,
    ) -> Result<SetupPlan, TerminalSetupError> {
        let spec = build_spec(fs)?;
        plan_two_file_setup(&spec, fs)
    }

    fn apply_setup(
        &self,
        _env: &TerminalEnv,
        fs: &dyn ConfigFsPort,
    ) -> Result<SetupReceipt, TerminalSetupError> {
        let spec = build_spec(fs)?;
        apply_two_file_setup(&spec, fs)
    }

    fn revert_setup(
        &self,
        _env: &TerminalEnv,
        fs: &dyn ConfigFsPort,
    ) -> Result<RevertReceipt, TerminalSetupError> {
        let spec = build_spec(fs)?;
        revert_main_file_and_snippet(
            self.kind(),
            &spec.main_config_path,
            &spec.snippet_path,
            fs,
            |content| remove_matching_line(content, &spec.include_line),
        )
    }
}
