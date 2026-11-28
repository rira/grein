use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use assert_cmd::Command;
use grein_cli::terminal::port::{
    ConfigFsPort, InstallStatus, OsFamily, TerminalSetupError,
};
use grein_cli::terminal::providers::default_providers;
use grein_cli::terminal::TerminalSetupHost;
use grein_tui::terminal::{TerminalEnv, TerminalKind};

#[derive(Debug, Default)]
struct FsState {
    files: BTreeMap<PathBuf, String>,
    running_processes: BTreeSet<String>,
    recorded_commands: Vec<(String, Vec<String>)>,
    command_outputs: BTreeMap<(String, Vec<String>), String>,
}

struct InMemoryConfigFs {
    home: Option<PathBuf>,
    xdg: Option<PathBuf>,
    os: OsFamily,
    state: Mutex<FsState>,
}

impl InMemoryConfigFs {
    fn new(home: &str, xdg: Option<&str>, os: OsFamily) -> Self {
        Self {
            home: Some(PathBuf::from(home)),
            xdg: xdg.map(PathBuf::from),
            os,
            state: Mutex::new(FsState::default()),
        }
    }

    fn seed_file(&self, path: &str, content: &str) {
        self.state
            .lock()
            .expect("lock")
            .files
            .insert(PathBuf::from(path), content.to_string());
    }

    fn read_file(&self, path: &str) -> Option<String> {
        self.state
            .lock()
            .expect("lock")
            .files
            .get(Path::new(path))
            .cloned()
    }

    fn has_file(&self, path: &str) -> bool {
        self.state
            .lock()
            .expect("lock")
            .files
            .contains_key(Path::new(path))
    }

    fn set_running(&self, process: &str, running: bool) {
        let mut guard = self.state.lock().expect("lock");
        if running {
            guard.running_processes.insert(process.to_string());
        } else {
            guard.running_processes.remove(process);
        }
    }

    fn recorded_commands(&self) -> Vec<(String, Vec<String>)> {
        self.state.lock().expect("lock").recorded_commands.clone()
    }
}

impl ConfigFsPort for InMemoryConfigFs {
    fn home_dir(&self) -> Option<PathBuf> {
        self.home.clone()
    }

    fn xdg_config_home(&self) -> Option<PathBuf> {
        self.xdg.clone()
    }

    fn os_family(&self) -> OsFamily {
        self.os
    }

    fn exists(&self, path: &Path) -> bool {
        self.state.lock().expect("lock").files.contains_key(path)
    }

    fn read_to_string(&self, path: &Path) -> Result<Option<String>, TerminalSetupError> {
        Ok(self.state.lock().expect("lock").files.get(path).cloned())
    }

    fn write_atomic(&self, path: &Path, content: &str) -> Result<(), TerminalSetupError> {
        self.state
            .lock()
            .expect("lock")
            .files
            .insert(path.to_path_buf(), content.to_string());
        Ok(())
    }

