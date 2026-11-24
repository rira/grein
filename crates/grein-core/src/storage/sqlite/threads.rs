use std::borrow::Borrow;

use rusqlite::params;
use uuid::Uuid;

use crate::error::{CoreError, Result};
use crate::model::{AnchorId, NodeId, Thread, ThreadId, ThreadKind, ThreadStatus};

use super::{ensure_node_exists, parse_uuid, SqliteStorage};

fn thread_from_row(row: &rusqlite::Row<'_>) -> Result<Thread> {
    let id_str: String = row.get(0)?;
    let parent_node_id_str: String = row.get(1)?;
    let anchor_id_str: String = row.get(2)?;
    let active_leaf_id_str: String = row.get(3)?;
    let status_str: String = row.get(4)?;
    let title: String = row.get(5)?;
    let acp_session_id: Option<String> = row.get(6)?;
    let merged_int: i64 = row.get(7)?;
    let parent_thread_id_str: Option<String> = row.get(8)?;
    let kind_str: String = row.get(9)?;
    let cwd: Option<String> = row.get(10)?;
    let custom_name: Option<String> = row.get(11)?;

    let id = parse_uuid(0, &id_str)?;
    let parent_node_id = parse_uuid(1, &parent_node_id_str)?;
    let anchor_id = parse_uuid(2, &anchor_id_str)?;
    let active_leaf_id = parse_uuid(3, &active_leaf_id_str)?;
    let status: ThreadStatus = serde_json::from_str(&format!("\"{}\"", status_str))?;
    let parent_thread_id = match parent_thread_id_str {
        Some(s) if !s.trim().is_empty() => Some(ThreadId(parse_uuid(8, s.trim())?)),
        _ => None,
    };
    let kind: ThreadKind = kind_str.parse()?;

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
        kind,
        cwd,
        custom_name,
    })
}

impl SqliteStorage {
    pub fn insert_thread(&self, thread: &Thread) -> Result<()> {
        let mut conn = self.lock()?;
        ensure_node_exists(&conn, &thread.parent_node_id.0)?;
        if thread.active_leaf_id != thread.parent_node_id {
            ensure_node_exists(&conn, &thread.active_leaf_id.0)?;
        }

        let status_str = serde_json::to_string(&thread.status)?;
        let status_trimmed = status_str.trim_matches('"');
        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let parent_thread_str = thread.parent_thread_id.map(|id| id.0.to_string());

        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO threads (id, parent_node_id, anchor_id, active_leaf_id, status, created_at, title, acp_session_id, merged, parent_thread_id, kind, cwd, custom_name)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                thread.id.to_string(),
                thread.parent_node_id.to_string(),
                thread.anchor_id.to_string(),
                thread.active_leaf_id.to_string(),
                status_trimmed,
                now,
                thread.title,
                thread.acp_session_id,
                i64::from(thread.merged),
                parent_thread_str,
                thread.kind.as_str(),
                thread.cwd,
                thread.custom_name,
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn create_thread_with_kind(
        &self,
        parent_node_id: NodeId,
        anchor_id: AnchorId,
        title: Option<&str>,
        parent_thread_id: Option<ThreadId>,
        kind: ThreadKind,
        cwd: Option<&str>,
    ) -> Result<ThreadId> {
        let mut thread = Thread::new(
            ThreadId::new_v7(),
            parent_node_id,
            anchor_id,
            parent_node_id,
            ThreadStatus::Idle,
        )
        .with_kind(kind);
        if let Some(t) = title {
            thread.title = t.to_string();
        }
        thread.parent_thread_id = parent_thread_id;
        thread.cwd = cwd.map(ToOwned::to_owned);
        self.insert_thread(&thread)?;
        Ok(thread.id)
    }

    pub fn create_thread_with_parent(
        &self,
        parent_node_id: NodeId,
        anchor_id: AnchorId,
        title: Option<&str>,
        parent_thread_id: Option<ThreadId>,
    ) -> Result<ThreadId> {
        self.create_thread_with_kind(
            parent_node_id,
            anchor_id,
            title,
            parent_thread_id,
            ThreadKind::Agent,
            None,
        )
    }

    pub fn create_thread<B1: Borrow<Uuid>, B2: Borrow<Uuid>>(
        &self,
        parent_node_id: B1,
        anchor_id: B2,
    ) -> Result<ThreadId> {
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
    ) -> Result<ThreadId> {
        self.create_thread_with_parent(parent_node_id, anchor_id, Some(title), None)
    }

    pub fn reparent_child_threads(
        &self,
        old_parent_id: &ThreadId,
        new_parent_id: Option<&ThreadId>,
    ) -> Result<usize> {
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
    ) -> Result<()> {
        let conn = self.lock()?;
        let rows = conn.execute(sql, params![value, thread_id.to_string()])?;
        if rows == 0 {
            return Err(CoreError::ThreadNotFound(thread_id.to_string()));
        }
        Ok(())
    }

    pub fn update_thread_title(&self, thread_id: &ThreadId, title: &str) -> Result<()> {
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
    ) -> Result<()> {
        self.update_thread_field(
            thread_id,
            "UPDATE threads SET acp_session_id = ?1 WHERE id = ?2",
            session_id,
        )
    }

    pub fn mark_thread_merged(&self, thread_id: &ThreadId, merged: bool) -> Result<()> {
        let merged_flag = i64::from(merged);
        self.update_thread_field(
            thread_id,
            "UPDATE threads SET merged = ?1 WHERE id = ?2",
            merged_flag,
        )
    }

    pub fn update_thread_cwd(&self, thread_id: &ThreadId, cwd: Option<&str>) -> Result<()> {
        self.update_thread_field(thread_id, "UPDATE threads SET cwd = ?1 WHERE id = ?2", cwd)
    }

    pub fn update_thread_custom_name(
        &self,
        thread_id: &ThreadId,
        custom_name: Option<&str>,
    ) -> Result<()> {
        self.update_thread_field(
            thread_id,
            "UPDATE threads SET custom_name = ?1 WHERE id = ?2",
            custom_name,
        )
    }

    pub fn get_thread<B: Borrow<Uuid>>(&self, thread_id: B) -> Result<Option<Thread>> {
        let t_ref = thread_id.borrow();
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, parent_node_id, anchor_id, active_leaf_id, status, title, acp_session_id, merged, parent_thread_id, kind, cwd, custom_name
             FROM threads WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![t_ref.to_string()])?;

        match rows.next()? {
            Some(row) => Ok(Some(thread_from_row(row)?)),
            None => Ok(None),
        }
    }

