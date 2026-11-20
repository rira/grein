use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser, Clone, PartialEq, Eq)]
#[command(name = "grein", version, about = "Grein autonomous engineering CLI")]
pub struct Cli {
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
