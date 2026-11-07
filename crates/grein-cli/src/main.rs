use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "grein", version, about = "Agentic TUI Manager with DAG Context Trees")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run the background agent engine daemon
    Daemon,
    /// Attach to an existing running daemon
    Attach,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Daemon) => {
            println!("Starting grein daemon...");
        }
        Some(Commands::Attach) => {
            println!("Attaching to grein daemon...");
        }
        None => {
            println!("Launching grein in standalone mode...");
        }
    }

    Ok(())
}
