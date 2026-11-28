use grein_tui::terminal::{TerminalEnv, TerminalKind};

use crate::terminal::port::{
    ConfigFsPort, RevertReceipt, SetupPlan, SetupReceipt, TerminalSetupError,
    TerminalSetupProvider, TerminalStatusEntry,
};
use crate::terminal::providers::{
    apply_two_file_setup, detect_two_file_status, plan_two_file_setup, remove_matching_line,
    revert_main_file_and_snippet, xdg_config_dir, TwoFileIncludeSpec,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct KittySetupProvider;

const KITTY_SNIPPET: &str = "macos_option_as_alt yes\n";
const KITTY_INCLUDE_LINE: &str = "include grein.conf";

fn build_spec(fs: &dyn ConfigFsPort) -> Result<TwoFileIncludeSpec, TerminalSetupError> {
    let dir = xdg_config_dir(fs)?.join("kitty");
    Ok(TwoFileIncludeSpec {
        kind: TerminalKind::Kitty,
        main_config_path: dir.join("kitty.conf"),
        snippet_path: dir.join("grein.conf"),
        snippet_content: KITTY_SNIPPET.to_string(),
        include_line: KITTY_INCLUDE_LINE.to_string(),
        manual_steps: vec!["Reload Kitty config (Ctrl+Shift+F5 on Linux or Cmd+Ctrl+, on macOS).".to_string()],
    })
}

impl TerminalSetupProvider for KittySetupProvider {
    fn kind(&self) -> TerminalKind {
        TerminalKind::Kitty
    }

    fn display_name(&self) -> &'static str {
        "Kitty"
    }

    fn detect_status(
        &self,
        env: &TerminalEnv,
        fs: &dyn ConfigFsPort,
    ) -> Result<TerminalStatusEntry, TerminalSetupError> {
        let spec = build_spec(fs)?;
        let install_status = detect_two_file_status(&spec, fs)?;
        let active_detected = env.kitty_window_id.is_some()
            || env
                .term
                .as_deref()
                .is_some_and(|t| t.eq_ignore_ascii_case("xterm-kitty"));

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
