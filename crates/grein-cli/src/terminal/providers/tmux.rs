use grein_tui::terminal::{TerminalEnv, TerminalKind};

use crate::terminal::port::{
    ConfigFsPort, RevertReceipt, SetupPlan, SetupReceipt, TerminalSetupError,
    TerminalSetupProvider, TerminalStatusEntry,
};
use crate::terminal::providers::{
    apply_two_file_setup, detect_two_file_status, home_dir, plan_two_file_setup,
    remove_matching_line, revert_main_file_and_snippet, xdg_config_dir, TwoFileIncludeSpec,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct TmuxSetupProvider;

const TMUX_SNIPPET: &str = "\
set -g xterm-keys on
set -g extended-keys always
set -g extended-keys-format csi-u
set -as terminal-features ',*:RGB:extkeys'
set -sg escape-time 10
";

fn build_spec(fs: &dyn ConfigFsPort) -> Result<TwoFileIncludeSpec, TerminalSetupError> {
    let home = home_dir(fs)?;
    let xdg_tmux = xdg_config_dir(fs)?.join("tmux");
    let dot_tmux_conf = home.join(".tmux.conf");
    let xdg_tmux_conf = xdg_tmux.join("tmux.conf");

    let main_config_path = if fs.exists(&dot_tmux_conf) && !fs.exists(&xdg_tmux_conf) {
        dot_tmux_conf
    } else {
        xdg_tmux_conf
    };
    let snippet_path = xdg_tmux.join("grein.conf");
    let include_line = format!("source-file -q {}", snippet_path.display());

    Ok(TwoFileIncludeSpec {
        kind: TerminalKind::Tmux,
        main_config_path: main_config_path.clone(),
        snippet_path,
        snippet_content: TMUX_SNIPPET.to_string(),
        include_line,
        manual_steps: vec![format!(
            "Reload running tmux server with: tmux source-file {}",
            main_config_path.display()
        )],
    })
}

impl TerminalSetupProvider for TmuxSetupProvider {
    fn kind(&self) -> TerminalKind {
        TerminalKind::Tmux
    }

    fn display_name(&self) -> &'static str {
        "tmux"
    }

    fn detect_status(
        &self,
        env: &TerminalEnv,
        fs: &dyn ConfigFsPort,
    ) -> Result<TerminalStatusEntry, TerminalSetupError> {
        let spec = build_spec(fs)?;
        let install_status = detect_two_file_status(&spec, fs)?;
        let active_detected = env.tmux.is_some()
            || env
                .term_program
                .as_deref()
                .is_some_and(|tp| tp.eq_ignore_ascii_case("tmux"));

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
