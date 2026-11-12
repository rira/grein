use clap::Parser;
use grein_cli::args::{Cli, Command};
use grein_cli::client::connect_to_daemon;
use grein_cli::daemon::{default_socket_path, run_daemon_with_socket};
use grein_cli::standalone::run_standalone;
use tokio::sync::oneshot;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Daemon(args)) => {
            let socket_path = args.socket.unwrap_or_else(default_socket_path);
            let (_shutdown_tx, shutdown_rx) = oneshot::channel();
            run_daemon_with_socket(socket_path, shutdown_rx).await?;
        }
        Some(Command::Attach(args)) => {
            let socket_path = args.socket.unwrap_or_else(default_socket_path);
            let mut client = connect_to_daemon(&socket_path).await?;
            let resp = client.ping().await?;
            println!("{resp}");
        }
        Some(Command::Standalone {}) | None => {
            let (_shutdown_tx, shutdown_rx) = oneshot::channel();
            let _ = run_standalone(shutdown_rx).await;
        }
    }

    Ok(())
}
