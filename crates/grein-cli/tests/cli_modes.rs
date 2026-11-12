use clap::Parser;
use grein_cli::args::{Cli, Command};
use std::path::PathBuf;

#[test]
fn test_standalone_mode_default_when_no_subcommand() {
    let args = ["grein"];
    let cli =
        Cli::try_parse_from(args).expect("Default standalone invocation must parse successfully");
    assert!(
        cli.command.is_none() || matches!(cli.command, Some(Command::Standalone { .. })),
        "No subcommand should resolve to standalone mode"
    );
}

#[test]
fn test_daemon_subcommand_defaults() {
    let args = ["grein", "daemon"];
    let cli = Cli::try_parse_from(args).expect("Daemon subcommand must parse successfully");
    match cli.command {
        Some(Command::Daemon(args)) => {
            assert!(
                args.socket.is_none()
                    || args.socket == Some(PathBuf::from("~/.local/share/grein/grein.sock"))
            );
        }
        other => panic!("Expected Command::Daemon, got {:?}", other),
    }
}

#[test]
fn test_daemon_subcommand_with_custom_socket() {
    let args = ["grein", "daemon", "--socket", "/tmp/test-grein.sock"];
    let cli =
        Cli::try_parse_from(args).expect("Daemon with custom socket flag must parse successfully");
    match cli.command {
        Some(Command::Daemon(args)) => {
            assert_eq!(args.socket, Some(PathBuf::from("/tmp/test-grein.sock")));
        }
        other => panic!("Expected Command::Daemon, got {:?}", other),
    }
}

#[test]
fn test_attach_subcommand_defaults() {
    let args = ["grein", "attach"];
    let cli = Cli::try_parse_from(args).expect("Attach subcommand must parse successfully");
    match cli.command {
        Some(Command::Attach(args)) => {
            assert!(
                args.socket.is_none()
                    || args.socket == Some(PathBuf::from("~/.local/share/grein/grein.sock"))
            );
        }
        other => panic!("Expected Command::Attach, got {:?}", other),
    }
}

#[test]
fn test_attach_subcommand_with_custom_socket() {
    let args = ["grein", "attach", "--socket", "/tmp/test-grein.sock"];
    let cli =
        Cli::try_parse_from(args).expect("Attach with custom socket flag must parse successfully");
    match cli.command {
        Some(Command::Attach(args)) => {
            assert_eq!(args.socket, Some(PathBuf::from("/tmp/test-grein.sock")));
        }
        other => panic!("Expected Command::Attach, got {:?}", other),
    }
}

#[test]
fn test_invalid_subcommand_fails_gracefully() {
    let args = ["grein", "nonexistent"];
    let result = Cli::try_parse_from(args);
    assert!(
        result.is_err(),
        "Unrecognized subcommand must fail validation"
    );
}
