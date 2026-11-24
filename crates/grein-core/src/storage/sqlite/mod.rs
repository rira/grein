mod nodes;
mod threads;

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{params, Connection};
use uuid::Uuid;

use crate::blackboard::BlackboardEntry;
use crate::error::{CoreError, Result};
use crate::model::{
    Anchor, AnchorId, ConversationNode, NodeId, Thread, ThreadId, ThreadKind, ThreadStatus,
};
use crate::port::outbound::SessionRepository;

pub struct SqliteStorage {
    conn: Mutex<Connection>,
    initialized: bool,
}

pub type SqliteDagStorage = SqliteStorage;

pub(super) fn conversion_error<E: std::error::Error + Send + Sync + 'static>(
    idx: usize,
    e: E,
) -> CoreError {
    CoreError::from(rusqlite::Error::FromSqlConversionFailure(
        idx,
        rusqlite::types::Type::Text,
        Box::new(e),
    ))
}

pub(super) fn parse_uuid(idx: usize, s: &str) -> Result<Uuid> {
    Uuid::parse_str(s).map_err(|e| conversion_error(idx, e))
}

pub(super) fn ensure_node_exists(conn: &Connection, node_id: &Uuid) -> Result<()> {
    let exists: i64 = conn.query_row(
        "SELECT count(*) FROM nodes WHERE id = ?1",
        params![node_id.to_string()],
        |row| row.get(0),
    )?;
    if exists == 0 {
        return Err(CoreError::NodeNotFound(node_id.to_string()));
    }
    Ok(())
}

