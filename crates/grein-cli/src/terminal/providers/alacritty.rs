use std::path::{Path, PathBuf};

use grein_tui::terminal::{TerminalEnv, TerminalKind};

use crate::terminal::port::{
    ChangeAction, ConfigFsPort, InstallStatus, PlannedChange, RevertReceipt, SetupPlan,
    SetupReceipt, TerminalSetupError, TerminalSetupProvider, TerminalStatusEntry,
};
use crate::terminal::providers::{
    ensure_backup_if_needed, revert_main_file_and_snippet, xdg_config_dir,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct AlacrittySetupProvider;

const ALACRITTY_SNIPPET: &str = "\
[window]
option_as_alt = \"Both\"
";

fn resolve_paths(fs: &dyn ConfigFsPort) -> Result<(PathBuf, PathBuf), TerminalSetupError> {
    let dir = xdg_config_dir(fs)?.join("alacritty");
    Ok((dir.join("alacritty.toml"), dir.join("grein.toml")))
}

fn toml_escaped_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn has_import_entry(content: &str, snippet_path: &Path) -> bool {
    let escaped = toml_escaped_path(snippet_path);
    let quoted_full = format!("\"{escaped}\"");
    content.contains(&quoted_full) || content.contains("\"grein.toml\"")
}

fn add_import_to_alacritty_toml(existing: Option<&str>, snippet_path: &Path) -> String {
    let escaped = toml_escaped_path(snippet_path);
    let entry = format!("\"{escaped}\"");

    let Some(prev) = existing else {
        return format!("[general]\nimport = [{entry}]\n");
    };

    if has_import_entry(prev, snippet_path) {
        return prev.to_string();
    }

    let lines: Vec<String> = prev.lines().map(ToString::to_string).collect();
    let mut general_idx: Option<usize> = None;
    let mut section_end = lines.len();

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed == "[general]" {
            general_idx = Some(idx);
            continue;
        }
        if general_idx.is_some() && trimmed.starts_with('[') && !trimmed.starts_with("[[") {
            section_end = idx;
            break;
        }
        if general_idx.is_some() && trimmed.starts_with("[[") {
            section_end = idx;
            break;
        }
    }

    let Some(gen_idx) = general_idx else {
        let mut out = prev.trim_end_matches('\n').to_string();
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&format!("[general]\nimport = [{entry}]\n"));
        return out;
    };

    let mut out = lines.clone();
    for idx in (gen_idx + 1)..section_end {
        let trimmed = out[idx].trim();
        if trimmed.starts_with("import") && trimmed.contains('=') {
            if let Some(bracket_end) = out[idx].rfind(']') {
                let before = out[idx][..bracket_end].trim_end();
                let sep = if before.ends_with('[') || before.ends_with(',') {
                    ""
                } else {
                    ", "
                };
                let replacement = format!("{before}{sep}{entry}{}", &out[idx][bracket_end..]);
                out[idx] = replacement;
                return format!("{}\n", out.join("\n"));
            }
            // Multi-line import array: find closing `]` within section
            for close_idx in (idx + 1)..section_end {
                if let Some(pos) = out[close_idx].find(']') {
                    let indent = "  ";
                    out.insert(close_idx, format!("{indent}{entry},"));
                    let _ = pos;
                    return format!("{}\n", out.join("\n"));
                }
            }
        }
    }

    out.insert(gen_idx + 1, format!("import = [{entry}]"));
    format!("{}\n", out.join("\n"))
}

fn remove_import_from_alacritty_toml(content: &str, snippet_path: &Path) -> String {
    let escaped = toml_escaped_path(snippet_path);
    let quoted_full = format!("\"{escaped}\"");
    let mut out = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == format!("import = [{quoted_full}]")
            || trimmed == "import = [\"grein.toml\"]"
        {
            continue;
        }
        if trimmed == format!("{quoted_full},") || trimmed == quoted_full {
            continue;
        }
        let cleaned = line
            .replace(&format!(", {quoted_full}"), "")
            .replace(&format!("{quoted_full}, "), "")
            .replace(&quoted_full, "");
        out.push(cleaned);
    }
    if out.is_empty() {
        String::new()
    } else {
        format!("{}\n", out.join("\n"))
    }
}

impl TerminalSetupProvider for AlacrittySetupProvider {
    fn kind(&self) -> TerminalKind {
        TerminalKind::Alacritty
    }

    fn display_name(&self) -> &'static str {
        "Alacritty"
    }

    fn detect_status(
        &self,
        env: &TerminalEnv,
        fs: &dyn ConfigFsPort,
    ) -> Result<TerminalStatusEntry, TerminalSetupError> {
        let (main_path, snippet_path) = resolve_paths(fs)?;
        let snippet_ok = fs
            .read_to_string(&snippet_path)?
            .is_some_and(|c| c == ALACRITTY_SNIPPET);
        let main_ok = fs
            .read_to_string(&main_path)?
            .is_some_and(|c| has_import_entry(&c, &snippet_path));

        let install_status = if snippet_ok && main_ok {
            InstallStatus::Configured
        } else if snippet_ok || main_ok || fs.exists(&snippet_path) {
            InstallStatus::Partial
        } else {
            InstallStatus::NotConfigured
        };

        let active_detected = env
            .term_program
            .as_deref()
            .is_some_and(|tp| tp.eq_ignore_ascii_case("Alacritty"))
            || env
                .term
                .as_deref()
                .is_some_and(|t| t.to_ascii_lowercase().contains("alacritty"));

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
        if existing_snippet.as_deref() != Some(ALACRITTY_SNIPPET) {
            changes.push(PlannedChange {
                path: snippet_path.clone(),
                action: if existing_snippet.is_some() {
                    ChangeAction::Update
                } else {
                    ChangeAction::Create
                },
                summary: "Write Alacritty TOML snippet with option_as_alt = \"Both\"".to_string(),
                snippet: ALACRITTY_SNIPPET.to_string(),
            });
        }

        let existing_main = fs.read_to_string(&main_path)?;
        if !existing_main
            .as_deref()
            .is_some_and(|c| has_import_entry(c, &snippet_path))
        {
            let updated = add_import_to_alacritty_toml(existing_main.as_deref(), &snippet_path);
            changes.push(PlannedChange {
                path: main_path,
                action: if existing_main.is_some() {
                    ChangeAction::Update
                } else {
                    ChangeAction::Create
                },
                summary: "Add grein.toml import under [general] in alacritty.toml".to_string(),
                snippet: updated,
            });
        }

        Ok(SetupPlan {
            terminal: self.kind(),
            dry_run: true,
            status_before: status.install_status,
            changes,
            warnings: Vec::new(),
            manual_steps: vec!["Alacritty reloads configuration automatically when live_config_reload is enabled.".to_string()],
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
        if existing_snippet.as_deref() != Some(ALACRITTY_SNIPPET) {
            ensure_backup_if_needed(fs, &snippet_path, &mut backup_paths)?;
            fs.write_atomic(&snippet_path, ALACRITTY_SNIPPET)?;
            changed_paths.push(snippet_path.clone());
        }

        let existing_main = fs.read_to_string(&main_path)?;
        if !existing_main
            .as_deref()
            .is_some_and(|c| has_import_entry(c, &snippet_path))
        {
            ensure_backup_if_needed(fs, &main_path, &mut backup_paths)?;
            let updated = add_import_to_alacritty_toml(existing_main.as_deref(), &snippet_path);
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
            manual_steps: vec!["Restart or reload Alacritty if live_config_reload is disabled.".to_string()],
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
            |content| remove_import_from_alacritty_toml(content, &snippet_path),
        )
    }
}
