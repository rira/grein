use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Parser, Clone, PartialEq, Eq)]
#[command(name = "grein", version, about = "Grein autonomous engineering CLI")]
pub struct Cli {
    #[arg(
        long,
        global = true,
        help = "Start a fresh session in an ACP agent branch instead of a terminal"
    )]
    pub agent: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum Command {
    #[command(about = "Run in standalone mode")]
    Standalone {},
    #[command(about = "Run daemon background server")]
    Daemon(DaemonArgs),
    #[command(about = "Attach client to running daemon")]
    Attach(AttachArgs),
    #[command(about = "List session threads")]
    Threads(ThreadsArgs),
    #[command(about = "Show workspace status")]
    Status(StatusArgs),
    #[command(about = "Inspect ACP agents")]
    Agents(AgentsArgs),
    #[command(about = "Run environment and workspace diagnostics")]
    Doctor(DoctorArgs),
    /// Inspect raw terminal key events and macOS modifier support
    Keys(KeysArgs),
    #[command(about = "Inspect terminal capabilities and configure native terminal settings")]
    Terminal(TerminalArgs),
    #[command(about = "Export or import global user settings")]
    Settings(SettingsArgs),
    #[command(about = "Export or import theme presets and color overrides")]
    Theme(ThemeArgs),
}

#[derive(Debug, Args, Clone, PartialEq, Eq, Default)]
pub struct DaemonArgs {
    #[arg(short, long, value_name = "SOCKET", help = "Unix domain socket path")]
    pub socket: Option<PathBuf>,
}

#[derive(Debug, Args, Clone, PartialEq, Eq, Default)]
pub struct AttachArgs {
    #[arg(short, long, value_name = "SOCKET", help = "Unix domain socket path")]
    pub socket: Option<PathBuf>,
}

#[derive(Debug, Args, Clone, PartialEq, Eq, Default)]
pub struct ThreadsArgs {
    #[arg(long, help = "Output in JSON format")]
    pub json: bool,
    #[arg(long, value_name = "DB", help = "Session database path override")]
    pub db: Option<PathBuf>,
}

#[derive(Debug, Args, Clone, PartialEq, Eq, Default)]
pub struct StatusArgs {
    #[arg(long, help = "Output in JSON format")]
    pub json: bool,
    #[arg(
        short,
        long,
        value_name = "SOCKET",
        help = "Unix domain socket path override"
    )]
    pub socket: Option<PathBuf>,
    #[arg(long, value_name = "DB", help = "Session database path override")]
    pub db: Option<PathBuf>,
}

#[derive(Debug, Args, Clone, PartialEq, Eq, Default)]
pub struct AgentsArgs {
    #[arg(long, help = "Output in JSON format")]
    pub json: bool,
    #[arg(
        short,
        long,
        value_name = "CONFIG",
        help = "Agent configuration file path override"
    )]
    pub config: Option<PathBuf>,
}

#[derive(Debug, Args, Clone, PartialEq, Eq, Default)]
pub struct DoctorArgs {
    #[arg(long, help = "Output in JSON format")]
    pub json: bool,
}

#[derive(Debug, Args, Clone, PartialEq, Eq, Default)]
pub struct KeysArgs {
    /// Disable kitty/CSI-u keyboard enhancement protocol probing
    #[arg(long)]
    pub no_enhance: bool,
    /// Print raw hex and escape bytes for each key event
    #[arg(long)]
    pub raw: bool,
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct TerminalArgs {
    #[command(subcommand)]
    pub command: TerminalCommand,
}

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum TerminalCommand {
    #[command(about = "Show active terminal capabilities and setup status")]
    Status(TerminalStatusArgs),
    #[command(about = "Plan, apply, or revert native terminal configuration")]
    Setup(TerminalSetupArgs),
}

#[derive(Debug, Args, Clone, PartialEq, Eq, Default)]
pub struct TerminalStatusArgs {
    #[arg(long, help = "Output in JSON format")]
    pub json: bool,
}

#[derive(Debug, Args, Clone, PartialEq, Eq, Default)]
pub struct TerminalSetupArgs {
    #[arg(
        value_name = "APP",
        default_value = "auto",
        help = "Terminal target (auto, apple-terminal, iterm2, ghostty, kitty, wezterm, alacritty, tmux, zellij)"
    )]
    pub app: String,
    #[arg(
        long,
        conflicts_with_all = ["apply", "revert"],
        help = "Preview planned changes without writing files (default)"
    )]
    pub dry_run: bool,
    #[arg(
        long,
        conflicts_with_all = ["dry_run", "revert"],
        help = "Apply configuration changes with .grein.bak backups"
    )]
    pub apply: bool,
    #[arg(
        long,
        conflicts_with_all = ["dry_run", "apply"],
        help = "Revert configuration changes and restore backups"
    )]
    pub revert: bool,
    #[arg(long, help = "Output in JSON format")]
    pub json: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormatArg {
    Toml,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum InputFormatArg {
    #[default]
    Auto,
    Toml,
    Json,
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct SettingsArgs {
    #[command(subcommand)]
    pub command: SettingsCommand,
}

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum SettingsCommand {
    #[command(about = "Export user preferences to TOML or JSON")]
    Export(SettingsExportArgs),
    #[command(about = "Import user preferences from TOML or JSON (or '-' for stdin)")]
    Import(SettingsImportArgs),
}

#[derive(Debug, Args, Clone, PartialEq, Eq, Default)]
pub struct SettingsExportArgs {
    #[arg(value_name = "FILE", help = "Destination file path (omit or '-' for stdout)")]
    pub file: Option<PathBuf>,
    #[arg(long, value_enum, help = "Output format (toml or json)")]
    pub format: Option<OutputFormatArg>,
    #[arg(
        long,
        value_name = "SECTIONS",
        value_delimiter = ',',
        help = "Comma-separated sections to include (e.g. theme,keybinding,ui)"
    )]
    pub only: Vec<String>,
}

