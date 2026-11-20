use std::io;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// A single hydrated conversation turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HydratedTurn {
    pub id: String,
    pub role: String,
    pub content: String,
}

/// A hydrated thread with its trunk transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HydratedThread {
    pub id: String,
    pub title: String,
    pub turns: Vec<HydratedTurn>,
}

/// A hydrated blackboard entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HydratedBlackboardEntry {
    pub key: String,
    pub value: String,
}

/// Full session snapshot used to seed the TUI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HydratedSession {
    pub active_thread_id: String,
    pub threads: Vec<HydratedThread>,
    pub blackboard: Vec<HydratedBlackboardEntry>,
}

/// Events streamed from the engine to the TUI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TuiEngineEvent {
    StreamChunk {
        thread_id: String,
        content: String,
    },
    TurnCompleted {
        thread_id: String,
    },
    ToolInvoked {
        thread_id: String,
        tool_name: String,
    },
    ToolResult {
        thread_id: String,
        tool_name: String,
        output: String,
    },
    PermissionRequested {
        request_id: String,
        description: String,
    },
    ThreadStatusChanged {
        thread_id: String,
        status: String,
    },
    BlackboardUpdated {
        key: String,
        value: String,
    },
    Error {
        message: String,
    },
}

/// Service provider interface between the TUI and an engine implementation.
#[async_trait::async_trait]
pub trait TuiEnginePort: Send + Sync {
    async fn hydrate_session(&self) -> io::Result<HydratedSession>;
    async fn send_turn(&self, thread_id: &str, prompt: &str) -> io::Result<()>;
    async fn fork_thread(&self, thread_id: &str, anchor_id: &str) -> io::Result<String>;
    async fn merge_thread(&self, source_id: &str, target_id: &str) -> io::Result<()>;
    async fn resolve_permission(&self, request_id: &str, approved: bool) -> io::Result<()>;
    async fn subscribe_events(&self) -> io::Result<mpsc::UnboundedReceiver<TuiEngineEvent>>;
}
