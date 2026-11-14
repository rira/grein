use clap::Parser;
use grein_cli::args::{AttachArgs, Command, DaemonArgs};
use std::io::IsTerminal;

#[derive(Debug, Parser, Clone, PartialEq, Eq)]
#[command(name = "grein", version, about = "Grein autonomous engineering CLI")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();

    match cli.command {
        None | Some(Command::Standalone {}) => {
            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
            let daemon_handle = tokio::spawn(async move {
                let _ = grein_cli::standalone::run_standalone(shutdown_rx).await;
            });
            let res = if std::io::stdout().is_terminal() {
                grein_tui::run().await
            } else {
                Ok(())
            };
            let _ = shutdown_tx.send(());
            let _ = daemon_handle.await;
            res?;
        }
        Some(Command::Daemon(DaemonArgs { socket })) => {
            let socket_path = socket.unwrap_or_else(grein_cli::daemon::default_socket_path);
            grein_cli::daemon::run_daemon_with_socket(socket_path).await?;
        }
        Some(Command::Attach(AttachArgs { socket })) => {
            let socket_path = socket.unwrap_or_else(grein_cli::daemon::default_socket_path);
            let _client = grein_cli::client::connect_to_daemon(&socket_path).await?;
            if std::io::stdout().is_terminal() {
                grein_tui::run().await?;
            }
        }
    }

    Ok(())
}
