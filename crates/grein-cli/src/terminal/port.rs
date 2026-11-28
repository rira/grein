use std::fmt;
use std::path::{Path, PathBuf};

use grein_tui::terminal::{TerminalEnv, TerminalKind};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallStatus {
    Configured,
    NotConfigured,
    Partial,
    NotApplicable,
    Unsupported,
}

impl InstallStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::NotConfigured => "not-configured",
            Self::Partial => "partial",
            Self::NotApplicable => "not-applicable",
            Self::Unsupported => "unsupported",
        }
    }
}

impl fmt::Display for InstallStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OsFamily {
    MacOs,
    Linux,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeAction {
    Create,
    Update,
    Command,
    Noop,
}

impl fmt::Display for ChangeAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Command => "command",
            Self::Noop => "noop",
        };
        f.write_str(label)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedChange {
    pub path: PathBuf,
    pub action: ChangeAction,
    pub summary: String,
    pub snippet: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupPlan {
    pub terminal: TerminalKind,
    pub dry_run: bool,
    pub status_before: InstallStatus,
    pub changes: Vec<PlannedChange>,
    pub warnings: Vec<String>,
    pub manual_steps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupReceipt {
    pub terminal: TerminalKind,
    pub applied: bool,
    pub status_after: InstallStatus,
    pub changed_paths: Vec<PathBuf>,
    pub backup_paths: Vec<PathBuf>,
    pub warnings: Vec<String>,
    pub manual_steps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevertReceipt {
    pub terminal: TerminalKind,
    pub reverted: bool,
    pub restored_paths: Vec<PathBuf>,
    pub removed_paths: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStatusEntry {
    pub kind: TerminalKind,
    pub display_name: String,
    pub active_detected: bool,
    pub install_status: InstallStatus,
    pub config_paths: Vec<PathBuf>,
    pub notes: Vec<String>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TerminalSetupError {
    #[error("unknown terminal `{0}`")]
    UnknownTerminal(String),
    #[error("terminal `{0}` does not support automated setup")]
    UnsupportedTerminal(TerminalKind),
    #[error("could not determine user home or XDG config directory")]
    MissingHomeDir,
    #[error("process `{process}` is running: {guidance}")]
    ProcessRunning { process: String, guidance: String },
    #[error("I/O error for {}: {message}", path.display())]
    Io { path: PathBuf, message: String },
    #[error("command `{program}` failed: {message}")]
    CommandFailed { program: String, message: String },
    #[error("invalid config in {}: {message}", path.display())]
    InvalidConfig { path: PathBuf, message: String },
}

pub trait ConfigFsPort: Send + Sync {
    fn home_dir(&self) -> Option<PathBuf>;
    fn xdg_config_home(&self) -> Option<PathBuf>;
    fn os_family(&self) -> OsFamily;
    fn exists(&self, path: &Path) -> bool;
    fn read_to_string(&self, path: &Path) -> Result<Option<String>, TerminalSetupError>;
    fn write_atomic(&self, path: &Path, content: &str) -> Result<(), TerminalSetupError>;
    fn copy_file(&self, from: &Path, to: &Path) -> Result<(), TerminalSetupError>;
    fn remove_file(&self, path: &Path) -> Result<(), TerminalSetupError>;
    fn is_process_running(&self, name: &str) -> bool;
    fn run_command(&self, program: &str, args: &[&str]) -> Result<String, TerminalSetupError>;
}

pub trait TerminalSetupProvider: Send + Sync {
    fn kind(&self) -> TerminalKind;
    fn display_name(&self) -> &'static str;
    fn detect_status(
        &self,
        env: &TerminalEnv,
        fs: &dyn ConfigFsPort,
    ) -> Result<TerminalStatusEntry, TerminalSetupError>;
    fn plan_setup(
        &self,
        env: &TerminalEnv,
        fs: &dyn ConfigFsPort,
    ) -> Result<SetupPlan, TerminalSetupError>;
    fn apply_setup(
        &self,
        env: &TerminalEnv,
        fs: &dyn ConfigFsPort,
    ) -> Result<SetupReceipt, TerminalSetupError>;
    fn revert_setup(
        &self,
        env: &TerminalEnv,
        fs: &dyn ConfigFsPort,
    ) -> Result<RevertReceipt, TerminalSetupError>;
}