    pub fn list_threads(&self) -> Result<Vec<Thread>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, parent_node_id, anchor_id, active_leaf_id, status, title, acp_session_id, merged, parent_thread_id, kind, cwd, custom_name
             FROM threads ORDER BY created_at ASC, rowid ASC",
        )?;
        let mut rows = stmt.query([])?;
        let mut threads = Vec::new();
        while let Some(row) = rows.next()? {
            threads.push(thread_from_row(row)?);
        }
        Ok(threads)
    }

    pub fn delete_thread<B: Borrow<Uuid>>(&self, thread_id: B) -> Result<bool> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "DELETE FROM threads WHERE id = ?1",
            params![thread_id.borrow().to_string()],
        )?;
        Ok(affected > 0)
    }

    pub fn get_thread_leaf<B: Borrow<Uuid>>(&self, thread_id: B) -> Result<NodeId> {
        let t_ref = thread_id.borrow();
        let t = self
            .get_thread(t_ref)?
            .ok_or_else(|| CoreError::ThreadNotFound(t_ref.to_string()))?;
        Ok(t.active_leaf_id)
    }

    pub fn get_active_leaf<B: Borrow<Uuid>>(&self, thread_id: B) -> Result<NodeId> {
        self.get_thread_leaf(thread_id)
    }

    pub fn update_thread_leaf<B1: Borrow<Uuid>, B2: Borrow<Uuid>>(
        &self,
        thread_id: B1,
        leaf_id: B2,
    ) -> Result<()> {
        let t_ref = thread_id.borrow();
        let l_ref = leaf_id.borrow();
        let conn = self.lock()?;
        ensure_node_exists(&conn, l_ref)?;

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
    ) -> Result<()> {
        self.update_thread_leaf(thread_id, leaf_id)
    }

    pub fn update_thread_status<B: Borrow<Uuid>>(
        &self,
        thread_id: B,
        status: ThreadStatus,
    ) -> Result<()> {
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
    ) -> Result<()> {
        self.update_thread_status(thread_id, state)
    }

    pub fn get_thread_state<B: Borrow<Uuid>>(&self, thread_id: B) -> Result<ThreadStatus> {
        let t_ref = thread_id.borrow();
        self.get_thread(t_ref)?
            .map(|t| t.status)
            .ok_or_else(|| CoreError::ThreadNotFound(t_ref.to_string()))
    }
}
