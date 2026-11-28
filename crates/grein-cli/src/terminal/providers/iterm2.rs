use std::path::PathBuf;

use grein_tui::terminal::{TerminalEnv, TerminalKind};

use crate::terminal::port::{
    ChangeAction, ConfigFsPort, InstallStatus, OsFamily, PlannedChange, RevertReceipt, SetupPlan,
    SetupReceipt, TerminalSetupError, TerminalSetupProvider, TerminalStatusEntry,
};
use crate::terminal::providers::{backup_path_for, ensure_backup_if_needed, home_dir};

#[derive(Debug, Clone, Copy, Default)]
pub struct ITerm2SetupProvider;

const ITERM2_DYNAMIC_PROFILE: &str = r#"{
  "Profiles": [
    {
      "Name": "Grein",
      "Guid": "grein-iterm2-dynamic-profile-v1",
      "Dynamic Profile Parent Name": "Default",
      "Option Key Sends": 2,
      "Right Option Key Sends": 2,
      "Keyboard Map": {
        "0xf700-0x220000": {
          "Action": 10,
          "Text": "[1;2A"
        },
        "0xf701-0x220000": {
          "Action": 10,
          "Text": "[1;2B"
        },
        "0xf702-0x220000": {
          "Action": 10,
          "Text": "[1;2D"
        },
        "0xf703-0x220000": {
          "Action": 10,
          "Text": "[1;2C"
        },
        "0xd-0x20000": {
          "Action": 10,
          "Text": "[13;2u"
        }
      }
    }
  ]
}
"#;

fn profile_path(fs: &dyn ConfigFsPort) -> Result<PathBuf, TerminalSetupError> {
    Ok(home_dir(fs)?.join("Library/Application Support/iTerm2/DynamicProfiles/grein.json"))
}

fn has_expected_profile(content: &str) -> bool {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(content) else {
        return false;
    };
    let Some(profile) = json["Profiles"].as_array().and_then(|a| a.first()) else {
        return false;
    };
    profile["Dynamic Profile Parent Name"] == "Default"
        && profile["Option Key Sends"] == 2
        && profile["Keyboard Map"]["0xf700-0x220000"]["Text"] == "[1;2A"
        && profile["Keyboard Map"]["0xf701-0x220000"]["Text"] == "[1;2B"
        && profile["Keyboard Map"]["0xf702-0x220000"]["Text"] == "[1;2D"
        && profile["Keyboard Map"]["0xf703-0x220000"]["Text"] == "[1;2C"
        && profile["Keyboard Map"]["0xd-0x20000"]["Text"] == "[13;2u"
}

impl TerminalSetupProvider for ITerm2SetupProvider {
    fn kind(&self) -> TerminalKind {
        TerminalKind::ITerm2
    }

    fn display_name(&self) -> &'static str {
        "iTerm2"
    }

    fn detect_status(
        &self,
        env: &TerminalEnv,
        fs: &dyn ConfigFsPort,
    ) -> Result<TerminalStatusEntry, TerminalSetupError> {
        let active_detected = env
            .term_program
            .as_deref()
            .is_some_and(|tp| tp.eq_ignore_ascii_case("iTerm.app") || tp.eq_ignore_ascii_case("iTerm2"));

        if fs.os_family() != OsFamily::MacOs {
            return Ok(TerminalStatusEntry {
                kind: self.kind(),
                display_name: self.display_name().to_string(),
                active_detected,
                install_status: InstallStatus::NotApplicable,
                config_paths: Vec::new(),
                notes: vec!["iTerm2 is only available on macOS".to_string()],
            });
        }

        let path = profile_path(fs)?;
        let install_status = match fs.read_to_string(&path)? {
            Some(content) if has_expected_profile(&content) => InstallStatus::Configured,
            Some(_) => InstallStatus::Partial,
            None => InstallStatus::NotConfigured,
        };

        Ok(TerminalStatusEntry {
            kind: self.kind(),
            display_name: self.display_name().to_string(),
            active_detected,
            install_status,
            config_paths: vec![path],
            notes: Vec::new(),
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
                warnings: vec!["iTerm2 setup is only applicable on macOS".to_string()],
                manual_steps: Vec::new(),
            });
        }

        let status = self.detect_status(env, fs)?;
        let path = profile_path(fs)?;
        let mut changes = Vec::new();
        if status.install_status != InstallStatus::Configured {
            changes.push(PlannedChange {
                path,
                action: if status.install_status == InstallStatus::NotConfigured {
                    ChangeAction::Create
                } else {
                    ChangeAction::Update
                },
                summary: "Write iTerm2 Dynamic Profile with Option=Esc+, Shift+Arrows, and Shift+Return".to_string(),
                snippet: ITERM2_DYNAMIC_PROFILE.to_string(),
            });
        }

        Ok(SetupPlan {
            terminal: self.kind(),
            dry_run: true,
            status_before: status.install_status,
            changes,
            warnings: Vec::new(),
            manual_steps: vec![
                "Select the `Grein` profile in iTerm2 Preferences -> Profiles (or set it as default)."
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
                warnings: vec!["iTerm2 setup is only applicable on macOS".to_string()],
                manual_steps: Vec::new(),
            });
        }

        let status = self.detect_status(env, fs)?;
        if status.install_status == InstallStatus::Configured {
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

        let path = profile_path(fs)?;
        let mut backup_paths = Vec::new();
        ensure_backup_if_needed(fs, &path, &mut backup_paths)?;
        fs.write_atomic(&path, ITERM2_DYNAMIC_PROFILE)?;

        Ok(SetupReceipt {
            terminal: self.kind(),
            applied: true,
            status_after: self.detect_status(env, fs)?.install_status,
            changed_paths: vec![path],
            backup_paths,
            warnings: Vec::new(),
            manual_steps: vec![
                "Select the `Grein` profile in iTerm2 Preferences -> Profiles (hot-reloaded automatically)."
                    .to_string(),
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
                warnings: vec!["iTerm2 setup is only applicable on macOS".to_string()],
            });
        }

        let path = profile_path(fs)?;
        let backup = backup_path_for(&path);
        let mut restored_paths = Vec::new();
        let mut removed_paths = Vec::new();

        if fs.exists(&backup) {
            fs.copy_file(&backup, &path)?;
            fs.remove_file(&backup)?;
            restored_paths.push(path);
            removed_paths.push(backup);
        } else if fs.exists(&path) {
            fs.remove_file(&path)?;
            removed_paths.push(path);
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
