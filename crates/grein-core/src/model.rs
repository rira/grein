use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! define_id {
    ($name:ident) => {
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Default,
            Serialize,
            Deserialize,
        )]
        #[repr(transparent)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }
            pub fn new_v7() -> Self {
                Self(Uuid::now_v7())
            }
            pub fn now_v7() -> Self {
                Self(Uuid::now_v7())
            }
        }

        impl std::ops::Deref for $name {
            type Target = Uuid;
            fn deref(&self) -> &Uuid {
                &self.0
            }
        }

        impl From<Uuid> for $name {
            fn from(u: Uuid) -> Self {
                Self(u)
            }
        }

        impl From<$name> for Uuid {
            fn from(id: $name) -> Self {
                id.0
            }
        }

        impl std::borrow::Borrow<Uuid> for $name {
            fn borrow(&self) -> &Uuid {
                &self.0
            }
        }

        impl PartialEq<Uuid> for $name {
            fn eq(&self, other: &Uuid) -> bool {
                &self.0 == other
            }
        }

        impl PartialEq<$name> for Uuid {
            fn eq(&self, other: &$name) -> bool {
                self == &other.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

define_id!(NodeId);
define_id!(ThreadId);
define_id!(AnchorId);
define_id!(BlackboardEntryId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AuthorRole {
    #[default]
    User,
    Assistant,
    System,
    ToolResult,
}

impl AuthorRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::System => "system",
            Self::ToolResult => "tool_result",
        }
    }
}

impl std::fmt::Display for AuthorRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    pub level: u8,
    pub title: String,
    pub slug: String,
    pub line_number: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationNode {
    pub id: NodeId,
    pub parent_id: Option<NodeId>,
    pub role: AuthorRole,
    pub content: String,
}

impl ConversationNode {
    pub fn new(
        id: impl Into<NodeId>,
        parent_id: Option<impl Into<NodeId>>,
        role: AuthorRole,
        content: String,
    ) -> Self {
        Self {
            id: id.into(),
            parent_id: parent_id.map(Into::into),
            role,
            content,
        }
    }

    pub fn new_root(role: AuthorRole, content: impl Into<String>) -> Self {
        Self::new(NodeId::new(), None::<NodeId>, role, content.into())
    }

    pub fn id(&self) -> NodeId {
        self.id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatus {
    Streaming,
    Executing,
    PermissionRequired,
    AwaitingInput,
    #[default]
    Idle,
    Error,
}

pub type ThreadLifecycleState = ThreadStatus;
pub type ThreadState = ThreadStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadKind {
    #[default]
    Agent,
    Terminal,
}

impl ThreadKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Terminal => "terminal",
        }
    }
}

impl std::fmt::Display for ThreadKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ThreadKind {
    type Err = crate::error::CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "agent" => Ok(Self::Agent),
            "terminal" => Ok(Self::Terminal),
            other => Err(crate::error::CoreError::Serialization(
                serde::de::Error::custom(format!("Unknown thread kind: {other}")),
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    pub id: ThreadId,
    pub parent_node_id: NodeId,
    pub head_node_id: NodeId,
    pub anchor_id: AnchorId,
    pub active_leaf_id: NodeId,
    pub leaf_node_id: NodeId,
    pub status: ThreadStatus,
    pub state: ThreadLifecycleState,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub acp_session_id: Option<String>,
    #[serde(default)]
    pub merged: bool,
    #[serde(default)]
    pub parent_thread_id: Option<ThreadId>,
    #[serde(default)]
    pub kind: ThreadKind,
    #[serde(default)]
    pub cwd: Option<String>,
}

impl Thread {
    pub fn new(
        id: impl Into<ThreadId>,
        parent_node_id: impl Into<NodeId>,
        anchor_id: impl Into<AnchorId>,
        active_leaf_id: impl Into<NodeId>,
        status: ThreadStatus,
    ) -> Self {
        let p = parent_node_id.into();
        let a = active_leaf_id.into();
        Self {
            id: id.into(),
            parent_node_id: p,
            head_node_id: p,
            anchor_id: anchor_id.into(),
            active_leaf_id: a,
            leaf_node_id: a,
            status,
            state: status,
            title: String::new(),
            acp_session_id: None,
            merged: false,
            parent_thread_id: None,
            kind: ThreadKind::default(),
            cwd: None,
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn with_kind(mut self, kind: ThreadKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn with_cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn id(&self) -> ThreadId {
        self.id
    }

    pub fn parent_node_id(&self) -> NodeId {
        self.parent_node_id
    }

    pub fn head_node_id(&self) -> NodeId {
        self.head_node_id
    }

    pub fn anchor_id(&self) -> AnchorId {
        self.anchor_id
    }

    pub fn active_leaf_id(&self) -> NodeId {
        self.active_leaf_id
    }

    pub fn leaf_node_id(&self) -> NodeId {
        self.leaf_node_id
    }

    pub fn status(&self) -> ThreadStatus {
        self.status
    }

    pub fn state(&self) -> ThreadStatus {
        self.state
    }

    pub fn kind(&self) -> ThreadKind {
        self.kind
    }

    pub fn cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }
}
