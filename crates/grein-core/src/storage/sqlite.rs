use crate::blackboard::{BlackboardEntry, BlackboardEntryKind};
use crate::error::CoreError;
use crate::model::{
    Anchor, AnchorId, AuthorRole, BlackboardEntryId, ConversationNode, NodeId, Thread, ThreadId,
    ThreadStatus,
};
use rusqlite::{params, Connection};
use std::borrow::Borrow;
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use uuid::Uuid;

pub struct SqliteStorage {
    conn: Mutex<Connection>,
    initialized: bool,
}

fn conversion_error<E: std::error::Error + Send + Sync + 'static>(idx: usize, e: E) -> CoreError {
    CoreError::Database(rusqlite::Error::FromSqlConversionFailure(
        idx,
        rusqlite::types::Type::Text,
        Box::new(e),
    ))
}

fn parse_uuid(idx: usize, s: &str) -> Result<Uuid, CoreError> {
    Uuid::parse_str(s).map_err(|e| conversion_error(idx, e))
}

fn node_from_row(row: &rusqlite::Row<'_>) -> Result<ConversationNode, CoreError> {
    let id_str: String = row.get(0)?;
    let parent_id_str: Option<String> = row.get(1)?;
    let role_str: String = row.get(2)?;
    let content: String = row.get(3)?;

    let node_id = parse_uuid(0, &id_str)?;
    let parent_id = match parent_id_str {
        Some(s) => Some(NodeId(parse_uuid(1, &s)?)),
        None => None,
    };
    let role: AuthorRole = serde_json::from_str(&format!("\"{}\"", role_str))?;

    Ok(ConversationNode {
        id: NodeId(node_id),
        parent_id,
        role,
        content,
    })
}

fn thread_from_row(row: &rusqlite::Row<'_>) -> Result<Thread, CoreError> {
    let id_str: String = row.get(0)?;
    let parent_node_id_str: String = row.get(1)?;
    let anchor_id_str: String = row.get(2)?;
    let active_leaf_id_str: String = row.get(3)?;
    let status_str: String = row.get(4)?;
    let title: String = row.get(5)?;
    let acp_session_id: Option<String> = row.get(6)?;
    let merged_int: i64 = row.get(7)?;
    let parent_thread_id_str: Option<String> = row.get(8)?;

    let id = parse_uuid(0, &id_str)?;
    let parent_node_id = parse_uuid(1, &parent_node_id_str)?;
    let anchor_id = parse_uuid(2, &anchor_id_str)?;
    let active_leaf_id = parse_uuid(3, &active_leaf_id_str)?;
    let status: ThreadStatus = serde_json::from_str(&format!("\"{}\"", status_str))?;
    let parent_thread_id = match parent_thread_id_str {
        Some(s) if !s.trim().is_empty() => Some(ThreadId(parse_uuid(8, s.trim())?)),
        _ => None,
    };

    Ok(Thread {
        id: ThreadId(id),
        parent_node_id: NodeId(parent_node_id),
        head_node_id: NodeId(parent_node_id),
        anchor_id: AnchorId(anchor_id),
        active_leaf_id: NodeId(active_leaf_id),
        leaf_node_id: NodeId(active_leaf_id),
        status,
        state: status,
        title,
        acp_session_id,
        merged: merged_int != 0,
        parent_thread_id,
    })
}

fn anchor_from_row(row: &rusqlite::Row<'_>) -> Result<Anchor, rusqlite::Error> {
    let level: u8 = row.get(0)?;
    let title: String = row.get(1)?;
    let slug: String = row.get(2)?;
    let line_number: i64 = row.get(3)?;
    Ok(Anchor {
        level,
        title,
        slug,
        line_number: usize::try_from(line_number).unwrap_or(0),
    })
}

impl SqliteStorage {
    pub fn new(mut conn: Connection) -> Result<Self, CoreError> {
        // Pragmas must be applied outside of a transaction and before migrations.
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA busy_timeout = 5000;",
        )?;
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

