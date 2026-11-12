use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser, Clone, PartialEq, Eq)]
#[command(name = "grein", about = "Grein autonomous engineering CLI")]
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
