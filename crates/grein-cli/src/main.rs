use clap::{Parser, Subcommand};
use std::path::PathBuf;

pub mod daemon {
    use std::path::Path;

    pub fn run_daemon_with_socket(
        socket_path: &Path,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if socket_path.exists() {
            let _ = std::fs::remove_file(socket_path);
        }
        let listener = std::os::unix::net::UnixListener::bind(socket_path)?;
        for stream in listener.incoming() {
            match stream {
                Ok(_) => {}
                Err(_) => break,
            }
        }
        Ok(())
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "grein",
    version,
    about = "Grein autonomous software engineering CLI"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Command {
    /// Standalone mode (interactive TUI)
    Standalone,
    /// Daemon mode (headless server)
    Daemon {
        /// Unix domain socket path
        #[arg(short, long)]
        socket: Option<PathBuf>,
    },
    /// Attach mode (connect to running daemon)
    Attach {
        /// Unix domain socket path
        #[arg(short, long)]
        socket: Option<PathBuf>,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();

    match cli.command {
        None | Some(Command::Standalone) => {
            println!("Grein Standalone mode");
            Ok(())
        }
        Some(Command::Daemon { socket }) => {
            let socket_path = socket.unwrap_or_else(|| {
                std::env::temp_dir().join(format!("grein-daemon-{}.sock", std::process::id()))
            });
            daemon::run_daemon_with_socket(&socket_path)
        }
        Some(Command::Attach { socket: _ }) => {
            println!("Grein Attach mode");
            Ok(())
        }
    }
}
