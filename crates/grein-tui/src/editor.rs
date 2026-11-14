use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorError {
    Io(String),
    ExecutionFailed(String),
    NotFound(String),
}

impl std::fmt::Display for EditorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(msg) => write!(f, "IO error: {msg}"),
            Self::ExecutionFailed(msg) => write!(f, "Editor execution failed: {msg}"),
            Self::NotFound(msg) => write!(f, "Editor not found: {msg}"),
        }
    }
}

impl std::error::Error for EditorError {}

impl From<std::io::Error> for EditorError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

pub trait ExternalEditorRunner {
    fn run_editor(&self, file_path: &Path) -> Result<(), EditorError>;
}

#[derive(Debug, Clone, Default)]
pub struct SystemEditorRunner;

impl ExternalEditorRunner for SystemEditorRunner {
    fn run_editor(&self, file_path: &Path) -> Result<(), EditorError> {
        let editor = std::env::var("EDITOR")
            .or_else(|_| std::env::var("VISUAL"))
            .unwrap_or_else(|_| "nano".to_string());

        let status = std::process::Command::new(&editor)
            .arg(file_path)
            .status()
            .map_err(|e| {
                EditorError::ExecutionFailed(format!("Failed to execute editor {editor}: {e}"))
            })?;

        if status.success() {
            Ok(())
        } else {
            Err(EditorError::ExecutionFailed(format!(
                "Editor exited with status {status}"
            )))
        }
    }
}

pub type EditorPane = SystemEditorRunner;
