use std::io;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use grein_core::storage::sqlite::SqliteStorage;
use grein_core::{default_prefs_db_path, UserPrefsStore};
use serde::{Deserialize, Serialize};

use crate::args::DoctorArgs;
use crate::daemon::resolve_state_dir;
use crate::discovery::{resolve_agents_configuration, AcpAgent, AcpAgentKind, DiscoveryError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Ok,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorCheck {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub checks: Vec<DoctorCheck>,
}

pub type DbProbe = Arc<dyn Fn(&Path) -> Result<(), String> + Send + Sync>;
pub type AgentDiscoverer = Arc<dyn Fn() -> Result<Vec<AcpAgent>, DiscoveryError> + Send + Sync>;

pub struct DoctorDeps {
    pub cwd: PathBuf,
    pub prefs_db_path: Option<PathBuf>,
    pub session_db_probe: DbProbe,
    pub prefs_db_probe: DbProbe,
    pub agent_discoverer: AgentDiscoverer,
}

impl DoctorDeps {
    pub fn from_env() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            prefs_db_path: default_prefs_db_path(),
            session_db_probe: Arc::new(|path| {
                SqliteStorage::open(path)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            }),
            prefs_db_probe: Arc::new(|path| {
                UserPrefsStore::open(path)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            }),
            agent_discoverer: Arc::new(|| resolve_agents_configuration(None::<&Path>, None)),
        }
    }
}

fn check_git() -> DoctorCheck {
    match Command::new("git").arg("--version").output() {
        Ok(out) if out.status.success() => {
            let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
            DoctorCheck {
                name: "git".to_string(),
                status: CheckStatus::Ok,
                detail: if version.is_empty() {
                    "git available".to_string()
                } else {
                    version
                },
            }
        }
        Ok(out) => DoctorCheck {
            name: "git".to_string(),
            status: CheckStatus::Error,
            detail: format!("git exited with status {}", out.status),
        },
        Err(err) => DoctorCheck {
            name: "git".to_string(),
            status: CheckStatus::Error,
            detail: format!("git not found or failed to execute: {err}"),
        },
    }
}

fn check_shell() -> DoctorCheck {
    match std::env::var("SHELL") {
        Ok(shell) if !shell.trim().is_empty() => {
            let trimmed = shell.trim();
            if Path::new(trimmed).exists() {
                DoctorCheck {
                    name: "shell".to_string(),
                    status: CheckStatus::Ok,
                    detail: trimmed.to_string(),
                }
            } else {
                DoctorCheck {
                    name: "shell".to_string(),
                    status: CheckStatus::Warn,
                    detail: format!("{trimmed} (path does not exist)"),
                }
            }
        }
        _ => DoctorCheck {
            name: "shell".to_string(),
            status: CheckStatus::Warn,
            detail: "$SHELL is unset; falling back to /bin/sh".to_string(),
        },
    }
}

fn check_acp_agents(agent_discoverer: &AgentDiscoverer) -> DoctorCheck {
    if let Ok(bin) = std::env::var("GREIN_ACP_BIN") {
        let trimmed = bin.trim();
        if !trimmed.is_empty() {
            let path = Path::new(trimmed);
            let status = if path.is_absolute() && !path.exists() {
                CheckStatus::Warn
            } else {
                CheckStatus::Ok
            };
            return DoctorCheck {
                name: "acp_agents".to_string(),
                status,
                detail: format!("GREIN_ACP_BIN={trimmed}"),
            };
        }
    }

    match (agent_discoverer)() {
        Ok(agents) if !agents.is_empty() => {
            let summary = agents
                .into_iter()
                .map(|agent| {
                    let kind = match agent.kind {
                        AcpAgentKind::Agy => "agy",
                        AcpAgentKind::Claude => "claude",
                        AcpAgentKind::Custom => "custom",
                    };
                    format!("{kind} ({})", agent.executable_path.display())
                })
                .collect::<Vec<_>>()
                .join(", ");
            DoctorCheck {
                name: "acp_agents".to_string(),
                status: CheckStatus::Ok,
                detail: summary,
            }
        }
        Ok(_) => DoctorCheck {
            name: "acp_agents".to_string(),
            status: CheckStatus::Warn,
            detail: "no ACP agents found on PATH (agy/claude); terminal mode works, set GREIN_ACP_BIN for custom agent".to_string(),
        },
        Err(err) => DoctorCheck {
            name: "acp_agents".to_string(),
            status: CheckStatus::Warn,
            detail: format!("failed to inspect ACP configuration: {err:?}"),
        },
    }
}

fn check_terminal_env() -> DoctorCheck {
    let term = std::env::var("TERM")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let colorterm = std::env::var("COLORTERM")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .into_iter()
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        });

    let term_ok = term
        .as_deref()
        .is_some_and(|t| !t.eq_ignore_ascii_case("dumb"));
    let utf8_ok = locale.as_deref().is_some_and(|loc| {
        let lower = loc.to_ascii_lowercase();
        lower.contains("utf-8") || lower.contains("utf8")
    });

    let status = if term_ok && utf8_ok {
        CheckStatus::Ok
    } else {
        CheckStatus::Warn
    };

    DoctorCheck {
        name: "terminal_env".to_string(),
        status,
        detail: format!(
            "TERM={}, COLORTERM={}, locale={}",
            term.as_deref().unwrap_or("<unset>"),
            colorterm.as_deref().unwrap_or("<unset>"),
            locale.as_deref().unwrap_or("<unset>"),
        ),
    }
}

