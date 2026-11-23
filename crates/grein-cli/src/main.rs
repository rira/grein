use std::io::IsTerminal;
use std::sync::Arc;

use clap::Parser;
use grein_cli::args::{AttachArgs, Cli, Command, DaemonArgs};
use grein_cli::engine::{InProcessEngine, UnixSocketEngineClient};

mod tui_port;

use tui_port::{EngineClientPort, InitialSessionMode};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();
    let initial_mode = if cli.agent {
        InitialSessionMode::Agent
    } else {
        InitialSessionMode::Terminal
    };

    match cli.command {
        None | Some(Command::Standalone {}) => {
            if std::io::stdout().is_terminal() {
                let engine = InProcessEngine::open(&grein_cli::daemon::default_db_path())?;
                grein_tui::run_with_engine(Arc::new(EngineClientPort::new_with_mode(
                    engine,
                    initial_mode,
                )))
                .await?;
            }
        }
        Some(Command::Daemon(DaemonArgs { socket })) => {
            let socket_path = socket.unwrap_or_else(grein_cli::daemon::default_socket_path);
            grein_cli::daemon::run_daemon_with_socket(socket_path).await?;
        }
        Some(Command::Attach(AttachArgs { socket })) => {
            let socket_path = socket.unwrap_or_else(grein_cli::daemon::default_socket_path);
            let transport = UnixSocketEngineClient::connect(&socket_path).await?;
            if std::io::stdout().is_terminal() {
                grein_tui::run_with_engine(Arc::new(EngineClientPort::new_with_mode(
                    transport,
                    initial_mode,
                )))
                .await?;
            }
        }
        Some(Command::Threads(args)) => {
            grein_cli::headless::handle_threads(&args).await?;
        }
        Some(Command::Status(args)) => {
            grein_cli::headless::handle_status(&args).await?;
        }
        Some(Command::Agents(args)) => {
            grein_cli::headless::handle_agents(&args).await?;
        }
        Some(Command::Doctor(args)) => {
            grein_cli::doctor::handle_doctor(&args)?;
        }
        Some(Command::Settings(args)) => {
            grein_cli::settings_cmd::handle_settings(&args)?;
        }
        Some(Command::Theme(args)) => {
            grein_cli::settings_cmd::handle_theme(&args)?;
        }
    }

    Ok(())
}
