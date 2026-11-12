mod daemon;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "grein",
    version,
    about = "Grein CLI - Multi-Agent Terminal Development Environment"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run standalone interactive TUI session with embedded engine
    Standalone,
    /// Run headless Unix socket server
    Daemon {
        /// Socket path for the daemon IPC
        #[arg(long, default_value_os_t = daemon::default_socket_path())]
        socket: PathBuf,
    },
    /// Attach interactive TUI to a running daemon
    Attach {
        /// Socket path for the daemon IPC
        #[arg(long, default_value_os_t = daemon::default_socket_path())]
        socket: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();

    match cli.command {
        None | Some(Commands::Standalone) => {
            grein_tui::run().await?;
        }
        Some(Commands::Daemon { socket }) => {
            daemon::run_daemon_with_socket(socket).await?;
        }
        Some(Commands::Attach { socket: _ }) => {
            grein_tui::run().await?;
        }
    }

    Ok(())
}
