use std::path::{Path, PathBuf};

use grein_tui::terminal::{TerminalEnv, TerminalKind};

use crate::terminal::port::{
    ChangeAction, ConfigFsPort, InstallStatus, PlannedChange, RevertReceipt, SetupPlan,
    SetupReceipt, TerminalSetupError, TerminalSetupProvider, TerminalStatusEntry,
};
use crate::terminal::providers::{
    ensure_backup_if_needed, has_marked_block, home_dir, revert_main_file_and_snippet,
    strip_marked_block, xdg_config_dir,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct WezTermSetupProvider;

const BEGIN_MARKER: &str = "-- BEGIN GREIN TERMINAL CONFIG";
const END_MARKER: &str = "-- END GREIN TERMINAL CONFIG";

const GREIN_LUA_SNIPPET: &str = "\
local M = {}

function M.apply_to_config(config)
  config.enable_kitty_keyboard = true
  return config
end

return M
";

fn resolve_paths(fs: &dyn ConfigFsPort) -> Result<(PathBuf, PathBuf), TerminalSetupError> {
    let home = home_dir(fs)?;
    let xdg_wezterm = xdg_config_dir(fs)?.join("wezterm");
    let dot_wezterm = home.join(".wezterm.lua");
    let xdg_main = xdg_wezterm.join("wezterm.lua");

    let main_path = if fs.exists(&dot_wezterm) && !fs.exists(&xdg_main) {
        dot_wezterm
    } else {
        xdg_main
    };
    let snippet_path = xdg_wezterm.join("grein.lua");
    Ok((main_path, snippet_path))
}

fn loader_block(snippet_path: &Path, config_ident: &str) -> String {
    let escaped = snippet_path.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "{BEGIN_MARKER}\n\
local _grein_ok, _grein = pcall(dofile, \"{escaped}\")\n\
if _grein_ok and type(_grein) == \"table\" and type(_grein.apply_to_config) == \"function\" then\n\
  _grein.apply_to_config({config_ident})\n\
end\n\
{END_MARKER}"
    )
}

fn find_return_ident(content: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = content.lines().collect();
    for (idx, line) in lines.iter().enumerate().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("--") {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("return ") {
            let ident = rest.trim().trim_end_matches(';').trim();
            if !ident.is_empty()
                && ident
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                return Some((idx, ident.to_string()));
            }
        }
        break;
    }
    None
}

fn build_updated_wezterm_lua(
    existing: Option<&str>,
    snippet_path: &Path,
) -> (String, Option<String>) {
    let Some(prev) = existing else {
        let block = loader_block(snippet_path, "config");
        let content = format!(
            "local wezterm = require(\"wezterm\")\n\
local config = wezterm.config_builder and wezterm.config_builder() or {{}}\n\n\
{block}\n\n\
return config\n"
        );
        return (content, None);
    };

    let cleaned = strip_marked_block(prev, BEGIN_MARKER, END_MARKER);
    if let Some((ret_idx, ident)) = find_return_ident(&cleaned) {
        let block = loader_block(snippet_path, &ident);
        let lines: Vec<&str> = cleaned.lines().collect();
        let mut out = Vec::with_capacity(lines.len() + 6);
        out.extend(lines[..ret_idx].iter().copied());
        if !out.last().is_some_and(|l| l.trim().is_empty()) && !out.is_empty() {
            out.push("");
        }
        out.push(&block);
        out.push("");
        out.extend(lines[ret_idx..].iter().copied());
        return (format!("{}\n", out.join("\n")), None);
    }

    let block = loader_block(snippet_path, "config");
    let mut out = cleaned.trim_end_matches('\n').to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(&block);
    out.push('\n');
    let warning = Some(
        "Could not detect a trailing `return <config>` identifier in wezterm.lua; ensure `_grein.apply_to_config(...)` runs on your returned config table."
            .to_string(),
    );
    (out, warning)
}

impl TerminalSetupProvider for WezTermSetupProvider {
    fn kind(&self) -> TerminalKind {
        TerminalKind::WezTerm
    }

