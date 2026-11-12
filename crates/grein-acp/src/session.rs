use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

/// Identifies a thread or execution branch in the DAG.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BranchThreadId(String);

impl BranchThreadId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Level of isolation applied to an ACP session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionIsolation {
    ProcessPerSession,
    SharedProcess,
}

/// Errors related to session management and multiplexing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    InvalidBranchIdentifier(String),
    SessionNotFound(String),
    Internal(String),
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBranchIdentifier(msg) => write!(f, "Invalid branch identifier: {}", msg),
            Self::SessionNotFound(msg) => write!(f, "Session not found: {}", msg),
            Self::Internal(msg) => write!(f, "Internal session error: {}", msg),
        }
    }
}

impl std::error::Error for SessionError {}

/// Handle representing an active ACP session.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    session_id: String,
    isolation: SessionIsolation,
}

impl SessionHandle {
    pub fn new(session_id: impl Into<String>, isolation: SessionIsolation) -> Self {
        Self {
            session_id: session_id.into(),
            isolation,
        }
    }

    pub fn session_id(&self) -> &String {
        &self.session_id
    }

    pub fn isolation(&self) -> SessionIsolation {
        self.isolation
    }
}

/// Multiplexes parallel DAG branch threads to distinct ACP sessions.
pub struct SessionMultiplexer {
    isolation: SessionIsolation,
    sessions: RwLock<HashMap<BranchThreadId, SessionHandle>>,
    counter: AtomicU64,
}

impl SessionMultiplexer {
    pub fn new(isolation: SessionIsolation) -> Self {
        Self {
            isolation,
            sessions: RwLock::new(HashMap::new()),
            counter: AtomicU64::new(1),
        }
    }

    /// Retrieves an existing session for the given branch or creates a new one.
    pub fn get_or_create_session(
        &self,
        branch: &BranchThreadId,
    ) -> Result<SessionHandle, SessionError> {
        if branch.as_str().is_empty() {
            return Err(SessionError::InvalidBranchIdentifier(
                "branch identifier cannot be empty".to_string(),
            ));
        }

        // Fast path: read lock
        {
            let guard = self
                .sessions
                .read()
                .map_err(|e| SessionError::Internal(e.to_string()))?;
            if let Some(session) = guard.get(branch) {
                return Ok(session.clone());
            }
        }

        // Slow path: write lock
        let mut guard = self
            .sessions
            .write()
            .map_err(|e| SessionError::Internal(e.to_string()))?;
        if let Some(session) = guard.get(branch) {
            return Ok(session.clone());
        }

        let next_id = self.counter.fetch_add(1, Ordering::SeqCst);
        let session = SessionHandle::new(format!("acp-session-{}", next_id), self.isolation);
        guard.insert(branch.clone(), session.clone());
        Ok(session)
    }

    /// Terminates an active session associated with the branch thread.
    pub fn terminate_session(&self, branch: &BranchThreadId) -> Result<(), SessionError> {
        let mut guard = self
            .sessions
            .write()
            .map_err(|e| SessionError::Internal(e.to_string()))?;
        guard.remove(branch);
        Ok(())
    }

    /// Returns the number of currently active sessions.
    pub fn active_session_count(&self) -> usize {
        self.sessions.read().map(|guard| guard.len()).unwrap_or(0)
    }
}
