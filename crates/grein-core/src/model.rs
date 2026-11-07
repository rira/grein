use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorRole {
    User,
    Assistant,
    System,
    ToolResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Anchor {
    pub level: u8,
    pub title: String,
    pub slug: String,
    pub line_number: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationNode {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub role: AuthorRole,
    pub content: String,
    pub anchors: Vec<Anchor>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatus {
    Streaming,
    Executing,
    PermissionRequired,
    AwaitingInput,
    Idle,
    Error,
}