#[derive(Debug, Args, Clone, PartialEq, Eq, Default)]
pub struct SettingsImportArgs {
    #[arg(value_name = "FILE", help = "Source file path or '-' for stdin")]
    pub file: PathBuf,
    #[arg(long, value_enum, default_value_t = InputFormatArg::Auto, help = "Input format")]
    pub format: InputFormatArg,
    #[arg(
        long,
        value_name = "SECTIONS",
        value_delimiter = ',',
        help = "Comma-separated sections to import (e.g. theme,keybinding,ui)"
    )]
    pub only: Vec<String>,
    #[arg(
        long,
        help = "Replace targeted section(s) or all preferences instead of merging"
    )]
    pub replace: bool,
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct ThemeArgs {
    #[command(subcommand)]
    pub command: ThemeCommand,
}

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum ThemeCommand {
    #[command(about = "Export active resolved theme palette to TOML or JSON")]
    Export(ThemeExportArgs),
    #[command(about = "Import theme preset or color overrides from TOML or JSON (or '-' for stdin)")]
    Import(ThemeImportArgs),
}

#[derive(Debug, Args, Clone, PartialEq, Eq, Default)]
pub struct ThemeExportArgs {
    #[arg(value_name = "FILE", help = "Destination file path (omit or '-' for stdout)")]
    pub file: Option<PathBuf>,
    #[arg(long, value_enum, help = "Output format (toml or json)")]
    pub format: Option<OutputFormatArg>,
}

#[derive(Debug, Args, Clone, PartialEq, Eq, Default)]
pub struct ThemeImportArgs {
    #[arg(value_name = "FILE", help = "Source file path or '-' for stdin")]
    pub file: PathBuf,
    #[arg(long, value_enum, default_value_t = InputFormatArg::Auto, help = "Input format")]
    pub format: InputFormatArg,
    #[arg(long, help = "Replace theme overrides instead of merging")]
    pub replace: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_keys_subcommand_and_flags() {
        let default = Cli::try_parse_from(["grein", "keys"]).expect("parse grein keys");
        assert_eq!(
            default.command,
            Some(Command::Keys(KeysArgs {
                no_enhance: false,
                raw: false,
            }))
        );

        let no_enhance =
            Cli::try_parse_from(["grein", "keys", "--no-enhance", "--raw"]).expect("parse flags");
        assert_eq!(
            no_enhance.command,
            Some(Command::Keys(KeysArgs {
                no_enhance: true,
                raw: true,
            }))
        );
    }

    #[test]
    fn parses_terminal_subcommands_and_rejects_conflicting_setup_flags() {
        let status = Cli::try_parse_from(["grein", "terminal", "status", "--json"])
            .expect("parse terminal status");
        assert_eq!(
            status.command,
            Some(Command::Terminal(TerminalArgs {
                command: TerminalCommand::Status(TerminalStatusArgs { json: true }),
            }))
        );

        let setup = Cli::try_parse_from(["grein", "terminal", "setup", "ghostty", "--dry-run", "--json"])
            .expect("parse terminal setup");
        assert_eq!(
            setup.command,
            Some(Command::Terminal(TerminalArgs {
                command: TerminalCommand::Setup(TerminalSetupArgs {
                    app: "ghostty".to_string(),
                    dry_run: true,
                    apply: false,
                    revert: false,
                    json: true,
                }),
            }))
        );

        assert!(Cli::try_parse_from(["grein", "terminal", "setup", "kitty", "--apply", "--revert"]).is_err());
    }
}