    fn display_name(&self) -> &'static str {
        "WezTerm"
    }

    fn detect_status(
        &self,
        env: &TerminalEnv,
        fs: &dyn ConfigFsPort,
    ) -> Result<TerminalStatusEntry, TerminalSetupError> {
        let (main_path, snippet_path) = resolve_paths(fs)?;
        let snippet_ok = fs
            .read_to_string(&snippet_path)?
            .is_some_and(|c| c == GREIN_LUA_SNIPPET);
        let main_ok = fs
            .read_to_string(&main_path)?
            .is_some_and(|c| has_marked_block(&c, BEGIN_MARKER, END_MARKER));

        let install_status = if snippet_ok && main_ok {
            InstallStatus::Configured
        } else if snippet_ok || main_ok || fs.exists(&snippet_path) {
            InstallStatus::Partial
        } else {
            InstallStatus::NotConfigured
        };

        let active_detected = env.wezterm_executable.is_some()
            || env
                .term_program
                .as_deref()
                .is_some_and(|tp| tp.eq_ignore_ascii_case("WezTerm"));

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
        let mut warnings = Vec::new();

        let existing_snippet = fs.read_to_string(&snippet_path)?;
        if existing_snippet.as_deref() != Some(GREIN_LUA_SNIPPET) {
            changes.push(PlannedChange {
                path: snippet_path.clone(),
                action: if existing_snippet.is_some() {
                    ChangeAction::Update
                } else {
                    ChangeAction::Create
                },
                summary: "Write WezTerm Lua snippet with enable_kitty_keyboard = true".to_string(),
                snippet: GREIN_LUA_SNIPPET.to_string(),
            });
        }

        let existing_main = fs.read_to_string(&main_path)?;
        let main_ok = existing_main
            .as_deref()
            .is_some_and(|c| has_marked_block(c, BEGIN_MARKER, END_MARKER));
        if !main_ok {
            let (updated, warn) = build_updated_wezterm_lua(existing_main.as_deref(), &snippet_path);
            if let Some(w) = warn {
                warnings.push(w);
            }
            changes.push(PlannedChange {
                path: main_path,
                action: if existing_main.is_some() {
                    ChangeAction::Update
                } else {
                    ChangeAction::Create
                },
                summary: "Insert marked Grein loader block into wezterm.lua".to_string(),
                snippet: updated,
            });
        }

        Ok(SetupPlan {
            terminal: self.kind(),
            dry_run: true,
            status_before: status.install_status,
            changes,
            warnings,
            manual_steps: vec!["Reload WezTerm config (Ctrl+Shift+R or automatic reload).".to_string()],
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
        let mut warnings = Vec::new();

        let existing_snippet = fs.read_to_string(&snippet_path)?;
        if existing_snippet.as_deref() != Some(GREIN_LUA_SNIPPET) {
            ensure_backup_if_needed(fs, &snippet_path, &mut backup_paths)?;
            fs.write_atomic(&snippet_path, GREIN_LUA_SNIPPET)?;
            changed_paths.push(snippet_path.clone());
        }

        let existing_main = fs.read_to_string(&main_path)?;
        let main_ok = existing_main
            .as_deref()
            .is_some_and(|c| has_marked_block(c, BEGIN_MARKER, END_MARKER));
        if !main_ok {
            ensure_backup_if_needed(fs, &main_path, &mut backup_paths)?;
            let (updated, warn) = build_updated_wezterm_lua(existing_main.as_deref(), &snippet_path);
            if let Some(w) = warn {
                warnings.push(w);
            }
            fs.write_atomic(&main_path, &updated)?;
            changed_paths.push(main_path);
        }

        Ok(SetupReceipt {
            terminal: self.kind(),
            applied: !changed_paths.is_empty(),
            status_after: self.detect_status(env, fs)?.install_status,
            changed_paths,
            backup_paths,
            warnings,
            manual_steps: vec!["Reload WezTerm configuration if hot-reload is disabled.".to_string()],
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
