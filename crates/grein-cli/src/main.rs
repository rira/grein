use clap::Parser;
use grein_cli::args::Cli;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    grein_cli::bootstrap::run_cli(Cli::parse()).await
}
