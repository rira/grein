pub mod alacritty;
pub mod apple_terminal;
pub mod ghostty;
pub mod iterm2;
pub mod kitty;
pub mod tmux;
pub mod wezterm;
pub mod zellij;

use std::path::{Path, PathBuf};

use grein_tui::terminal::TerminalKind;

use crate::terminal::port::{
    ChangeAction, ConfigFsPort, InstallStatus, PlannedChange, RevertReceipt, SetupPlan,
    SetupReceipt, TerminalSetupError, TerminalSetupProvider,
};

pub use alacritty::AlacrittySetupProvider;
pub use apple_terminal::AppleTerminalSetupProvider;
pub use ghostty::GhosttySetupProvider;
pub use iterm2::ITerm2SetupProvider;
pub use kitty::KittySetupProvider;
pub use tmux::TmuxSetupProvider;
pub use wezterm::WezTermSetupProvider;
pub use zellij::ZellijSetupProvider;

pub fn default_providers() -> Vec<Box<dyn TerminalSetupProvider>> {
    vec![
        Box::new(AppleTerminalSetupProvider),
        Box::new(ITerm2SetupProvider),
        Box::new(GhosttySetupProvider),
        Box::new(KittySetupProvider),
        Box::new(WezTermSetupProvider),
        Box::new(AlacrittySetupProvider),
        Box::new(TmuxSetupProvider),
        Box::new(ZellijSetupProvider),
    ]
}

pub(crate) fn home_dir(fs: &dyn ConfigFsPort) -> Result<PathBuf, TerminalSetupError> {
    fs.home_dir().ok_or(TerminalSetupError::MissingHomeDir)
}

pub(crate) fn xdg_config_dir(fs: &dyn ConfigFsPort) -> Result<PathBuf, TerminalSetupError> {
    fs.xdg_config_home()
        .or_else(|| fs.home_dir().map(|h| h.join(".config")))
        .ok_or(TerminalSetupError::MissingHomeDir)
}

pub(crate) fn backup_path_for(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(".grein.bak");
    PathBuf::from(os)
}

pub(crate) fn ensure_backup_if_needed(
    fs: &dyn ConfigFsPort,
    path: &Path,
    backup_paths: &mut Vec<PathBuf>,
) -> Result<(), TerminalSetupError> {
    if fs.exists(path) {
        let backup = backup_path_for(path);
        if !fs.exists(&backup) {
            fs.copy_file(path, &backup)?;
            backup_paths.push(backup);
        }
    }
    Ok(())
}

pub(crate) fn contains_line_trimmed(content: &str, needle: &str) -> bool {
    let target = needle.trim();
    content.lines().any(|line| line.trim() == target)
}

pub(crate) fn append_line_idempotent(existing: Option<&str>, line: &str) -> String {
    let trimmed_line = line.trim();
    match existing {
        None => format!("{trimmed_line}\n"),
        Some(prev) => {
            if contains_line_trimmed(prev, trimmed_line) {
                return prev.to_string();
            }
            let mut out = prev.to_string();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(trimmed_line);
            out.push('\n');
            out
        }
    }
}

pub(crate) fn remove_matching_line(content: &str, line: &str) -> String {
    let target = line.trim();
    let kept: Vec<&str> = content
        .lines()
        .filter(|l| l.trim() != target)
        .collect();
    if kept.is_empty() {
        String::new()
    } else {
        format!("{}\n", kept.join("\n"))
    }
}

pub(crate) fn has_marked_block(content: &str, begin_marker: &str, end_marker: &str) -> bool {
    let mut saw_begin = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == begin_marker {
            saw_begin = true;
        } else if saw_begin && trimmed == end_marker {
            return true;
        }
    }
    false
}

pub(crate) fn strip_marked_block(content: &str, begin_marker: &str, end_marker: &str) -> String {
    let mut out = Vec::new();
    let mut inside = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == begin_marker {
            inside = true;
            continue;
        }
        if inside && trimmed == end_marker {
            inside = false;
            continue;
        }
        if !inside {
            out.push(line);
        }
    }
    if out.is_empty() {
        String::new()
    } else {
        format!("{}\n", out.join("\n"))
    }
}

pub(crate) fn upsert_marked_block(
    existing: Option<&str>,
    begin_marker: &str,
    end_marker: &str,
    block_body: &str,
) -> String {
    let full_block = format!(
        "{begin_marker}\n{}\n{end_marker}",
        block_body.trim_end_matches('\n')
    );
    match existing {
        None => format!("{full_block}\n"),
        Some(prev) => {
            let cleaned = strip_marked_block(prev, begin_marker, end_marker);
            if cleaned.trim().is_empty() {
                format!("{full_block}\n")
            } else {
                let mut out = cleaned.trim_end_matches('\n').to_string();
                out.push_str("\n\n");
                out.push_str(&full_block);
                out.push('\n');
                out
            }
        }
    }
}

pub(crate) struct TwoFileIncludeSpec {
    pub kind: TerminalKind,
    pub main_config_path: PathBuf,
    pub snippet_path: PathBuf,
    pub snippet_content: String,
    pub include_line: String,
    pub manual_steps: Vec<String>,
}

