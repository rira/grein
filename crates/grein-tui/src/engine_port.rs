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

/// Structural parent/child metadata for a conversation thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadMeta {
    pub thread_id: String,
    pub parent_thread_id: Option<String>,
    pub inherited_turn_count: usize,
    #[serde(default)]
    pub merged: bool,
}

/// Execution lifecycle status of a live subagent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubagentStatus {
    Running,
    Completed,
    Failed,
}

/// Point-in-time snapshot of a subagent running under a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentSnapshot {
    pub id: String,
    pub role: String,
    pub agent_type: String,
    pub model: Option<String>,
    pub activity: Option<String>,
    pub status: SubagentStatus,
    pub started_at_ms: u64,
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
    BlackboardDeleted {
        key: String,
    },
    ThoughtChunk {
        thread_id: String,
        content: String,
    },
    ThreadCreated {
        thread: HydratedThread,
    },
    ThreadTitleUpdated {
        thread_id: String,
        title: String,
    },
    ThreadMerged {
        thread_id: String,
        target_thread_id: String,
        summary: String,
    },
    ThreadMetadataUpdated {
        meta: Vec<ThreadMeta>,
    },
    ThreadDeleted {
        thread_id: String,
        next_active_thread_id: String,
    },
    AgentMetaUpdated {
        thread_id: String,
        model: Option<String>,
        mode: Option<String>,
    },
    SubagentUpdated {
        thread_id: String,
        subagent: SubagentSnapshot,
    },
    SubagentRemoved {
        thread_id: String,
        subagent_id: String,
    },
    ToolActivityUpdated {
        thread_id: String,
        tool_name: Option<String>,
    },
    ShellOutput {
        thread_id: String,
        command: String,
        stdout: String,
        stderr: String,
        exit_code: i32,
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

    async fn fork_branch(
        &self,
        source_thread_id: &str,
        title: Option<&str>,
    ) -> io::Result<HydratedThread> {
        let id = self
            .fork_thread(source_thread_id, title.unwrap_or(""))
            .await?;
        Ok(HydratedThread {
            id,
            title: title
                .filter(|t| !t.trim().is_empty())
                .unwrap_or("Branch")
                .to_string(),
            turns: Vec::new(),
        })
    }

    async fn cancel_turn(&self, _thread_id: &str) -> io::Result<()> {
        Ok(())
    }

    async fn upsert_blackboard(&self, _key: &str, _value: &str) -> io::Result<()> {
        Ok(())
    }

    async fn delete_blackboard(&self, _key: &str) -> io::Result<()> {
        Ok(())
    }

    async fn run_shell_command(&self, _thread_id: &str, _command: &str) -> io::Result<()> {
        Ok(())
    }

    async fn delete_thread(&self, _thread_id: &str) -> Result<String, String> {
        Err("delete_thread not supported".to_string())
    }

    async fn thread_metadata(&self) -> Result<Vec<ThreadMeta>, String> {
        Ok(Vec::new())
    }

    async fn merge_thread_with_summary(
        &self,
        source_id: &str,
        target_id: &str,
        _summary: Option<&str>,
    ) -> io::Result<()> {
        self.merge_thread(source_id, target_id).await
    }

    async fn create_root(&self, _title: Option<&str>) -> io::Result<HydratedThread> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "create_root not supported",
        ))
    }

    async fn rename_thread(&self, _thread_id: &str, _title: &str) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "rename_thread not supported",
        ))
    }
}