impl SqliteStorage {
    pub fn new(mut conn: Connection) -> Result<Self> {
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

    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::new(conn)
    }

    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::new(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::in_memory()
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    pub(super) fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| CoreError::SyncError("sqlite connection mutex poisoned".to_string()))
    }

    pub fn run_migrations(&self) -> Result<()> {
        let mut conn = self.lock()?;
        Self::apply_migrations(&mut conn)
    }

    fn apply_migrations(conn: &mut Connection) -> Result<()> {
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
                kind TEXT NOT NULL DEFAULT 'agent',
                cwd TEXT,
                custom_name TEXT,
                agent_provider TEXT,
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
            (
                "kind",
                "ALTER TABLE threads ADD COLUMN kind TEXT NOT NULL DEFAULT 'agent'",
            ),
            ("cwd", "ALTER TABLE threads ADD COLUMN cwd TEXT"),
            (
                "custom_name",
                "ALTER TABLE threads ADD COLUMN custom_name TEXT",
            ),
            (
                "agent_provider",
                "ALTER TABLE threads ADD COLUMN agent_provider TEXT",
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

    pub fn table_exists(&self, table_name: &str) -> Result<bool> {
        let conn = self.lock()?;
        let count: i64 = conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
            params![table_name],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn list_tables(&self) -> Result<Vec<String>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type='table'")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let mut tables = Vec::new();
        for r in rows {
            tables.push(r?);
        }
        Ok(tables)
    }
}

impl SessionRepository for SqliteStorage {
    fn insert_anchor(&self, anchor_id: &Uuid, anchor: &Anchor) -> Result<()> {
        SqliteStorage::insert_anchor(self, anchor_id, anchor)
    }

    fn list_anchors(&self) -> Result<Vec<(AnchorId, Anchor)>> {
        SqliteStorage::list_anchors(self)
    }

    fn link_node_anchor(&self, node_id: &Uuid, anchor_id: &Uuid) -> Result<()> {
        SqliteStorage::link_node_anchor(self, node_id, anchor_id)
    }

    fn list_anchors_for_node(&self, node_id: &Uuid) -> Result<Vec<Anchor>> {
        SqliteStorage::list_anchors_for_node(self, node_id)
    }

    fn list_root_nodes(&self) -> Result<Vec<ConversationNode>> {
        SqliteStorage::list_root_nodes(self)
    }

    fn list_recent_user_prompts(&self, limit: usize) -> Result<Vec<String>> {
        SqliteStorage::list_recent_user_prompts(self, limit)
    }

    fn insert_thread(&self, thread: &Thread) -> Result<()> {
        SqliteStorage::insert_thread(self, thread)
    }

    fn create_thread_with_kind(
        &self,
        parent_node_id: NodeId,
        anchor_id: AnchorId,
        title: Option<&str>,
        parent_thread_id: Option<ThreadId>,
        kind: ThreadKind,
        cwd: Option<&str>,
    ) -> Result<ThreadId> {
        SqliteStorage::create_thread_with_kind(
            self,
            parent_node_id,
            anchor_id,
            title,
            parent_thread_id,
            kind,
            cwd,
        )
    }

    fn create_thread_with_parent(
        &self,
        parent_node_id: NodeId,
        anchor_id: AnchorId,
        title: Option<&str>,
        parent_thread_id: Option<ThreadId>,
    ) -> Result<ThreadId> {
        SqliteStorage::create_thread_with_parent(
            self,
            parent_node_id,
            anchor_id,
            title,
            parent_thread_id,
        )
    }

    fn create_thread_with_title(
        &self,
        parent_node_id: NodeId,
        anchor_id: AnchorId,
        title: &str,
    ) -> Result<ThreadId> {
        SqliteStorage::create_thread_with_title(self, parent_node_id, anchor_id, title)
    }

    fn reparent_child_threads(
        &self,
        old_parent_id: &ThreadId,
        new_parent_id: Option<&ThreadId>,
    ) -> Result<usize> {
        SqliteStorage::reparent_child_threads(self, old_parent_id, new_parent_id)
    }

    fn update_thread_title(&self, thread_id: &ThreadId, title: &str) -> Result<()> {
        SqliteStorage::update_thread_title(self, thread_id, title)
    }

    fn update_thread_acp_session_id(
        &self,
        thread_id: &ThreadId,
        session_id: Option<&str>,
    ) -> Result<()> {
        SqliteStorage::update_thread_acp_session_id(self, thread_id, session_id)
    }

    fn mark_thread_merged(&self, thread_id: &ThreadId, merged: bool) -> Result<()> {
        SqliteStorage::mark_thread_merged(self, thread_id, merged)
    }

    fn update_thread_cwd(&self, thread_id: &ThreadId, cwd: Option<&str>) -> Result<()> {
        SqliteStorage::update_thread_cwd(self, thread_id, cwd)
    }

    fn update_thread_custom_name(
        &self,
        thread_id: &ThreadId,
        custom_name: Option<&str>,
    ) -> Result<()> {
        SqliteStorage::update_thread_custom_name(self, thread_id, custom_name)
    }

    fn update_thread_agent_provider(
        &self,
        thread_id: &ThreadId,
        provider: Option<&str>,
    ) -> Result<()> {
        SqliteStorage::update_thread_agent_provider(self, thread_id, provider)
    }

    fn delete_thread(&self, thread_id: &ThreadId) -> Result<bool> {
        SqliteStorage::delete_thread(self, *thread_id)
    }

    fn get_thread_leaf(&self, thread_id: &ThreadId) -> Result<NodeId> {
        SqliteStorage::get_thread_leaf(self, *thread_id)
    }

    fn get_thread_state(&self, thread_id: &ThreadId) -> Result<ThreadStatus> {
        SqliteStorage::get_thread_state(self, *thread_id)
    }

    fn insert_blackboard_entry(&self, entry: &BlackboardEntry) -> Result<()> {
        SqliteStorage::insert_blackboard_entry(self, entry)
    }

    fn get_blackboard_entry(&self, key: &str) -> Result<Option<BlackboardEntry>> {
        SqliteStorage::get_blackboard_entry(self, key)
    }

    fn list_blackboard_entries(&self) -> Result<Vec<BlackboardEntry>> {
        SqliteStorage::list_blackboard_entries(self)
    }

    fn delete_blackboard_entry(&self, key: &str) -> Result<bool> {
        SqliteStorage::delete_blackboard_entry(self, key)
    }
}
