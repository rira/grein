use thiserror::Error;

#[derive(Error, Debug)]
pub enum CoreError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Node not found: {0}")]
    NodeNotFound(String),

    #[error("Thread not found: {0}")]
    ThreadNotFound(String),

    #[error("Cycle detected in DAG: {0}")]
    CycleDetected(String),

    #[error("Dangling parent node: {0}")]
    DanglingParent(String),
}