fn check_workspace_state(cwd: &Path, probe: &DbProbe) -> DoctorCheck {
    let state_dir = resolve_state_dir(cwd);
    let db_path = state_dir.join("session.db");
    let socket_path = state_dir.join("daemon.sock");
    let socket_reachable = socket_path.exists() && UnixStream::connect(&socket_path).is_ok();

    if db_path.exists() {
        match (probe)(&db_path) {
            Ok(()) => DoctorCheck {
                name: "workspace_state".to_string(),
                status: CheckStatus::Ok,
                detail: format!(
                    "state_dir={}, db={} (ok), socket={} (reachable={socket_reachable})",
                    state_dir.display(),
                    db_path.display(),
                    socket_path.display(),
                ),
            },
            Err(err) => DoctorCheck {
                name: "workspace_state".to_string(),
                status: CheckStatus::Error,
                detail: format!("failed to open {}: {err}", db_path.display()),
            },
        }
    } else {
        DoctorCheck {
            name: "workspace_state".to_string(),
            status: CheckStatus::Ok,
            detail: format!(
                "state_dir={}, db={} (not yet created), socket={} (reachable={socket_reachable})",
                state_dir.display(),
                db_path.display(),
                socket_path.display(),
            ),
        }
    }
}

fn check_preferences_db(prefs_path: Option<&Path>, probe: &DbProbe) -> DoctorCheck {
    let Some(path) = prefs_path else {
        return DoctorCheck {
            name: "preferences_db".to_string(),
            status: CheckStatus::Warn,
            detail: "no HOME, XDG_CONFIG_HOME, or GREIN_CONFIG_DB set; using defaults".to_string(),
        };
    };

    if path.exists() {
        match (probe)(path) {
            Ok(()) => DoctorCheck {
                name: "preferences_db".to_string(),
                status: CheckStatus::Ok,
                detail: format!("{} (ok)", path.display()),
            },
            Err(err) => DoctorCheck {
                name: "preferences_db".to_string(),
                status: CheckStatus::Error,
                detail: format!("failed to open {}: {err}", path.display()),
            },
        }
    } else {
        DoctorCheck {
            name: "preferences_db".to_string(),
            status: CheckStatus::Ok,
            detail: format!("{} (not yet created)", path.display()),
        }
    }
}

pub fn collect_doctor_report_with(deps: &DoctorDeps) -> DoctorReport {
    let checks = vec![
        check_git(),
        check_shell(),
        check_acp_agents(&deps.agent_discoverer),
        check_terminal_env(),
        check_workspace_state(&deps.cwd, &deps.session_db_probe),
        check_preferences_db(deps.prefs_db_path.as_deref(), &deps.prefs_db_probe),
    ];
    let ok = !checks.iter().any(|c| c.status == CheckStatus::Error);
    DoctorReport { ok, checks }
}

pub fn collect_doctor_report() -> DoctorReport {
    collect_doctor_report_with(&DoctorDeps::from_env())
}

pub fn handle_doctor_with(
    args: &DoctorArgs,
    deps: &DoctorDeps,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let report = collect_doctor_report_with(deps);
    if args.json {
        let encoded = serde_json::to_string_pretty(&report)?;
        println!("{encoded}");
    } else {
        for check in &report.checks {
            let tag = match check.status {
                CheckStatus::Ok => "OK",
                CheckStatus::Warn => "WARN",
                CheckStatus::Error => "ERR",
            };
            println!("[{tag:<4}] {:<16} {}", check.name, check.detail);
        }
    }

    if report.ok {
        Ok(())
    } else {
        Err(Box::new(io::Error::other(
            "doctor reported one or more critical errors",
        )))
    }
}

pub fn handle_doctor(args: &DoctorArgs) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    handle_doctor_with(args, &DoctorDeps::from_env())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_doctor_report_includes_expected_checks() {
        let report = collect_doctor_report();
        let names: Vec<&str> = report.checks.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "git",
                "shell",
                "acp_agents",
                "terminal_env",
                "workspace_state",
                "preferences_db",
            ]
        );
        assert_eq!(report.checks[0].status, CheckStatus::Ok);
    }

    #[test]
    fn test_check_preferences_db_detects_missing_and_corrupt_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let deps = DoctorDeps::from_env();
        let missing = temp.path().join("nonexistent.db");
        let missing_check = check_preferences_db(Some(&missing), &deps.prefs_db_probe);
        assert_eq!(missing_check.status, CheckStatus::Ok);

        let dir_as_db = temp.path().join("dir.db");
        std::fs::create_dir_all(&dir_as_db).expect("mkdir");
        let corrupt_check = check_preferences_db(Some(&dir_as_db), &deps.prefs_db_probe);
        assert_eq!(corrupt_check.status, CheckStatus::Error);

        let unset_check = check_preferences_db(None, &deps.prefs_db_probe);
        assert_eq!(unset_check.status, CheckStatus::Warn);
    }
}
