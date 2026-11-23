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
