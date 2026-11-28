use std::path::PathBuf;

use grein_tui::terminal::{TerminalEnv, TerminalKind};

use crate::terminal::port::{
    ChangeAction, ConfigFsPort, InstallStatus, PlannedChange, RevertReceipt, SetupPlan,
    SetupReceipt, TerminalSetupError, TerminalSetupProvider, TerminalStatusEntry,
};
use crate::terminal::providers::{
    ensure_backup_if_needed, has_marked_block, revert_main_file_and_snippet, strip_marked_block,
    upsert_marked_block, xdg_config_dir,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct ZellijSetupProvider;

const BEGIN_MARKER: &str = "// BEGIN GREIN TERMINAL CONFIG";
const END_MARKER: &str = "// END GREIN TERMINAL CONFIG";
const KDL_SETTING: &str = "support_kitty_keyboard_protocol true";
const ZELLIJ_SNIPPET: &str = "\
// Grein Zellij keyboard protocol snippet
support_kitty_keyboard_protocol true
";

fn resolve_paths(fs: &dyn ConfigFsPort) -> Result<(PathBuf, PathBuf), TerminalSetupError> {
    let dir = xdg_config_dir(fs)?.join("zellij");
    Ok((dir.join("config.kdl"), dir.join("grein-snippet.kdl")))
}

impl TerminalSetupProvider for ZellijSetupProvider {
    fn kind(&self) -> TerminalKind {
        TerminalKind::Zellij
    }

    fn display_name(&self) -> &'static str {
        "Zellij"
    }

    fn detect_status(
        &self,
        env: &TerminalEnv,
        fs: &dyn ConfigFsPort,
    ) -> Result<TerminalStatusEntry, TerminalSetupError> {
        let (main_path, snippet_path) = resolve_paths(fs)?;
        let snippet_ok = fs
            .read_to_string(&snippet_path)?
            .is_some_and(|c| c == ZELLIJ_SNIPPET);
        let main_ok = fs
            .read_to_string(&main_path)?
            .is_some_and(|c| has_marked_block(&c, BEGIN_MARKER, END_MARKER) && c.contains(KDL_SETTING));

        let install_status = if snippet_ok && main_ok {
            InstallStatus::Configured
        } else if snippet_ok || main_ok || fs.exists(&snippet_path) {
            InstallStatus::Partial
        } else {
            InstallStatus::NotConfigured
        };

        let active_detected = env.zellij.is_some()
            || env.zellij_session_name.is_some()
            || env
                .term_program
                .as_deref()
                .is_some_and(|tp| tp.eq_ignore_ascii_case("zellij"));

        Ok(TerminalStatusEntry {
            kind: self.kind(),
            display_name: self.display_name().to_string(),
            active_detected,
            install_status,
            config_paths: vec![main_path, snippet_path],
            notes: Vec::new(),
        })
    }

    fn plan_setup(
        &self,
        env: &TerminalEnv,
        fs: &dyn ConfigFsPort,
    ) -> Result<SetupPlan, TerminalSetupError> {
        let status = self.detect_status(env, fs)?;
        let (main_path, snippet_path) = resolve_paths(fs)?;
        let mut changes = Vec::new();

        let existing_snippet = fs.read_to_string(&snippet_path)?;
        if existing_snippet.as_deref() != Some(ZELLIJ_SNIPPET) {
            changes.push(PlannedChange {
                path: snippet_path,
                action: if existing_snippet.is_some() {
                    ChangeAction::Update
                } else {
                    ChangeAction::Create
                },
                summary: "Write Zellij grein-snippet.kdl".to_string(),
                snippet: ZELLIJ_SNIPPET.to_string(),
            });
        }

        let existing_main = fs.read_to_string(&main_path)?;
        let main_ok = existing_main
            .as_deref()
            .is_some_and(|c| has_marked_block(c, BEGIN_MARKER, END_MARKER) && c.contains(KDL_SETTING));
        if !main_ok {
            changes.push(PlannedChange {
                path: main_path,
                action: if existing_main.is_some() {
                    ChangeAction::Update
                } else {
                    ChangeAction::Create
                },
                summary: "Insert marked support_kitty_keyboard_protocol block into config.kdl"
                    .to_string(),
                snippet: format!("{BEGIN_MARKER}\n{KDL_SETTING}\n{END_MARKER}"),
            });
        }

        Ok(SetupPlan {
            terminal: self.kind(),
            dry_run: true,
            status_before: status.install_status,
            changes,
            warnings: Vec::new(),
            manual_steps: vec!["Restart Zellij sessions for config.kdl changes to take effect.".to_string()],
        })
    }

    fn apply_setup(
        &self,
        env: &TerminalEnv,
        fs: &dyn ConfigFsPort,
    ) -> Result<SetupReceipt, TerminalSetupError> {
        let (main_path, snippet_path) = resolve_paths(fs)?;
        let mut changed_paths = Vec::new();
        let mut backup_paths = Vec::new();

        let existing_snippet = fs.read_to_string(&snippet_path)?;
        if existing_snippet.as_deref() != Some(ZELLIJ_SNIPPET) {
            ensure_backup_if_needed(fs, &snippet_path, &mut backup_paths)?;
            fs.write_atomic(&snippet_path, ZELLIJ_SNIPPET)?;
            changed_paths.push(snippet_path);
        }

        let existing_main = fs.read_to_string(&main_path)?;
        let main_ok = existing_main
            .as_deref()
            .is_some_and(|c| has_marked_block(c, BEGIN_MARKER, END_MARKER) && c.contains(KDL_SETTING));
        if !main_ok {
            ensure_backup_if_needed(fs, &main_path, &mut backup_paths)?;
            let updated = upsert_marked_block(
                existing_main.as_deref(),
                BEGIN_MARKER,
                END_MARKER,
                KDL_SETTING,
            );
            fs.write_atomic(&main_path, &updated)?;
            changed_paths.push(main_path);
        }

        Ok(SetupReceipt {
            terminal: self.kind(),
            applied: !changed_paths.is_empty(),
            status_after: self.detect_status(env, fs)?.install_status,
            changed_paths,
            backup_paths,
            warnings: Vec::new(),
            manual_steps: vec!["Restart Zellij sessions to apply updated config.kdl.".to_string()],
        })
    }

    fn revert_setup(
        &self,
        _env: &TerminalEnv,
        fs: &dyn ConfigFsPort,
    ) -> Result<RevertReceipt, TerminalSetupError> {
        let (main_path, snippet_path) = resolve_paths(fs)?;
        revert_main_file_and_snippet(
            self.kind(),
            &main_path,
            &snippet_path,
            fs,
            |content| strip_marked_block(content, BEGIN_MARKER, END_MARKER),
        )
    }
}
