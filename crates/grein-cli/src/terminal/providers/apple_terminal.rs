use std::path::PathBuf;

use grein_tui::terminal::{TerminalEnv, TerminalKind};

use crate::terminal::port::{
    ChangeAction, ConfigFsPort, InstallStatus, OsFamily, PlannedChange, RevertReceipt, SetupPlan,
    SetupReceipt, TerminalSetupError, TerminalSetupProvider, TerminalStatusEntry,
};
use crate::terminal::providers::{
    backup_path_for, ensure_backup_if_needed, home_dir, xdg_config_dir,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct AppleTerminalSetupProvider;

const GREIN_TERMINAL_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>name</key>
	<string>Grein</string>
	<key>type</key>
	<string>Window Settings</string>
	<key>useOptionAsMetaKey</key>
	<true/>
	<key>keyMapBoundKeys</key>
	<dict>
		<key>$F700</key>
		<string>\033[1;2A</string>
		<key>$F701</key>
		<string>\033[1;2B</string>
		<key>$F702</key>
		<string>\033[1;2D</string>
		<key>$F703</key>
		<string>\033[1;2C</string>
	</dict>
</dict>
</plist>
"#;

const KEY_BINDINGS: [(&str, &str); 4] = [
    ("$F700", "\\033[1;2A"),
    ("$F701", "\\033[1;2B"),
    ("$F702", "\\033[1;2D"),
    ("$F703", "\\033[1;2C"),
];

fn snippet_path(fs: &dyn ConfigFsPort) -> Result<PathBuf, TerminalSetupError> {
    Ok(xdg_config_dir(fs)?.join("grein/apple-terminal/grein.terminal"))
}

fn plist_path(fs: &dyn ConfigFsPort) -> Result<PathBuf, TerminalSetupError> {
    Ok(home_dir(fs)?.join("Library/Preferences/com.apple.Terminal.plist"))
}

fn read_default_profile(fs: &dyn ConfigFsPort) -> String {
    fs.run_command("defaults", &["read", "com.apple.Terminal", "Default Window Settings"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Basic".to_string())
}

fn defaults_show_bindings(fs: &dyn ConfigFsPort) -> Option<bool> {
    let out = fs
        .run_command("defaults", &["read", "com.apple.Terminal", "Window Settings"])
        .ok()?;
    let trimmed = out.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(
        trimmed.contains("useOptionAsMetaKey = 1")
            && trimmed.contains("F700")
            && trimmed.contains("F701")
            && trimmed.contains("F702")
            && trimmed.contains("F703"),
    )
}

fn running_terminal_warning(fs: &dyn ConfigFsPort) -> Option<String> {
    if fs.is_process_running("Terminal") {
        Some(
            "Terminal.app is currently running; quit Terminal.app before applying so cfprefsd does not clobber plist changes."
                .to_string(),
        )
    } else {
        None
    }
}

impl TerminalSetupProvider for AppleTerminalSetupProvider {
    fn kind(&self) -> TerminalKind {
        TerminalKind::AppleTerminal
    }

    fn display_name(&self) -> &'static str {
        "Apple Terminal"
    }

    fn detect_status(
        &self,
        env: &TerminalEnv,
        fs: &dyn ConfigFsPort,
    ) -> Result<TerminalStatusEntry, TerminalSetupError> {
        let active_detected = env
            .term_program
            .as_deref()
            .is_some_and(|tp| tp.eq_ignore_ascii_case("Apple_Terminal"));

        if fs.os_family() != OsFamily::MacOs {
            return Ok(TerminalStatusEntry {
                kind: self.kind(),
                display_name: self.display_name().to_string(),
                active_detected,
                install_status: InstallStatus::NotApplicable,
                config_paths: Vec::new(),
                notes: vec!["Apple Terminal is only available on macOS".to_string()],
            });
        }

        let snippet = snippet_path(fs)?;
        let plist = plist_path(fs)?;
        let snippet_content = fs.read_to_string(&snippet)?;
        let snippet_ok = snippet_content.as_deref() == Some(GREIN_TERMINAL_XML);
        let defaults_ok = defaults_show_bindings(fs);

        let install_status = match (snippet_ok, defaults_ok) {
            (true, Some(false)) => InstallStatus::Partial,
            (true, _) => InstallStatus::Configured,
            (false, _) if snippet_content.is_some() => InstallStatus::Partial,
            (false, _) => InstallStatus::NotConfigured,
        };

        let mut notes = Vec::new();
        if let Some(warn) = running_terminal_warning(fs) {
            notes.push(warn);
        }

        Ok(TerminalStatusEntry {
            kind: self.kind(),
            display_name: self.display_name().to_string(),
            active_detected,
            install_status,
            config_paths: vec![snippet, plist],
            notes,
        })
    }

    fn plan_setup(
        &self,
        env: &TerminalEnv,
        fs: &dyn ConfigFsPort,
    ) -> Result<SetupPlan, TerminalSetupError> {
        if fs.os_family() != OsFamily::MacOs {
            return Ok(SetupPlan {
                terminal: self.kind(),
                dry_run: true,
                status_before: InstallStatus::NotApplicable,
                changes: Vec::new(),
                warnings: vec!["Apple Terminal setup is only applicable on macOS".to_string()],
                manual_steps: Vec::new(),
            });
        }

        let status = self.detect_status(env, fs)?;
        let snippet = snippet_path(fs)?;
        let plist = plist_path(fs)?;
        let mut changes = Vec::new();

        if fs.read_to_string(&snippet)?.as_deref() != Some(GREIN_TERMINAL_XML) {
            changes.push(PlannedChange {
                path: snippet,
                action: if status.install_status == InstallStatus::NotConfigured {
                    ChangeAction::Create
                } else {
                    ChangeAction::Update
                },
                summary: "Write Grein Apple Terminal XML profile snippet".to_string(),
                snippet: GREIN_TERMINAL_XML.to_string(),
            });
        }

        if status.install_status != InstallStatus::Configured {
            changes.push(PlannedChange {
                path: plist,
                action: ChangeAction::Command,
                summary: "Bind Shift+Arrows ($F700-$F703 -> \\033[1;2A..D) and enable useOptionAsMetaKey via defaults/PlistBuddy".to_string(),
                snippet: "useOptionAsMetaKey=true; $F700=\\033[1;2A; $F701=\\033[1;2B; $F702=\\033[1;2D; $F703=\\033[1;2C".to_string(),
            });
        }

        let warnings = running_terminal_warning(fs).into_iter().collect();

        Ok(SetupPlan {
            terminal: self.kind(),
            dry_run: true,
            status_before: status.install_status,
            changes,
            warnings,
            manual_steps: vec![
                "Quit and reopen Terminal.app after applying so cfprefsd reloads the updated key bindings."
                    .to_string(),
            ],
        })
    }

    fn apply_setup(
        &self,
        env: &TerminalEnv,
        fs: &dyn ConfigFsPort,
    ) -> Result<SetupReceipt, TerminalSetupError> {
        if fs.os_family() != OsFamily::MacOs {
            return Ok(SetupReceipt {
                terminal: self.kind(),
                applied: false,
                status_after: InstallStatus::NotApplicable,
                changed_paths: Vec::new(),
                backup_paths: Vec::new(),
                warnings: vec!["Apple Terminal setup is only applicable on macOS".to_string()],
                manual_steps: Vec::new(),
            });
        }

        if fs.is_process_running("Terminal") {
            return Err(TerminalSetupError::ProcessRunning {
                process: "Terminal".to_string(),
                guidance: "Quit Terminal.app before applying so cfprefsd does not clobber plist changes."
                    .to_string(),
            });
        }

        let current = self.detect_status(env, fs)?;
        if current.install_status == InstallStatus::Configured {
            return Ok(SetupReceipt {
                terminal: self.kind(),
                applied: false,
                status_after: InstallStatus::Configured,
                changed_paths: Vec::new(),
                backup_paths: Vec::new(),
                warnings: Vec::new(),
                manual_steps: Vec::new(),
            });
        }

        let snippet = snippet_path(fs)?;
        let plist = plist_path(fs)?;
        let mut changed_paths = Vec::new();
        let mut backup_paths = Vec::new();

        ensure_backup_if_needed(fs, &plist, &mut backup_paths)?;
        if fs.read_to_string(&snippet)?.as_deref() != Some(GREIN_TERMINAL_XML) {
            ensure_backup_if_needed(fs, &snippet, &mut backup_paths)?;
            fs.write_atomic(&snippet, GREIN_TERMINAL_XML)?;
            changed_paths.push(snippet);
        }

        let profile = read_default_profile(fs);
        let plist_str = plist.to_string_lossy().to_string();
        let mut warnings = Vec::new();
        let option_cmd = format!("Set ':Window Settings:{profile}:useOptionAsMetaKey' true");
        let add_option_cmd = format!("Add ':Window Settings:{profile}:useOptionAsMetaKey' bool true");
        if fs
            .run_command("/usr/libexec/PlistBuddy", &["-c", &option_cmd, &plist_str])
            .is_err()
        {
            if let Err(err) =
                fs.run_command("/usr/libexec/PlistBuddy", &["-c", &add_option_cmd, &plist_str])
            {
                warnings.push(format!(
                    "Could not update useOptionAsMetaKey automatically ({err}); import grein.terminal manually."
                ));
            }
        }

        let ensure_keymap = format!("Add ':Window Settings:{profile}:keyMapBoundKeys' dict");
        let _ = fs.run_command("/usr/libexec/PlistBuddy", &["-c", &ensure_keymap, &plist_str]);

        for (key, seq) in KEY_BINDINGS {
            let set_cmd = format!("Set ':Window Settings:{profile}:keyMapBoundKeys:{key}' '{seq}'");
            if fs
                .run_command("/usr/libexec/PlistBuddy", &["-c", &set_cmd, &plist_str])
                .is_err()
            {
                let add_cmd =
                    format!("Add ':Window Settings:{profile}:keyMapBoundKeys:{key}' string '{seq}'");
                let _ =
                    fs.run_command("/usr/libexec/PlistBuddy", &["-c", &add_cmd, &plist_str]);
            }
        }
        let _ = fs.run_command("defaults", &["read", "com.apple.Terminal"]);

        let status_after = self.detect_status(env, fs)?.install_status;
        Ok(SetupReceipt {
            terminal: self.kind(),
            applied: !changed_paths.is_empty(),
            status_after,
            changed_paths,
            backup_paths,
            warnings,
            manual_steps: vec![
                "Reopen Terminal.app to activate the updated keyboard bindings.".to_string(),
            ],
        })
    }

    fn revert_setup(
        &self,
        _env: &TerminalEnv,
        fs: &dyn ConfigFsPort,
    ) -> Result<RevertReceipt, TerminalSetupError> {
        if fs.os_family() != OsFamily::MacOs {
            return Ok(RevertReceipt {
                terminal: self.kind(),
                reverted: false,
                restored_paths: Vec::new(),
                removed_paths: Vec::new(),
                warnings: vec!["Apple Terminal setup is only applicable on macOS".to_string()],
            });
        }

        if fs.is_process_running("Terminal") {
            return Err(TerminalSetupError::ProcessRunning {
                process: "Terminal".to_string(),
                guidance: "Quit Terminal.app before reverting so cfprefsd does not clobber the restored plist."
                    .to_string(),
            });
        }

        let snippet = snippet_path(fs)?;
        let plist = plist_path(fs)?;
        let plist_backup = backup_path_for(&plist);

        let mut restored_paths = Vec::new();
        let mut removed_paths = Vec::new();

        if fs.exists(&plist_backup) {
            fs.copy_file(&plist_backup, &plist)?;
            fs.remove_file(&plist_backup)?;
            restored_paths.push(plist);
            removed_paths.push(plist_backup);
            let _ = fs.run_command("defaults", &["read", "com.apple.Terminal"]);
        }

        let snippet_backup = backup_path_for(&snippet);
        if fs.exists(&snippet_backup) {
            fs.copy_file(&snippet_backup, &snippet)?;
            fs.remove_file(&snippet_backup)?;
            restored_paths.push(snippet);
            removed_paths.push(snippet_backup);
        } else if fs.exists(&snippet) {
            fs.remove_file(&snippet)?;
            removed_paths.push(snippet);
        }

        Ok(RevertReceipt {
            terminal: self.kind(),
            reverted: !restored_paths.is_empty() || !removed_paths.is_empty(),
            restored_paths,
            removed_paths,
            warnings: Vec::new(),
        })
    }
}
