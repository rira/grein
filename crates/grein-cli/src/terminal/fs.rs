use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::terminal::port::{ConfigFsPort, OsFamily, TerminalSetupError};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Default)]
pub struct RealConfigFs;

impl RealConfigFs {
    pub fn new() -> Self {
        Self
    }
}

fn non_empty_env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn map_io_err(path: &Path, err: io::Error) -> TerminalSetupError {
    TerminalSetupError::Io {
        path: path.to_path_buf(),
        message: err.to_string(),
    }
}

impl ConfigFsPort for RealConfigFs {
    fn home_dir(&self) -> Option<PathBuf> {
        non_empty_env_path("HOME")
    }

    fn xdg_config_home(&self) -> Option<PathBuf> {
        non_empty_env_path("XDG_CONFIG_HOME")
    }

    fn os_family(&self) -> OsFamily {
        if cfg!(target_os = "macos") {
            OsFamily::MacOs
        } else if cfg!(target_os = "linux") {
            OsFamily::Linux
        } else {
            OsFamily::Other
        }
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn read_to_string(&self, path: &Path) -> Result<Option<String>, TerminalSetupError> {
        match std::fs::read_to_string(path) {
            Ok(content) => Ok(Some(content)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(map_io_err(path, err)),
        }
    }

    fn write_atomic(&self, path: &Path, content: &str) -> Result<(), TerminalSetupError> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent).map_err(|err| map_io_err(parent, err))?;

        let seq = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("config");
        let temp_path = parent.join(format!(".{file_name}.grein-tmp-{}-{seq}", std::process::id()));

        if let Err(err) = std::fs::write(&temp_path, content) {
            let _ = std::fs::remove_file(&temp_path);
            return Err(map_io_err(&temp_path, err));
        }

        if let Err(err) = std::fs::rename(&temp_path, path) {
            let _ = std::fs::remove_file(&temp_path);
            return Err(map_io_err(path, err));
        }
        Ok(())
    }

    fn copy_file(&self, from: &Path, to: &Path) -> Result<(), TerminalSetupError> {
        if let Some(parent) = to.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|err| map_io_err(parent, err))?;
        }
        std::fs::copy(from, to)
            .map(|_| ())
            .map_err(|err| map_io_err(to, err))
    }

    fn remove_file(&self, path: &Path) -> Result<(), TerminalSetupError> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(map_io_err(path, err)),
        }
    }

    fn is_process_running(&self, name: &str) -> bool {
        #[cfg(unix)]
        {
            Command::new("pgrep")
                .args(["-x", name])
                .output()
                .is_ok_and(|out| out.status.success())
        }
        #[cfg(not(unix))]
        {
            let _ = name;
            false
        }
    }

    fn run_command(&self, program: &str, args: &[&str]) -> Result<String, TerminalSetupError> {
        let output = Command::new(program).args(args).output().map_err(|err| {
            TerminalSetupError::CommandFailed {
                program: program.to_string(),
                message: err.to_string(),
            }
        })?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let message = if stderr.is_empty() {
                format!("exited with status {}", output.status)
            } else {
                stderr
            };
            Err(TerminalSetupError::CommandFailed {
                program: program.to_string(),
                message,
            })
        }
    }
}
