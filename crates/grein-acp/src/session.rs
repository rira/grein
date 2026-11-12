use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(String);

impl SessionId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<T: Into<String>> From<T> for SessionId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ThreadId(String);

impl ThreadId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<T: Into<String>> From<T> for ThreadId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl std::fmt::Display for ThreadId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    id: SessionId,
    thread_id: ThreadId,
}

impl Session {
    pub fn new(id: SessionId, thread_id: ThreadId) -> Self {
        Self { id, thread_id }
    }

    pub fn id(&self) -> &SessionId {
        &self.id
    }

    pub fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    SessionNotFound(String),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionNotFound(msg) => write!(f, "Session not found: {msg}"),
        }
    }
}

impl std::error::Error for SessionError {}

#[derive(Debug)]
pub struct SessionManager {
    process_per_session: bool,
    sessions: HashMap<ThreadId, Session>,
    next_session_index: usize,
}

impl SessionManager {
    pub fn new(process_per_session: bool) -> Self {
        Self {
            process_per_session,
            sessions: HashMap::new(),
            next_session_index: 1,
        }
    }

    pub fn process_per_session(&self) -> bool {
        self.process_per_session
    }

    pub fn get_or_create_session(&mut self, thread_id: &ThreadId) -> Result<Session, SessionError> {
        if let Some(session) = self.sessions.get(thread_id) {
            return Ok(session.clone());
        }

        let session_id = SessionId::from(format!("session-{}", self.next_session_index));
        self.next_session_index += 1;

        let session = Session::new(session_id, thread_id.clone());
        self.sessions.insert(thread_id.clone(), session.clone());
        Ok(session)
    }

    pub fn get_session(&self, thread_id: &ThreadId) -> Option<&Session> {
        self.sessions.get(thread_id)
    }

    pub fn active_session_count(&self) -> usize {
        self.sessions.len()
    }
}
