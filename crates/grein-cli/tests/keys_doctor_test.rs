use std::process::{Command, Stdio};

use clap::Parser;
use grein_cli::args::{Cli, Command as CliCommand, KeysArgs};

fn grein_cmd() -> Command {
    if let Ok(bin) = std::env::var("CARGO_BIN_EXE_grein") {
        Command::new(bin)
    } else {
        Command::new(env!("CARGO_BIN_EXE_grein"))
    }
}

#[test]
fn test_cli_parses_keys_subcommand_and_no_enhance_flag() {
    let default_keys = Cli::try_parse_from(["grein", "keys"]).expect("parse grein keys");
    assert_eq!(
        default_keys.command,
        Some(CliCommand::Keys(KeysArgs {
            no_enhance: false,
            raw: false,
        }))
    );

    let no_enhance_keys = Cli::try_parse_from(["grein", "keys", "--no-enhance", "--raw"])
        .expect("parse grein keys --no-enhance --raw");
    assert_eq!(
        no_enhance_keys.command,
        Some(CliCommand::Keys(KeysArgs {
            no_enhance: true,
            raw: true,
        }))
    );
}

#[test]
fn test_grein_keys_help_displays_description_and_no_enhance_flag() {
    let output = grein_cmd()
        .args(["keys", "--help"])
        .output()
        .expect("run grein keys --help");

    assert!(
        output.status.success(),
        "expected `grein keys --help` to succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--no-enhance"),
        "expected --no-enhance in help output: {stdout}"
    );
    assert!(
        stdout.contains("Inspect raw terminal key events"),
        "expected subcommand description in help output: {stdout}"
    );
}

#[test]
fn test_grein_keys_non_tty_fails_immediately_with_unsupported_message() {
    for args in [vec!["keys"], vec!["keys", "--no-enhance"]] {
        let output = grein_cmd()
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("run grein keys in non-TTY");

        assert!(
            !output.status.success(),
            "expected `grein {:?}` to fail outside a TTY",
            args
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("grein keys requires an interactive terminal (TTY)"),
            "expected explicit TTY error message, got stderr: {stderr}"
        );
    }
}

#[test]
fn test_grein_doctor_terminal_env_reports_apple_terminal_fallback_and_truecolor() {
    let temp = tempfile::tempdir().expect("tempdir");
    let prefs_db = temp.path().join("prefs.db");

    let apple_out = grein_cmd()
        .args(["doctor", "--json"])
        .env("GREIN_CONFIG_DB", &prefs_db)
        .env("TERM", "xterm-256color")
        .env("TERM_PROGRAM", "Apple_Terminal")
        .env("LC_ALL", "en_US.UTF-8")
        .env_remove("COLORTERM")
        .env_remove("KITTY_WINDOW_ID")
        .env_remove("GHOSTTY_RESOURCES_DIR")
        .current_dir(temp.path())
        .output()
        .expect("run grein doctor for Apple_Terminal");

    assert!(
        apple_out.status.success(),
        "expected Apple_Terminal doctor check to succeed, stderr: {}",
        String::from_utf8_lossy(&apple_out.stderr)
    );
    let apple_json: serde_json::Value =
        serde_json::from_slice(&apple_out.stdout).expect("parse doctor json");
    let apple_term_check = apple_json["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|c| c["name"] == "terminal_env")
        .expect("terminal_env check");
    assert_eq!(apple_term_check["status"], "ok");
    let apple_detail = apple_term_check["detail"].as_str().expect("detail string");
    assert!(apple_detail.contains("TERM=xterm-256color"), "{apple_detail}");
    assert!(
        apple_detail.contains("TERM_PROGRAM=Apple_Terminal"),
        "{apple_detail}"
    );
    assert!(apple_detail.contains("COLORTERM=<unset>"), "{apple_detail}");
    assert!(apple_detail.contains("256-color fallback"), "{apple_detail}");
    assert!(
        apple_detail.contains("256-color palette fallback"),
        "{apple_detail}"
    );
    assert!(apple_detail.contains("Ctrl+G"), "{apple_detail}");
    assert!(apple_detail.contains("grein keys"), "{apple_detail}");

    let iterm_out = grein_cmd()
        .args(["doctor", "--json"])
        .env("GREIN_CONFIG_DB", &prefs_db)
        .env("TERM", "xterm-256color")
        .env("TERM_PROGRAM", "iTerm.app")
        .env("LC_ALL", "en_US.UTF-8")
        .env_remove("COLORTERM")
        .env_remove("KITTY_WINDOW_ID")
        .env_remove("GHOSTTY_RESOURCES_DIR")
        .current_dir(temp.path())
        .output()
        .expect("run grein doctor for iTerm.app");

    assert!(iterm_out.status.success());
    let iterm_json: serde_json::Value =
        serde_json::from_slice(&iterm_out.stdout).expect("parse iterm doctor json");
    let iterm_detail = iterm_json["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|c| c["name"] == "terminal_env")
        .expect("terminal_env check")["detail"]
        .as_str()
        .expect("detail string");
    assert!(iterm_detail.contains("TERM_PROGRAM=iTerm.app"), "{iterm_detail}");
    assert!(iterm_detail.contains("truecolor"), "{iterm_detail}");
    assert!(
        !iterm_detail.contains("256-color fallback"),
        "{iterm_detail}"
    );

    let colorterm_out = grein_cmd()
        .args(["doctor", "--json"])
        .env("GREIN_CONFIG_DB", &prefs_db)
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .env("LC_ALL", "en_US.UTF-8")
        .env_remove("TERM_PROGRAM")
        .env_remove("KITTY_WINDOW_ID")
        .env_remove("GHOSTTY_RESOURCES_DIR")
        .current_dir(temp.path())
        .output()
        .expect("run grein doctor for COLORTERM=truecolor");

    assert!(colorterm_out.status.success());
    let colorterm_json: serde_json::Value =
        serde_json::from_slice(&colorterm_out.stdout).expect("parse colorterm doctor json");
    let colorterm_detail = colorterm_json["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|c| c["name"] == "terminal_env")
        .expect("terminal_env check")["detail"]
        .as_str()
        .expect("detail string");
    assert!(
        colorterm_detail.contains("TERM_PROGRAM=<unset>"),
        "{colorterm_detail}"
    );
    assert!(
        colorterm_detail.contains("COLORTERM=truecolor"),
        "{colorterm_detail}"
    );
    assert!(colorterm_detail.contains("truecolor"), "{colorterm_detail}");
}