    fn lock(&self) -> Result<MutexGuard<'_, Connection>, CoreError> {
        self.conn
            .lock()
            .map_err(|_| CoreError::SyncError("sqlite connection mutex poisoned".to_string()))
    }

    pub fn run_migrations(&self) -> Result<(), CoreError> {
        let mut conn = self.lock()?;
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
                created_at INTEGER NOT NULL,
                title TEXT NOT NULL DEFAULT '',
                acp_session_id TEXT,
                merged INTEGER NOT NULL DEFAULT 0,
                parent_thread_id TEXT,
                FOREIGN KEY (parent_node_id) REFERENCES nodes(id) ON DELETE RESTRICT,
                FOREIGN KEY (active_leaf_id) REFERENCES nodes(id) ON DELETE RESTRICT
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
            );

            CREATE TABLE IF NOT EXISTS blackboard_entries (
                id TEXT PRIMARY KEY NOT NULL,
                key TEXT NOT NULL,
                value TEXT NOT NULL,
                kind TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_blackboard_entries_key_unique ON blackboard_entries(key);
            CREATE INDEX IF NOT EXISTS idx_blackboard_entries_kind ON blackboard_entries(kind);",
        )?;

        let existing_columns = {
            let mut stmt = tx.prepare("PRAGMA table_info(threads)")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
            let mut columns = HashSet::new();
            for col in rows {
                columns.insert(col?);
            }
            columns
        };

        let thread_column_migrations = [
            (
                "title",
                "ALTER TABLE threads ADD COLUMN title TEXT NOT NULL DEFAULT ''",
            ),
            (
                "acp_session_id",
                "ALTER TABLE threads ADD COLUMN acp_session_id TEXT",
            ),
            (
                "merged",
                "ALTER TABLE threads ADD COLUMN merged INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "parent_thread_id",
                "ALTER TABLE threads ADD COLUMN parent_thread_id TEXT",
            ),
        ];
        for (column_name, ddl) in thread_column_migrations {
            if !existing_columns.contains(column_name) {
                tx.execute(ddl, [])?;
            }
        }

        tx.commit()?;
        Ok(())
    }

    pub fn table_exists(&self, table_name: &str) -> Result<bool, CoreError> {
        let conn = self.lock()?;
        let count: i64 = conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
            params![table_name],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn list_tables(&self) -> Result<Vec<String>, CoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type='table'")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let mut tables = Vec::new();
        for r in rows {
            tables.push(r?);
        }
        Ok(tables)
    }

    pub fn insert_anchor(&self, anchor_id: &Uuid, anchor: &Anchor) -> Result<(), CoreError> {
        let conn = self.lock()?;
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

    pub fn get_anchor<B: Borrow<Uuid>>(&self, anchor_id: B) -> Result<Option<Anchor>, CoreError> {
        let conn = self.lock()?;
        let mut stmt =
            conn.prepare("SELECT level, title, slug, line_number FROM anchors WHERE id = ?1")?;
        let mut rows = stmt.query(params![anchor_id.borrow().to_string()])?;
        match rows.next()? {
            Some(row) => Ok(Some(anchor_from_row(row)?)),
            None => Ok(None),
        }
    }

    pub fn list_anchors(&self) -> Result<Vec<(AnchorId, Anchor)>, CoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, level, title, slug, line_number
             FROM anchors
             ORDER BY created_at ASC, rowid ASC",
        )?;
        let mut rows = stmt.query([])?;
        let mut anchors = Vec::new();
        while let Some(row) = rows.next()? {
            let id_str: String = row.get(0)?;
            let anchor_id = AnchorId(parse_uuid(0, &id_str)?);
            let level: u8 = row.get(1)?;
            let title: String = row.get(2)?;
            let slug: String = row.get(3)?;
            let line_number: i64 = row.get(4)?;
            anchors.push((
                anchor_id,
                Anchor {
                    level,
                    title,
                    slug,
                    line_number: usize::try_from(line_number).unwrap_or(0),
                },
            ));
        }
        Ok(anchors)
    }

    pub fn link_node_anchor<B1: Borrow<Uuid>, B2: Borrow<Uuid>>(
        &self,
        node_id: B1,
        anchor_id: B2,
    ) -> Result<(), CoreError> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT OR IGNORE INTO node_anchors (node_id, anchor_id) VALUES (?1, ?2)",
            params![node_id.borrow().to_string(), anchor_id.borrow().to_string()],
        )?;
        Ok(())
    }

    pub fn list_anchors_for_node<B: Borrow<Uuid>>(
        &self,
        node_id: B,
    ) -> Result<Vec<Anchor>, CoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT a.level, a.title, a.slug, a.line_number
             FROM anchors a
             JOIN node_anchors na ON na.anchor_id = a.id
             WHERE na.node_id = ?1
             ORDER BY a.line_number ASC, a.rowid ASC",
        )?;
        let rows = stmt.query_map(params![node_id.borrow().to_string()], anchor_from_row)?;
        let mut anchors = Vec::new();
        for r in rows {
            anchors.push(r?);
        }
        Ok(anchors)
    }

    pub fn insert_node(&self, node: &ConversationNode) -> Result<(), CoreError> {
        let mut conn = self.lock()?;

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
        let conn = self.lock()?;
        let mut stmt =
            conn.prepare("SELECT id, parent_id, role, content FROM nodes WHERE id = ?1")?;
        let mut rows = stmt.query(params![id_ref.to_string()])?;

        match rows.next()? {
            Some(row) => Ok(Some(node_from_row(row)?)),
            None => Ok(None),
        }
    }

    pub fn get_children<B: Borrow<Uuid>>(
        &self,
        parent_id: B,
    ) -> Result<Vec<ConversationNode>, CoreError> {
        let pid_ref = parent_id.borrow();
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, parent_id, role, content FROM nodes WHERE parent_id = ?1 ORDER BY created_at ASC, rowid ASC",
        )?;
        let mut rows = stmt.query(params![pid_ref.to_string()])?;

        let mut children = Vec::new();
        while let Some(row) = rows.next()? {
            children.push(node_from_row(row)?);
        }
        Ok(children)
    }

    pub fn list_nodes(&self) -> Result<Vec<ConversationNode>, CoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, parent_id, role, content FROM nodes ORDER BY created_at ASC, rowid ASC",
        )?;
        let mut rows = stmt.query([])?;
        let mut nodes = Vec::new();
        while let Some(row) = rows.next()? {
            nodes.push(node_from_row(row)?);
        }
        Ok(nodes)
    }

    pub fn list_root_nodes(&self) -> Result<Vec<ConversationNode>, CoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, parent_id, role, content FROM nodes WHERE parent_id IS NULL ORDER BY created_at ASC, rowid ASC",
        )?;
        let mut rows = stmt.query([])?;
        let mut nodes = Vec::new();
        while let Some(row) = rows.next()? {
            nodes.push(node_from_row(row)?);
        }
        Ok(nodes)
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

    fn insert_thread_row(
        &self,
        parent_node_id: &Uuid,
        anchor_id: &Uuid,
        title: &str,
        parent_thread_id: Option<ThreadId>,
    ) -> Result<ThreadId, CoreError> {
        let conn = self.lock()?;

        let parent_exists: i64 = conn.query_row(
            "SELECT count(*) FROM nodes WHERE id = ?1",
            params![parent_node_id.to_string()],
            |row| row.get(0),
        )?;
        if parent_exists == 0 {
            return Err(CoreError::NodeNotFound(parent_node_id.to_string()));
        }

        let thread_id = Uuid::now_v7();
        let status_str = serde_json::to_string(&ThreadStatus::Idle)?;
        let status_trimmed = status_str.trim_matches('"');
        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let parent_thread_str = parent_thread_id.map(|id| id.0.to_string());

        conn.execute(
            "INSERT INTO threads (id, parent_node_id, anchor_id, active_leaf_id, status, created_at, title, acp_session_id, merged, parent_thread_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, 0, ?8)",
            params![
                thread_id.to_string(),
                parent_node_id.to_string(),
                anchor_id.to_string(),
                parent_node_id.to_string(),
                status_trimmed,
                now,
                title,
                parent_thread_str,
            ],
        )?;

        Ok(ThreadId(thread_id))
    }

    pub fn create_thread_with_parent(
        &self,
        parent_node_id: NodeId,
        anchor_id: AnchorId,
        title: Option<&str>,
        parent_thread_id: Option<ThreadId>,
    ) -> Result<ThreadId, CoreError> {
        self.insert_thread_row(
            &parent_node_id.0,
            &anchor_id.0,
            title.unwrap_or(""),
            parent_thread_id,
        )
    }

    pub fn create_thread<B1: Borrow<Uuid>, B2: Borrow<Uuid>>(
        &self,
        parent_node_id: B1,
        anchor_id: B2,
    ) -> Result<ThreadId, CoreError> {
        self.create_thread_with_parent(
            NodeId(*parent_node_id.borrow()),
            AnchorId(*anchor_id.borrow()),
            None,
            None,
        )
    }

    pub fn create_thread_with_title(
        &self,
        parent_node_id: NodeId,
        anchor_id: AnchorId,
        title: &str,
    ) -> Result<ThreadId, CoreError> {
        self.create_thread_with_parent(parent_node_id, anchor_id, Some(title), None)
    }

    pub fn reparent_child_threads(
        &self,
        old_parent_id: &ThreadId,
        new_parent_id: Option<&ThreadId>,
    ) -> Result<usize, CoreError> {
        let conn = self.lock()?;
        let new_parent_str = new_parent_id.map(|id| id.0.to_string());
        let affected = conn.execute(
            "UPDATE threads SET parent_thread_id = ?1 WHERE parent_thread_id = ?2",
            params![new_parent_str, old_parent_id.0.to_string()],
        )?;
        Ok(affected)
    }

    fn update_thread_field<P: rusqlite::ToSql>(
        &self,
        thread_id: &ThreadId,
        sql: &str,
        value: P,
    ) -> Result<(), CoreError> {
        let conn = self.lock()?;
        let rows = conn.execute(sql, params![value, thread_id.to_string()])?;
        if rows == 0 {
            return Err(CoreError::ThreadNotFound(thread_id.to_string()));
        }
        Ok(())
    }

    pub fn update_thread_title(&self, thread_id: &ThreadId, title: &str) -> Result<(), CoreError> {
        self.update_thread_field(
            thread_id,
            "UPDATE threads SET title = ?1 WHERE id = ?2",
            title,
        )
    }

    pub fn update_thread_acp_session_id(
        &self,
        thread_id: &ThreadId,
        session_id: Option<&str>,
    ) -> Result<(), CoreError> {
        self.update_thread_field(
            thread_id,
            "UPDATE threads SET acp_session_id = ?1 WHERE id = ?2",
            session_id,
        )
    }

    pub fn mark_thread_merged(&self, thread_id: &ThreadId, merged: bool) -> Result<(), CoreError> {
        let merged_flag = i64::from(merged);
        self.update_thread_field(
            thread_id,
            "UPDATE threads SET merged = ?1 WHERE id = ?2",
            merged_flag,
        )
    }

    pub fn get_thread<B: Borrow<Uuid>>(&self, thread_id: B) -> Result<Option<Thread>, CoreError> {
        let t_ref = thread_id.borrow();
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, parent_node_id, anchor_id, active_leaf_id, status, title, acp_session_id, merged, parent_thread_id
             FROM threads WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![t_ref.to_string()])?;

        match rows.next()? {
            Some(row) => Ok(Some(thread_from_row(row)?)),
            None => Ok(None),
        }
    }

    pub fn list_threads(&self) -> Result<Vec<Thread>, CoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, parent_node_id, anchor_id, active_leaf_id, status, title, acp_session_id, merged, parent_thread_id
             FROM threads ORDER BY created_at ASC, rowid ASC",
        )?;
        let mut rows = stmt.query([])?;
        let mut threads = Vec::new();
        while let Some(row) = rows.next()? {
            threads.push(thread_from_row(row)?);
        }
        Ok(threads)
    }

    pub fn delete_thread<B: Borrow<Uuid>>(&self, thread_id: B) -> Result<bool, CoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "DELETE FROM threads WHERE id = ?1",
            params![thread_id.borrow().to_string()],
        )?;
        Ok(affected > 0)
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
        let conn = self.lock()?;
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
        let mut conn = self.lock()?;
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

    pub fn insert_blackboard_entry(&self, entry: &BlackboardEntry) -> Result<(), CoreError> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO blackboard_entries (id, key, value, kind, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(key) DO UPDATE SET
                id = excluded.id,
                value = excluded.value,
                kind = excluded.kind,
                created_at = excluded.created_at",
            params![
                entry.id.to_string(),
                entry.key,
                entry.value,
                entry.kind.as_str(),
                entry.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_blackboard_entry(&self, key: &str) -> Result<Option<BlackboardEntry>, CoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, key, value, kind, created_at FROM blackboard_entries WHERE key = ?1",
        )?;
        let mut rows = stmt.query(params![key])?;

        if let Some(row) = rows.next()? {
            let id_str: String = row.get(0)?;
            let key_str: String = row.get(1)?;
            let value_str: String = row.get(2)?;
            let kind_str: String = row.get(3)?;
            let created_at: i64 = row.get(4)?;

            let id = BlackboardEntryId(parse_uuid(0, &id_str)?);
            let kind = kind_str.parse::<BlackboardEntryKind>()?;

            Ok(Some(BlackboardEntry {
                id,
                key: key_str,
                value: value_str,
                kind,
                created_at,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn list_blackboard_entries(&self) -> Result<Vec<BlackboardEntry>, CoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, key, value, kind, created_at FROM blackboard_entries ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let id_str: String = row.get(0)?;
            let key_str: String = row.get(1)?;
            let value_str: String = row.get(2)?;
            let kind_str: String = row.get(3)?;
            let created_at: i64 = row.get(4)?;
            Ok((id_str, key_str, value_str, kind_str, created_at))
        })?;

        let mut entries = Vec::new();
        for r in rows {
            let (id_str, key_str, value_str, kind_str, created_at) = r?;
            let id = BlackboardEntryId(parse_uuid(0, &id_str)?);
            let kind = kind_str.parse::<BlackboardEntryKind>()?;
            entries.push(BlackboardEntry {
                id,
                key: key_str,
                value: value_str,
                kind,
                created_at,
            });
        }
        Ok(entries)
    }

    pub fn delete_blackboard_entry(&self, key: &str) -> Result<bool, CoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "DELETE FROM blackboard_entries WHERE key = ?1",
            params![key],
        )?;
        Ok(affected > 0)
    }
}

pub type SqliteDagStorage = SqliteStorage;
