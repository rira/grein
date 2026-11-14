pub mod ui;
pub mod views;

pub use ui::focus::PaneFocus;
pub use views::blackboard::{BlackboardState, BlackboardView, DecisionItem};

pub async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    Ok(())
}
