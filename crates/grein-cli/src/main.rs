use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "grein", about = "Grein coding engine")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
enum Command {
    Standalone,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Standalone) | None => {
            grein_tui::run().await?;
        }
    }

    Ok(())
}