pub(crate) fn detect_two_file_status(
    spec: &TwoFileIncludeSpec,
    fs: &dyn ConfigFsPort,
) -> Result<InstallStatus, TerminalSetupError> {
    let snippet_ok = fs
        .read_to_string(&spec.snippet_path)?
        .is_some_and(|c| c == spec.snippet_content);
    let main_ok = fs
        .read_to_string(&spec.main_config_path)?
        .is_some_and(|c| contains_line_trimmed(&c, &spec.include_line));

    if snippet_ok && main_ok {
        Ok(InstallStatus::Configured)
    } else if snippet_ok || main_ok || fs.exists(&spec.snippet_path) {
        Ok(InstallStatus::Partial)
    } else {
        Ok(InstallStatus::NotConfigured)
    }
}

pub(crate) fn plan_two_file_setup(
    spec: &TwoFileIncludeSpec,
    fs: &dyn ConfigFsPort,
) -> Result<SetupPlan, TerminalSetupError> {
    let status_before = detect_two_file_status(spec, fs)?;
    let mut changes = Vec::new();

    let existing_snippet = fs.read_to_string(&spec.snippet_path)?;
    if existing_snippet.as_deref() != Some(spec.snippet_content.as_str()) {
        changes.push(PlannedChange {
            path: spec.snippet_path.clone(),
            action: if existing_snippet.is_some() {
                ChangeAction::Update
            } else {
                ChangeAction::Create
            },
            summary: "Write Grein terminal settings snippet".to_string(),
            snippet: spec.snippet_content.clone(),
        });
    }

    let existing_main = fs.read_to_string(&spec.main_config_path)?;
    let main_has_line = existing_main
        .as_deref()
        .is_some_and(|c| contains_line_trimmed(c, &spec.include_line));
    if !main_has_line {
        changes.push(PlannedChange {
            path: spec.main_config_path.clone(),
            action: if existing_main.is_some() {
                ChangeAction::Update
            } else {
                ChangeAction::Create
            },
            summary: format!("Add `{}` directive", spec.include_line),
            snippet: spec.include_line.clone(),
        });
    }

    Ok(SetupPlan {
        terminal: spec.kind,
        dry_run: true,
        status_before,
        changes,
        warnings: Vec::new(),
        manual_steps: spec.manual_steps.clone(),
    })
}

pub(crate) fn apply_two_file_setup(
    spec: &TwoFileIncludeSpec,
    fs: &dyn ConfigFsPort,
) -> Result<SetupReceipt, TerminalSetupError> {
    let mut changed_paths = Vec::new();
    let mut backup_paths = Vec::new();

    let existing_snippet = fs.read_to_string(&spec.snippet_path)?;
    if existing_snippet.as_deref() != Some(spec.snippet_content.as_str()) {
        ensure_backup_if_needed(fs, &spec.snippet_path, &mut backup_paths)?;
        fs.write_atomic(&spec.snippet_path, &spec.snippet_content)?;
        changed_paths.push(spec.snippet_path.clone());
    }

    let existing_main = fs.read_to_string(&spec.main_config_path)?;
    let main_has_line = existing_main
        .as_deref()
        .is_some_and(|c| contains_line_trimmed(c, &spec.include_line));
    if !main_has_line {
        ensure_backup_if_needed(fs, &spec.main_config_path, &mut backup_paths)?;
        let updated = append_line_idempotent(existing_main.as_deref(), &spec.include_line);
        fs.write_atomic(&spec.main_config_path, &updated)?;
        changed_paths.push(spec.main_config_path.clone());
    }

    let status_after = detect_two_file_status(spec, fs)?;
    Ok(SetupReceipt {
        terminal: spec.kind,
        applied: !changed_paths.is_empty(),
        status_after,
        changed_paths,
        backup_paths,
        warnings: Vec::new(),
        manual_steps: spec.manual_steps.clone(),
    })
}

pub(crate) fn revert_main_file_and_snippet(
    kind: TerminalKind,
    main_config_path: &Path,
    snippet_path: &Path,
    fs: &dyn ConfigFsPort,
    strip_main: impl FnOnce(&str) -> String,
) -> Result<RevertReceipt, TerminalSetupError> {
    let mut restored_paths = Vec::new();
    let mut removed_paths = Vec::new();

    let main_backup = backup_path_for(main_config_path);
    if fs.exists(&main_backup) {
        fs.copy_file(&main_backup, main_config_path)?;
        fs.remove_file(&main_backup)?;
        restored_paths.push(main_config_path.to_path_buf());
        removed_paths.push(main_backup);
    } else if let Some(content) = fs.read_to_string(main_config_path)? {
        let stripped = strip_main(&content);
        if stripped != content {
            if stripped.trim().is_empty() {
                fs.remove_file(main_config_path)?;
                removed_paths.push(main_config_path.to_path_buf());
            } else {
                fs.write_atomic(main_config_path, &stripped)?;
                restored_paths.push(main_config_path.to_path_buf());
            }
        }
    }

    let snippet_backup = backup_path_for(snippet_path);
    if fs.exists(&snippet_backup) {
        fs.copy_file(&snippet_backup, snippet_path)?;
        fs.remove_file(&snippet_backup)?;
        restored_paths.push(snippet_path.to_path_buf());
        removed_paths.push(snippet_backup);
    } else if fs.exists(snippet_path) {
        fs.remove_file(snippet_path)?;
        removed_paths.push(snippet_path.to_path_buf());
    }

    let reverted = !restored_paths.is_empty() || !removed_paths.is_empty();
    Ok(RevertReceipt {
        terminal: kind,
        reverted,
        restored_paths,
        removed_paths,
        warnings: Vec::new(),
    })
}