    fn copy_file(&self, from: &Path, to: &Path) -> Result<(), TerminalSetupError> {
        let mut guard = self.state.lock().expect("lock");
        let data = guard
            .files
            .get(from)
            .cloned()
            .ok_or_else(|| TerminalSetupError::Io {
                path: from.to_path_buf(),
                message: "source file not found".to_string(),
            })?;
        guard.files.insert(to.to_path_buf(), data);
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> Result<(), TerminalSetupError> {
        self.state.lock().expect("lock").files.remove(path);
        Ok(())
    }

    fn is_process_running(&self, name: &str) -> bool {
        self.state
            .lock()
            .expect("lock")
            .running_processes
            .contains(name)
    }

    fn run_command(&self, program: &str, args: &[&str]) -> Result<String, TerminalSetupError> {
        let args_vec: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
        let mut guard = self.state.lock().expect("lock");
        guard
            .recorded_commands
            .push((program.to_string(), args_vec.clone()));
        Ok(guard
            .command_outputs
            .get(&(program.to_string(), args_vec))
            .cloned()
            .unwrap_or_default())
    }
}

fn make_host(fs: Arc<InMemoryConfigFs>) -> TerminalSetupHost {
    TerminalSetupHost::new(default_providers(), fs)
}

#[test]
fn test_apple_terminal_macos_lifecycle_and_running_guard() {
    let fs = Arc::new(InMemoryConfigFs::new("/Users/alice", None, OsFamily::MacOs));
    fs.seed_file("/Users/alice/Library/Preferences/com.apple.Terminal.plist", "ORIGINAL_PLIST");
    let host = make_host(Arc::clone(&fs));
    let env = TerminalEnv::default().with_term_program("Apple_Terminal");

    fs.set_running("Terminal", true);
    let plan = host.plan("apple-terminal", &env).expect("plan");
    assert_eq!(plan.status_before, InstallStatus::NotConfigured);
    assert!(!plan.changes.is_empty());
    assert!(plan.warnings.iter().any(|w| w.contains("cfprefsd")));

    let guard_err = host
        .apply("apple-terminal", &env)
        .expect_err("must block apply when Terminal.app is running");
    assert!(matches!(guard_err, TerminalSetupError::ProcessRunning { .. }));

    fs.set_running("Terminal", false);
    let receipt = host.apply("apple-terminal", &env).expect("apply");
    assert!(receipt.applied);
    assert_eq!(receipt.status_after, InstallStatus::Configured);
    assert!(fs.has_file("/Users/alice/Library/Preferences/com.apple.Terminal.plist.grein.bak"));

    let snippet = fs
        .read_file("/Users/alice/.config/grein/apple-terminal/grein.terminal")
        .expect("grein.terminal");
    assert!(snippet.contains("useOptionAsMetaKey"));
    assert!(snippet.contains("$F700"));
    assert!(snippet.contains("\\033[1;2A"));
    assert!(snippet.contains("$F703"));
    assert!(snippet.contains("\\033[1;2C"));

    let cmds = fs.recorded_commands();
    assert!(cmds.iter().any(|(prog, _)| prog == "defaults"));
    assert!(cmds.iter().any(|(prog, _)| prog.contains("PlistBuddy")));

    let second = host.apply("apple-terminal", &env).expect("second apply");
    assert!(!second.applied);
    assert_eq!(second.status_after, InstallStatus::Configured);

    let revert = host.revert("apple-terminal", &env).expect("revert");
    assert!(revert.reverted);
    assert!(!fs.has_file("/Users/alice/.config/grein/apple-terminal/grein.terminal"));
    assert!(!fs.has_file("/Users/alice/Library/Preferences/com.apple.Terminal.plist.grein.bak"));
    assert_eq!(
        fs.read_file("/Users/alice/Library/Preferences/com.apple.Terminal.plist")
            .as_deref(),
        Some("ORIGINAL_PLIST")
    );
}

#[test]
fn test_macos_only_installers_report_not_applicable_on_linux() {
    let fs = Arc::new(InMemoryConfigFs::new("/home/alice", None, OsFamily::Linux));
    let host = make_host(fs);
    let env = TerminalEnv::default();

    let statuses = host.status(&env).expect("status");
    let apple = statuses
        .iter()
        .find(|e| e.kind == TerminalKind::AppleTerminal)
        .expect("apple entry");
    let iterm = statuses
        .iter()
        .find(|e| e.kind == TerminalKind::ITerm2)
        .expect("iterm entry");
    assert_eq!(apple.install_status, InstallStatus::NotApplicable);
    assert_eq!(iterm.install_status, InstallStatus::NotApplicable);

    let receipt = host.apply("iterm2", &env).expect("apply on linux");
    assert!(!receipt.applied);
    assert_eq!(receipt.status_after, InstallStatus::NotApplicable);
}

#[test]
fn test_iterm2_macos_lifecycle_idempotency_and_revert() {
    let fs = Arc::new(InMemoryConfigFs::new("/Users/alice", None, OsFamily::MacOs));
    let host = make_host(Arc::clone(&fs));
    let env = TerminalEnv::default().with_term_program("iTerm.app");

    let plan = host.plan("auto", &env).expect("plan");
    assert_eq!(plan.terminal, TerminalKind::ITerm2);
    assert_eq!(plan.status_before, InstallStatus::NotConfigured);
    assert_eq!(plan.changes.len(), 1);

    let receipt = host.apply("iterm2", &env).expect("apply");
    assert!(receipt.applied);
    assert_eq!(receipt.status_after, InstallStatus::Configured);
    let json_text = fs
        .read_file("/Users/alice/Library/Application Support/iTerm2/DynamicProfiles/grein.json")
        .expect("grein.json");
    let parsed: serde_json::Value = serde_json::from_str(&json_text).expect("valid json");
    assert_eq!(parsed["Profiles"][0]["Dynamic Profile Parent Name"], "Default");
    assert_eq!(parsed["Profiles"][0]["Option Key Sends"], 2);
    assert_eq!(parsed["Profiles"][0]["Keyboard Map"]["0xf700-0x220000"]["Text"], "[1;2A");
    assert_eq!(parsed["Profiles"][0]["Keyboard Map"]["0xd-0x20000"]["Text"], "[13;2u");

    let second = host.apply("iterm2", &env).expect("second apply");
    assert!(!second.applied);
    assert_eq!(second.status_after, InstallStatus::Configured);

    let revert = host.revert("iterm2", &env).expect("revert");
    assert!(revert.reverted);
    assert!(!fs.has_file("/Users/alice/Library/Application Support/iTerm2/DynamicProfiles/grein.json"));
}

#[test]
fn test_ghostty_macos_vs_xdg_precedence_backup_idempotency_and_revert() {
    // macOS default prefers Application Support/com.mitchellh.ghostty
    let mac_fs = Arc::new(InMemoryConfigFs::new("/Users/alice", None, OsFamily::MacOs));
    mac_fs.seed_file(
        "/Users/alice/Library/Application Support/com.mitchellh.ghostty/config",
        "font-size = 14\n",
    );
    let mac_host = make_host(Arc::clone(&mac_fs));
    let env = TerminalEnv::default().with_term_program("ghostty");

    let receipt = mac_host.apply("auto", &env).expect("mac apply");
    assert!(receipt.applied);
    assert_eq!(receipt.terminal, TerminalKind::Ghostty);
    assert!(mac_fs.has_file("/Users/alice/Library/Application Support/com.mitchellh.ghostty/grein.conf"));
    assert!(mac_fs.has_file("/Users/alice/Library/Application Support/com.mitchellh.ghostty/config.grein.bak"));

    let updated = mac_fs
        .read_file("/Users/alice/Library/Application Support/com.mitchellh.ghostty/config")
        .expect("config");
    assert!(updated.contains("font-size = 14"));
    assert!(updated.contains("config-file = ?grein.conf"));

    // Idempotent second run
    let second = mac_host.apply("ghostty", &env).expect("second");
    assert!(!second.applied);
    assert_eq!(second.status_after, InstallStatus::Configured);

    // Revert restores backup and removes grein.conf
    let revert = mac_host.revert("ghostty", &env).expect("revert");
    assert!(revert.reverted);
    assert_eq!(
        mac_fs
            .read_file("/Users/alice/Library/Application Support/com.mitchellh.ghostty/config")
            .as_deref(),
        Some("font-size = 14\n")
    );
    assert!(!mac_fs.has_file("/Users/alice/Library/Application Support/com.mitchellh.ghostty/grein.conf"));

    // macOS falls back to XDG if XDG config exists and Application Support config does not
    let mac_xdg_fs = Arc::new(InMemoryConfigFs::new("/Users/alice", None, OsFamily::MacOs));
    mac_xdg_fs.seed_file("/Users/alice/.config/ghostty/config", "theme = nord\n");
    let mac_xdg_host = make_host(Arc::clone(&mac_xdg_fs));
    let plan = mac_xdg_host.plan("ghostty", &env).expect("plan");
    assert!(plan
        .changes
        .iter()
        .any(|c| c.path == Path::new("/Users/alice/.config/ghostty/grein.conf")));

    // Linux uses XDG_CONFIG_HOME
    let linux_fs = Arc::new(InMemoryConfigFs::new(
        "/home/alice",
        Some("/home/alice/.xdg"),
        OsFamily::Linux,
    ));
    let linux_host = make_host(Arc::clone(&linux_fs));
    linux_host.apply("ghostty", &env).expect("linux apply");
    assert!(linux_fs.has_file("/home/alice/.xdg/ghostty/grein.conf"));
    assert!(linux_fs.has_file("/home/alice/.xdg/ghostty/config"));
}

#[test]
fn test_kitty_lifecycle_backup_idempotency_and_revert() {
    let fs = Arc::new(InMemoryConfigFs::new("/home/alice", None, OsFamily::Linux));
    fs.seed_file("/home/alice/.config/kitty/kitty.conf", "font_size 13.0\n");
    let host = make_host(Arc::clone(&fs));
    let env = TerminalEnv::default().with_kitty_window_id("1");

    let plan = host.plan("auto", &env).expect("plan");
    assert_eq!(plan.terminal, TerminalKind::Kitty);
    assert_eq!(plan.status_before, InstallStatus::NotConfigured);
    assert_eq!(plan.changes.len(), 2);

    let receipt = host.apply("kitty", &env).expect("apply");
    assert!(receipt.applied);
    assert_eq!(receipt.status_after, InstallStatus::Configured);
    assert_eq!(
        fs.read_file("/home/alice/.config/kitty/grein.conf").as_deref(),
        Some("macos_option_as_alt yes\n")
    );
    let kitty_conf = fs
        .read_file("/home/alice/.config/kitty/kitty.conf")
        .expect("kitty.conf");
    assert!(kitty_conf.contains("font_size 13.0"));
    assert!(kitty_conf.contains("include grein.conf"));
    assert!(fs.has_file("/home/alice/.config/kitty/kitty.conf.grein.bak"));

    let second = host.apply("kitty", &env).expect("second");
    assert!(!second.applied);
    assert_eq!(second.status_after, InstallStatus::Configured);

    let revert = host.revert("kitty", &env).expect("revert");
    assert!(revert.reverted);
    assert_eq!(
        fs.read_file("/home/alice/.config/kitty/kitty.conf").as_deref(),
        Some("font_size 13.0\n")
    );
    assert!(!fs.has_file("/home/alice/.config/kitty/grein.conf"));
}

#[test]
fn test_wezterm_inserts_before_return_and_reverts() {
    let fs = Arc::new(InMemoryConfigFs::new("/home/alice", None, OsFamily::Linux));
    fs.seed_file(
        "/home/alice/.wezterm.lua",
        "local wezterm = require('wezterm')\nlocal config = {}\nreturn config\n",
    );
    let host = make_host(Arc::clone(&fs));
    let env = TerminalEnv::default().with_term_program("WezTerm");

    let receipt = host.apply("auto", &env).expect("apply");
    assert!(receipt.applied);
    assert_eq!(receipt.terminal, TerminalKind::WezTerm);
    assert_eq!(receipt.status_after, InstallStatus::Configured);

    let grein_lua = fs
        .read_file("/home/alice/.config/wezterm/grein.lua")
        .expect("grein.lua");
    assert!(grein_lua.contains("enable_kitty_keyboard = true"));

    let wezterm_lua = fs.read_file("/home/alice/.wezterm.lua").expect(".wezterm.lua");
    let block_pos = wezterm_lua
        .find("-- BEGIN GREIN TERMINAL CONFIG")
        .expect("begin marker");
    let return_pos = wezterm_lua.find("return config").expect("return config");
    assert!(block_pos < return_pos, "loader block must precede return config");

    let second = host.apply("wezterm", &env).expect("second");
    assert!(!second.applied);
    assert_eq!(second.status_after, InstallStatus::Configured);

    let revert = host.revert("wezterm", &env).expect("revert");
    assert!(revert.reverted);
    assert_eq!(
        fs.read_file("/home/alice/.wezterm.lua").as_deref(),
        Some("local wezterm = require('wezterm')\nlocal config = {}\nreturn config\n")
    );
    assert!(!fs.has_file("/home/alice/.config/wezterm/grein.lua"));
}

#[test]
fn test_alacritty_updates_existing_general_import_and_reverts() {
    let fs = Arc::new(InMemoryConfigFs::new("/home/alice", None, OsFamily::Linux));
    fs.seed_file(
        "/home/alice/.config/alacritty/alacritty.toml",
        "[general]\nimport = [\"~/.config/alacritty/theme.toml\"]\n\n[font]\nsize = 12\n",
    );
    let host = make_host(Arc::clone(&fs));
    let env = TerminalEnv::default().with_term_program("Alacritty");

    let receipt = host.apply("alacritty", &env).expect("apply");
    assert!(receipt.applied);
    assert_eq!(receipt.status_after, InstallStatus::Configured);
    assert_eq!(
        fs.read_file("/home/alice/.config/alacritty/grein.toml")
            .as_deref(),
        Some("[window]\noption_as_alt = \"Both\"\n")
    );
    let main_toml = fs
        .read_file("/home/alice/.config/alacritty/alacritty.toml")
        .expect("alacritty.toml");
    assert!(main_toml.contains("theme.toml"));
    assert!(main_toml.contains("/home/alice/.config/alacritty/grein.toml"));

    let second = host.apply("alacritty", &env).expect("second");
    assert!(!second.applied);
    assert_eq!(second.status_after, InstallStatus::Configured);

    let revert = host.revert("alacritty", &env).expect("revert");
    assert!(revert.reverted);
    assert_eq!(
        fs.read_file("/home/alice/.config/alacritty/alacritty.toml")
            .as_deref(),
        Some("[general]\nimport = [\"~/.config/alacritty/theme.toml\"]\n\n[font]\nsize = 12\n")
    );
    assert!(!fs.has_file("/home/alice/.config/alacritty/grein.toml"));
}

#[test]
fn test_tmux_precedence_idempotency_and_revert() {
    let fs = Arc::new(InMemoryConfigFs::new("/home/alice", None, OsFamily::Linux));
    fs.seed_file("/home/alice/.tmux.conf", "set -g mouse on\n");
    let host = make_host(Arc::clone(&fs));
    let env = TerminalEnv::default().with_tmux("/tmp/tmux-1000/default,123,0");

    let plan = host.plan("auto", &env).expect("plan");
    assert_eq!(plan.terminal, TerminalKind::Tmux);
    assert_eq!(plan.status_before, InstallStatus::NotConfigured);

    let receipt = host.apply("tmux", &env).expect("apply");
    assert!(receipt.applied);
    assert_eq!(receipt.status_after, InstallStatus::Configured);

    let grein_conf = fs
        .read_file("/home/alice/.config/tmux/grein.conf")
        .expect("grein.conf");
    assert!(grein_conf.contains("set -g xterm-keys on"));
    assert!(grein_conf.contains("set -g extended-keys always"));
    assert!(grein_conf.contains("set -g extended-keys-format csi-u"));
    assert!(grein_conf.contains("set -as terminal-features ',*:RGB:extkeys'"));
    assert!(grein_conf.contains("set -sg escape-time 10"));

    let tmux_conf = fs.read_file("/home/alice/.tmux.conf").expect(".tmux.conf");
    assert!(tmux_conf.contains("source-file -q /home/alice/.config/tmux/grein.conf"));

    let second = host.apply("tmux", &env).expect("second");
    assert!(!second.applied);

    let revert = host.revert("tmux", &env).expect("revert");
    assert!(revert.reverted);
    assert_eq!(
        fs.read_file("/home/alice/.tmux.conf").as_deref(),
        Some("set -g mouse on\n")
    );
    assert!(!fs.has_file("/home/alice/.config/tmux/grein.conf"));
}

#[test]
fn test_zellij_lifecycle_idempotency_and_revert() {
    let fs = Arc::new(InMemoryConfigFs::new("/home/alice", None, OsFamily::Linux));
    fs.seed_file("/home/alice/.config/zellij/config.kdl", "theme \"default\"\n");
    let host = make_host(Arc::clone(&fs));
    let env = TerminalEnv::default().with_zellij("0");

    let receipt = host.apply("auto", &env).expect("apply");
    assert!(receipt.applied);
    assert_eq!(receipt.terminal, TerminalKind::Zellij);
    assert_eq!(receipt.status_after, InstallStatus::Configured);

    let snippet = fs
        .read_file("/home/alice/.config/zellij/grein-snippet.kdl")
        .expect("grein-snippet.kdl");
    assert!(snippet.contains("support_kitty_keyboard_protocol true"));

    let config_kdl = fs
        .read_file("/home/alice/.config/zellij/config.kdl")
        .expect("config.kdl");
    assert!(config_kdl.contains("// BEGIN GREIN TERMINAL CONFIG"));
    assert!(config_kdl.contains("support_kitty_keyboard_protocol true"));

    let second = host.apply("zellij", &env).expect("second");
    assert!(!second.applied);

    let revert = host.revert("zellij", &env).expect("revert");
    assert!(revert.reverted);
    assert_eq!(
        fs.read_file("/home/alice/.config/zellij/config.kdl")
            .as_deref(),
        Some("theme \"default\"\n")
    );
    assert!(!fs.has_file("/home/alice/.config/zellij/grein-snippet.kdl"));
}

#[test]
fn test_host_rejects_unknown_and_unsupported_terminals() {
    let fs = Arc::new(InMemoryConfigFs::new("/home/alice", None, OsFamily::Linux));
    let host = make_host(fs);
    let env = TerminalEnv::default();

    let unknown = host
        .plan("not-a-terminal", &env)
        .expect_err("unknown terminal should fail");
    assert!(matches!(unknown, TerminalSetupError::UnknownTerminal(_)));

    let unsupported = host
        .plan("generic-xterm", &env)
        .expect_err("generic-xterm has no setup installer");
    assert!(matches!(
        unsupported,
        TerminalSetupError::UnsupportedTerminal(TerminalKind::GenericXterm)
    ));
}

#[test]
fn test_cli_terminal_status_and_setup_json_blackbox() {
    let temp = tempfile::tempdir().expect("tempdir");
    let xdg_dir = temp.path().join("xdg");

    let status_out = Command::cargo_bin("grein")
        .expect("grein bin")
        .args(["terminal", "status", "--json"])
        .env("HOME", temp.path())
        .env("XDG_CONFIG_HOME", &xdg_dir)
        .env("TERM", "xterm-256color")
        .env("TERM_PROGRAM", "ghostty")
        .env_remove("TMUX")
        .env_remove("ZELLIJ")
        .env_remove("ZELLIJ_SESSION_NAME")
        .env_remove("KITTY_WINDOW_ID")
        .env_remove("GREIN_TERMINAL")
        .output()
        .expect("run grein terminal status --json");

    assert!(
        status_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&status_out.stderr)
    );
    let status_stdout = String::from_utf8(status_out.stdout).expect("utf8 stdout");
    assert!(
        !status_stdout.contains('\x1b'),
        "JSON stdout must contain zero ANSI escapes: {status_stdout}"
    );
    let status_json: serde_json::Value =
        serde_json::from_str(&status_stdout).expect("valid status JSON");
    assert_eq!(status_json["active_terminal"], "ghostty");
    assert_eq!(status_json["entries"].as_array().expect("entries").len(), 8);

