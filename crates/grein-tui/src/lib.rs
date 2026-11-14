pub mod app;

pub use app::{App, FocusedPane, TuiError};

pub async fn run() -> Result<(), TuiError> {
    let mut app = App::new();
    app.run().await
}
