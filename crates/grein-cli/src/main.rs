use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "grein", version, about = "Grein CLI - terminal AI workspace", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Launch in standalone mode with embedded engine and TUI
    Standalone,
    /// Run Grein in headless daemon mode
    Daemon {
        /// Unix domain socket path to bind
        #[arg(long, value_name = "SOCKET")]
        socket: Option<PathBuf>,
        /// Validate daemon configuration and socket path without running
        #[arg(long)]
        dry_run: bool,
    },
    /// Attach to an active Grein daemon session
    Attach {
        /// Unix domain socket path of running daemon
        #[arg(long, value_name = "SOCKET")]
        socket: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();

    match cli.command {
        None | Some(Command::Standalone) => {
            let _engine_task = tokio::spawn(async {});
            grein_tui::run().await?;
        }
        Some(Command::Daemon { socket, dry_run }) => {
            if dry_run {
                if let Some(path) = socket {
                    if let Some(parent) = path.parent() {
                        if !parent.as_os_str().is_empty() && !parent.exists() {
                            std::fs::create_dir_all(parent)?;
                        }
                    }
                }
                return Ok(());
            }
        }
        Some(Command::Attach { socket: _ }) => {
            grein_tui::run().await?;
        }
    }

    Ok(())
}
