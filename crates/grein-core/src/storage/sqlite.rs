use crate::error::CoreError;
use crate::model::{
    Anchor, AnchorId, AuthorRole, ConversationNode, NodeId, Thread, ThreadId, ThreadStatus,
};
use rusqlite::{params, Connection};
use std::borrow::Borrow;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;
use uuid::Uuid;

pub struct SqliteStorage {
    conn: Mutex<Connection>,
    initialized: bool,
}

impl SqliteStorage {
    pub fn new(mut conn: Connection) -> Result<Self, CoreError> {
        Self::apply_migrations(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            initialized: true,
        })
    }

    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, CoreError> {
        let conn = Connection::open(path)?;
        Self::new(conn)
    }

    pub fn in_memory() -> Result<Self, CoreError> {
        let conn = Connection::open_in_memory()?;
        Self::new(conn)
    }

    pub fn open_in_memory() -> Result<Self, CoreError> {
        Self::in_memory()
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    pub fn run_migrations(&self) -> Result<(), CoreError> {
        let mut conn = self.conn.lock().unwrap();
        Self::apply_migrations(&mut conn)
    }

    fn apply_migrations(conn: &mut Connection) -> Result<(), CoreError> {
        let tx = conn.transaction()?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS nodes (
                id TEXT PRIMARY KEY NOT NULL,
                parent_id TEXT,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                FOREIGN KEY (parent_id) REFERENCES nodes(id) ON DELETE RESTRICT
            );
            CREATE INDEX IF NOT EXISTS idx_nodes_parent_id ON nodes(parent_id);

            CREATE TABLE IF NOT EXISTS threads (
                id TEXT PRIMARY KEY NOT NULL,
                parent_node_id TEXT NOT NULL,
                anchor_id TEXT NOT NULL,
                active_leaf_id TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_threads_parent_node ON threads(parent_node_id);

            CREATE TABLE IF NOT EXISTS anchors (
                id TEXT PRIMARY KEY NOT NULL,
                level INTEGER NOT NULL,
                title TEXT NOT NULL,
                slug TEXT NOT NULL,
                line_number INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS node_anchors (
                node_id TEXT NOT NULL,
                anchor_id TEXT NOT NULL,
                PRIMARY KEY (node_id, anchor_id),
                FOREIGN KEY (node_id) REFERENCES nodes(id) ON DELETE CASCADE,
                FOREIGN KEY (anchor_id) REFERENCES anchors(id) ON DELETE CASCADE
            );",
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn table_exists(&self, table_name: &str) -> Result<bool, CoreError> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
            params![table_name],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn list_tables(&self) -> Result<Vec<String>, CoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type='table'")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let mut tables = Vec::new();
        for r in rows {
            tables.push(r?);
        }
        Ok(tables)
    }

    pub fn insert_anchor(&self, anchor_id: &Uuid, anchor: &Anchor) -> Result<(), CoreError> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        conn.execute(
            "INSERT INTO anchors (id, level, title, slug, line_number, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                anchor_id.to_string(),
                anchor.level,
                anchor.title,
                anchor.slug,
                anchor.line_number as i64,
                now,
            ],
        )?;
        Ok(())
    }

    pub fn insert_node(&self, node: &ConversationNode) -> Result<(), CoreError> {
        let mut conn = self.conn.lock().unwrap();

        if let Some(parent_id) = node.parent_id {
            if *parent_id == *node.id {
                return Err(CoreError::CycleDetected(format!(
                    "node {} cannot be its own parent",
                    node.id
                )));
            }
            let parent_exists: i64 = conn.query_row(
                "SELECT count(*) FROM nodes WHERE id = ?1",
                params![parent_id.to_string()],
                |row| row.get(0),
            )?;
            if parent_exists == 0 {
                return Err(CoreError::NodeNotFound(parent_id.to_string()));
            }
        }

        let role_str = serde_json::to_string(&node.role)?;
        let role_trimmed = role_str.trim_matches('"');
        let parent_str = node.parent_id.map(|p| p.to_string());
        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);

        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO nodes (id, parent_id, role, content, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                node.id.to_string(),
                parent_str,
                role_trimmed,
                node.content,
                now,
            ],
        )?;

        tx.commit()?;
        Ok(())
    }

    pub fn get_node<B: Borrow<Uuid>>(&self, id: B) -> Result<Option<ConversationNode>, CoreError> {
        let id_ref = id.borrow();
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT id, parent_id, role, content FROM nodes WHERE id = ?1")?;
        let mut rows = stmt.query(params![id_ref.to_string()])?;

        if let Some(row) = rows.next()? {
            let id_str: String = row.get(0)?;
            let parent_id_str: Option<String> = row.get(1)?;
            let role_str: String = row.get(2)?;
            let content: String = row.get(3)?;

            let node_id = Uuid::parse_str(&id_str).map_err(|e| {
                CoreError::Database(rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                ))
            })?;
            let parent_id = match parent_id_str {
                Some(s) => Some(NodeId(Uuid::parse_str(&s).map_err(|e| {
                    CoreError::Database(rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    ))
                })?)),
                None => None,
            };
            let role: AuthorRole = serde_json::from_str(&format!("\"{}\"", role_str))?;

            Ok(Some(ConversationNode {
                id: NodeId(node_id),
                parent_id,
                role,
                content,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn get_children<B: Borrow<Uuid>>(
        &self,
        parent_id: B,
    ) -> Result<Vec<ConversationNode>, CoreError> {
        let pid_ref = parent_id.borrow();
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, parent_id, role, content FROM nodes WHERE parent_id = ?1 ORDER BY created_at ASC",
        )?;
        let mut rows = stmt.query(params![pid_ref.to_string()])?;

        let mut children = Vec::new();
        while let Some(row) = rows.next()? {
            let id_str: String = row.get(0)?;
            let parent_id_str: Option<String> = row.get(1)?;
            let role_str: String = row.get(2)?;
            let content: String = row.get(3)?;

            let node_id = Uuid::parse_str(&id_str).map_err(|e| {
                CoreError::Database(rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                ))
            })?;
            let parent_id = match parent_id_str {
                Some(s) => Some(NodeId(Uuid::parse_str(&s).map_err(|e| {
                    CoreError::Database(rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    ))
                })?)),
                None => None,
            };
            let role: AuthorRole = serde_json::from_str(&format!("\"{}\"", role_str))?;

            children.push(ConversationNode {
                id: NodeId(node_id),
                parent_id,
                role,
                content,
            });
        }
        Ok(children)
    }

    pub fn get_trunk_path<B: Borrow<Uuid>>(
        &self,
        leaf_id: B,
    ) -> Result<Vec<ConversationNode>, CoreError> {
        let mut path = Vec::new();
        let mut current_id = *leaf_id.borrow();
        let mut visited = HashSet::new();

        while let Some(node) = self.get_node(current_id)? {
            if !visited.insert(node.id.0) {
                return Err(CoreError::CycleDetected(format!(
                    "cycle detected at node {}",
                    node.id
                )));
            }
            let parent = node.parent_id.map(|p| p.0);
            path.push(node);
            match parent {
                Some(p_id) => current_id = p_id,
                None => break,
            }
        }

        if path.is_empty() {
            return Err(CoreError::NodeNotFound(leaf_id.borrow().to_string()));
        }

        path.reverse();
        Ok(path)
    }

    pub fn create_thread<B1: Borrow<Uuid>, B2: Borrow<Uuid>>(
        &self,
        parent_node_id: B1,
        anchor_id: B2,
    ) -> Result<ThreadId, CoreError> {
        let p_ref = parent_node_id.borrow();
        let a_ref = anchor_id.borrow();
        let conn = self.conn.lock().unwrap();

        let parent_exists: i64 = conn.query_row(
            "SELECT count(*) FROM nodes WHERE id = ?1",
            params![p_ref.to_string()],
            |row| row.get(0),
        )?;
        if parent_exists == 0 {
            return Err(CoreError::NodeNotFound(p_ref.to_string()));
        }

        let thread_id = Uuid::now_v7();
        let status_str = serde_json::to_string(&ThreadStatus::Idle)?;
        let status_trimmed = status_str.trim_matches('"');
        let now = 0i64;

        conn.execute(
            "INSERT INTO threads (id, parent_node_id, anchor_id, active_leaf_id, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                thread_id.to_string(),
                p_ref.to_string(),
                a_ref.to_string(),
                p_ref.to_string(),
                status_trimmed,
                now,
            ],
        )?;

        Ok(ThreadId(thread_id))
    }

    pub fn get_thread<B: Borrow<Uuid>>(&self, thread_id: B) -> Result<Option<Thread>, CoreError> {
        let t_ref = thread_id.borrow();
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, parent_node_id, anchor_id, active_leaf_id, status FROM threads WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![t_ref.to_string()])?;

        if let Some(row) = rows.next()? {
            let id_str: String = row.get(0)?;
            let parent_node_id_str: String = row.get(1)?;
            let anchor_id_str: String = row.get(2)?;
            let active_leaf_id_str: String = row.get(3)?;
            let status_str: String = row.get(4)?;

            let id = Uuid::parse_str(&id_str).map_err(|e| {
                CoreError::Database(rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                ))
            })?;
            let parent_node_id = Uuid::parse_str(&parent_node_id_str).map_err(|e| {
                CoreError::Database(rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                ))
            })?;
            let anchor_id = Uuid::parse_str(&anchor_id_str).map_err(|e| {
                CoreError::Database(rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                ))
            })?;
            let active_leaf_id = Uuid::parse_str(&active_leaf_id_str).map_err(|e| {
                CoreError::Database(rusqlite::Error::FromSqlConversionFailure(
                    3,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                ))
            })?;
            let status: ThreadStatus = serde_json::from_str(&format!("\"{}\"", status_str))?;

            Ok(Some(Thread {
                id: ThreadId(id),
                parent_node_id: NodeId(parent_node_id),
                head_node_id: NodeId(parent_node_id),
                anchor_id: AnchorId(anchor_id),
                active_leaf_id: NodeId(active_leaf_id),
                leaf_node_id: NodeId(active_leaf_id),
                status,
                state: status,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn get_thread_leaf<B: Borrow<Uuid>>(&self, thread_id: B) -> Result<NodeId, CoreError> {
        let id_str = thread_id.borrow().to_string();
        let t = self
            .get_thread(thread_id)?
            .ok_or_else(|| CoreError::ThreadNotFound(id_str))?;
        Ok(t.active_leaf_id)
    }

    pub fn get_active_leaf<B: Borrow<Uuid>>(&self, thread_id: B) -> Result<NodeId, CoreError> {
        self.get_thread_leaf(thread_id)
    }

    pub fn update_thread_leaf<B1: Borrow<Uuid>, B2: Borrow<Uuid>>(
        &self,
        thread_id: B1,
        leaf_id: B2,
    ) -> Result<(), CoreError> {
        let t_ref = thread_id.borrow();
        let l_ref = leaf_id.borrow();
        let conn = self.conn.lock().unwrap();
        let leaf_exists: i64 = conn.query_row(
            "SELECT count(*) FROM nodes WHERE id = ?1",
            params![l_ref.to_string()],
            |row| row.get(0),
        )?;
        if leaf_exists == 0 {
            return Err(CoreError::NodeNotFound(l_ref.to_string()));
        }

        let rows = conn.execute(
            "UPDATE threads SET active_leaf_id = ?1 WHERE id = ?2",
            params![l_ref.to_string(), t_ref.to_string()],
        )?;
        if rows == 0 {
            return Err(CoreError::ThreadNotFound(t_ref.to_string()));
        }
        Ok(())
    }

    pub fn update_active_leaf<B1: Borrow<Uuid>, B2: Borrow<Uuid>>(
        &self,
        thread_id: B1,
        leaf_id: B2,
    ) -> Result<(), CoreError> {
        self.update_thread_leaf(thread_id, leaf_id)
    }

    pub fn update_thread_status<B: Borrow<Uuid>>(
        &self,
        thread_id: B,
        status: ThreadStatus,
    ) -> Result<(), CoreError> {
        let t_ref = thread_id.borrow();
        let mut conn = self.conn.lock().unwrap();
        let status_str = serde_json::to_string(&status)?;
        let status_trimmed = status_str.trim_matches('"');

        let tx = conn.transaction()?;
        let rows = tx.execute(
            "UPDATE threads SET status = ?1 WHERE id = ?2",
            params![status_trimmed, t_ref.to_string()],
        )?;
        if rows == 0 {
            return Err(CoreError::ThreadNotFound(t_ref.to_string()));
        }
        tx.commit()?;
        Ok(())
    }

    pub fn update_thread_state<B: Borrow<Uuid>>(
        &self,
        thread_id: B,
        state: ThreadStatus,
    ) -> Result<(), CoreError> {
        self.update_thread_status(thread_id, state)
    }

    pub fn get_thread_state<B: Borrow<Uuid>>(
        &self,
        thread_id: B,
    ) -> Result<ThreadStatus, CoreError> {
        let id_str = thread_id.borrow().to_string();
        self.get_thread(thread_id)?
            .map(|t| t.status)
            .ok_or_else(|| CoreError::ThreadNotFound(id_str))
    }
}

pub type SqliteDagStorage = SqliteStorage;