    let dry_run_out = Command::cargo_bin("grein")
        .expect("grein bin")
        .args(["terminal", "setup", "kitty", "--dry-run", "--json"])
        .env("HOME", temp.path())
        .env("XDG_CONFIG_HOME", &xdg_dir)
        .output()
        .expect("run grein terminal setup kitty --dry-run --json");

    assert!(
        dry_run_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&dry_run_out.stderr)
    );
    let dry_stdout = String::from_utf8(dry_run_out.stdout).expect("utf8 stdout");
    assert!(
        !dry_stdout.contains('\x1b'),
        "JSON stdout must contain zero ANSI escapes: {dry_stdout}"
    );
    let dry_json: serde_json::Value = serde_json::from_str(&dry_stdout).expect("valid plan JSON");
    assert_eq!(dry_json["terminal"], "kitty");
    assert_eq!(dry_json["dry_run"], true);
    assert_eq!(dry_json["status_before"], "not-configured");
    assert_eq!(dry_json["changes"].as_array().expect("changes").len(), 2);
    assert!(
        !xdg_dir.join("kitty/grein.conf").exists(),
        "dry-run must not write files"
    );

    let apply_out = Command::cargo_bin("grein")
        .expect("grein bin")
        .args(["terminal", "setup", "kitty", "--apply", "--json"])
        .env("HOME", temp.path())
        .env("XDG_CONFIG_HOME", &xdg_dir)
        .output()
        .expect("run grein terminal setup kitty --apply --json");
    assert!(apply_out.status.success());
    let apply_stdout = String::from_utf8(apply_out.stdout).expect("utf8 stdout");
    assert!(!apply_stdout.contains('\x1b'));
    let apply_json: serde_json::Value =
        serde_json::from_str(&apply_stdout).expect("valid receipt JSON");
    assert_eq!(apply_json["applied"], true);
    assert_eq!(apply_json["status_after"], "configured");
    assert!(xdg_dir.join("kitty/grein.conf").exists());

    let revert_out = Command::cargo_bin("grein")
        .expect("grein bin")
        .args(["terminal", "setup", "kitty", "--revert", "--json"])
        .env("HOME", temp.path())
        .env("XDG_CONFIG_HOME", &xdg_dir)
        .output()
        .expect("run grein terminal setup kitty --revert --json");
    assert!(revert_out.status.success());
    let revert_stdout = String::from_utf8(revert_out.stdout).expect("utf8 stdout");
    assert!(!revert_stdout.contains('\x1b'));
    let revert_json: serde_json::Value =
        serde_json::from_str(&revert_stdout).expect("valid revert JSON");
    assert_eq!(revert_json["reverted"], true);
    assert!(!xdg_dir.join("kitty/grein.conf").exists());

    let err_out = Command::cargo_bin("grein")
        .expect("grein bin")
        .args(["terminal", "setup", "unknown-app", "--dry-run", "--json"])
        .env("HOME", temp.path())
        .env("XDG_CONFIG_HOME", &xdg_dir)
        .output()
        .expect("run unknown app");
    assert!(!err_out.status.success());
    assert!(
        err_out.stdout.is_empty(),
        "stdout must be empty on error, got: {}",
        String::from_utf8_lossy(&err_out.stdout)
    );
    assert!(!err_out.stderr.is_empty());
}
