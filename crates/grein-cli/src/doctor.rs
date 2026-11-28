use std::io;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use grein_core::storage::sqlite::SqliteStorage;
use grein_core::{default_prefs_db_path, UserPrefsStore};
use grein_tui::terminal::providers::default_registry;
use grein_tui::terminal::{ColorCapability, TerminalEnv, TerminalKind};
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

fn clean_non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|s| !s.is_empty())
}

fn build_terminal_env(
    term: Option<&str>,
    term_program: Option<&str>,
    colorterm: Option<&str>,
    kitty_window_id: Option<&str>,
    ghostty_resources_dir: Option<&str>,
    locale: Option<&str>,
) -> TerminalEnv {
    let mut env = TerminalEnv::default();
    if let Some(v) = clean_non_empty(term) {
        env = env.with_term(v);
    }
    if let Some(v) = clean_non_empty(term_program) {
        env = env.with_term_program(v);
    }
    if let Some(v) = clean_non_empty(colorterm) {
        env = env.with_colorterm(v);
    }
    if let Some(v) = clean_non_empty(kitty_window_id) {
        env = env.with_kitty_window_id(v);
    }
    if let Some(v) = clean_non_empty(ghostty_resources_dir) {
        env = env.with_ghostty_resources_dir(v);
    }
    if let Some(v) = clean_non_empty(locale) {
        env = env.with_locale(v);
    }
    env
}

fn resolve_color_capability(env: &TerminalEnv) -> ColorCapability {
    default_registry()
        .resolve(env)
        .map(|adapter| adapter.capabilities(env).color)
        .unwrap_or(ColorCapability::Xterm256)
}

pub fn detect_color_mode_label(
    term: Option<&str>,
    term_program: Option<&str>,
    colorterm: Option<&str>,
    kitty_window_id: Option<&str>,
    ghostty_resources_dir: Option<&str>,
) -> &'static str {
    let env = build_terminal_env(
        term,
        term_program,
        colorterm,
        kitty_window_id,
        ghostty_resources_dir,
        None,
    );
    match resolve_color_capability(&env) {
        ColorCapability::TrueColor => "truecolor",
        ColorCapability::Xterm256 => "256-color fallback",
    }
}

pub fn check_terminal_env_with_env(env: &TerminalEnv) -> DoctorCheck {
    let term = env.term.as_deref();
    let term_program = env.term_program.as_deref();
    let colorterm = env.colorterm.as_deref();
    let locale = env.locale.as_deref();

    let term_ok = term.is_some_and(|t| !t.eq_ignore_ascii_case("dumb"));
    let utf8_ok = locale.is_some_and(|loc| {
        let lower = loc.to_ascii_lowercase();
        lower.contains("utf-8") || lower.contains("utf8")
    });

    let status = if term_ok && utf8_ok {
        CheckStatus::Ok
    } else {
        CheckStatus::Warn
    };

    let color_cap = resolve_color_capability(env);
    let color_mode = match color_cap {
        ColorCapability::TrueColor => "truecolor",
        ColorCapability::Xterm256 => "256-color fallback",
    };

    let registry = default_registry();
    let resolved_adapter = registry.resolve(env);
    let is_apple_terminal = resolved_adapter
        .as_ref()
        .is_some_and(|a| a.kind() == TerminalKind::AppleTerminal);

    let mut detail = format!(
        "TERM={}, TERM_PROGRAM={}, COLORTERM={} (color={color_mode}), locale={}",
        term.unwrap_or("<unset>"),
        term_program.unwrap_or("<unset>"),
        colorterm.unwrap_or("<unset>"),
        locale.unwrap_or("<unset>"),
    );
    if is_apple_terminal {
        if color_cap != ColorCapability::TrueColor {
            detail.push_str("; Apple_Terminal uses 256-color palette fallback automatically");
        }
        if let Some(hint) = resolved_adapter
            .as_ref()
            .and_then(|adapter| adapter.diagnostic_hints(env).into_iter().next())
        {
            detail.push_str("; ");
            detail.push_str(&hint.message);
        }
    }

    DoctorCheck {
        name: "terminal_env".to_string(),
        status,
        detail,
    }
}

pub fn check_terminal_env_from(
    term: Option<&str>,
    term_program: Option<&str>,
    colorterm: Option<&str>,
    kitty_window_id: Option<&str>,
    ghostty_resources_dir: Option<&str>,
    locale: Option<&str>,
) -> DoctorCheck {
    let env = build_terminal_env(
        term,
        term_program,
        colorterm,
        kitty_window_id,
        ghostty_resources_dir,
        locale,
    );
    check_terminal_env_with_env(&env)
}

fn check_terminal_env() -> DoctorCheck {
    check_terminal_env_with_env(&TerminalEnv::from_env())
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

    #[test]
    fn test_detect_color_mode_label_identifies_truecolor_and_fallback() {
        assert_eq!(
            detect_color_mode_label(Some("xterm-256color"), None, Some("truecolor"), None, None),
            "truecolor"
        );
        assert_eq!(
            detect_color_mode_label(Some("xterm-256color"), None, Some("24bit"), None, None),
            "truecolor"
        );
        for term_prog in ["iTerm.app", "WezTerm", "ghostty", "vscode"] {
            assert_eq!(
                detect_color_mode_label(Some("xterm-256color"), Some(term_prog), None, None, None),
                "truecolor",
                "expected truecolor for {term_prog}"
            );
        }
        assert_eq!(
            detect_color_mode_label(Some("xterm-256color"), None, None, Some("1"), None),
            "truecolor"
        );
        assert_eq!(
            detect_color_mode_label(
                Some("xterm-256color"),
                None,
                None,
                None,
                Some("/Applications/Ghostty.app/Contents/Resources/ghostty")
            ),
            "truecolor"
        );
        assert_eq!(
            detect_color_mode_label(Some("xterm-truecolor"), None, None, None, None),
            "truecolor"
        );
        assert_eq!(
            detect_color_mode_label(
                Some("xterm-256color"),
                Some("Apple_Terminal"),
                None,
                None,
                None
            ),
            "256-color fallback"
        );
    }

    #[test]
    fn test_check_terminal_env_from_apple_terminal_vs_truecolor() {
        let apple = check_terminal_env_from(
            Some("xterm-256color"),
            Some("Apple_Terminal"),
            None,
            None,
            None,
            Some("en_US.UTF-8"),
        );
        assert_eq!(apple.status, CheckStatus::Ok);
        assert!(apple.detail.contains("TERM_PROGRAM=Apple_Terminal"));
        assert!(apple.detail.contains("256-color fallback"));
        assert!(apple.detail.contains("256-color palette fallback"));
        assert!(apple.detail.contains("Ctrl+G"));
        assert!(apple.detail.contains("grein keys"));

        let iterm = check_terminal_env_from(
            Some("xterm-256color"),
            Some("iTerm.app"),
            None,
            None,
            None,
            Some("en_US.UTF-8"),
        );
        assert_eq!(iterm.status, CheckStatus::Ok);
        assert!(iterm.detail.contains("TERM_PROGRAM=iTerm.app"));
        assert!(iterm.detail.contains("truecolor"));
        assert!(!iterm.detail.contains("256-color fallback"));

        let dumb = check_terminal_env_from(
            Some("dumb"),
            Some("Apple_Terminal"),
            None,
            None,
            None,
            Some("C"),
        );
        assert_eq!(dumb.status, CheckStatus::Warn);
    }
}
