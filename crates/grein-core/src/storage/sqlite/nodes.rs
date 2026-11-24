use std::borrow::Borrow;
use std::collections::HashSet;

use rusqlite::params;
use uuid::Uuid;

use crate::blackboard::{BlackboardEntry, BlackboardEntryKind};
use crate::error::{CoreError, Result};
use crate::model::{Anchor, AnchorId, AuthorRole, BlackboardEntryId, ConversationNode, NodeId};

use super::{ensure_node_exists, parse_uuid, SqliteStorage};

fn node_from_row(row: &rusqlite::Row<'_>) -> Result<ConversationNode> {
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

fn anchor_from_row(row: &rusqlite::Row<'_>) -> std::result::Result<Anchor, rusqlite::Error> {
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
    pub fn insert_anchor(&self, anchor_id: &Uuid, anchor: &Anchor) -> Result<()> {
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

    pub fn get_anchor<B: Borrow<Uuid>>(&self, anchor_id: B) -> Result<Option<Anchor>> {
        let conn = self.lock()?;
        let mut stmt =
            conn.prepare("SELECT level, title, slug, line_number FROM anchors WHERE id = ?1")?;
        let mut rows = stmt.query(params![anchor_id.borrow().to_string()])?;
        match rows.next()? {
            Some(row) => Ok(Some(anchor_from_row(row)?)),
            None => Ok(None),
        }
    }

    pub fn list_anchors(&self) -> Result<Vec<(AnchorId, Anchor)>> {
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
    ) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT OR IGNORE INTO node_anchors (node_id, anchor_id) VALUES (?1, ?2)",
            params![node_id.borrow().to_string(), anchor_id.borrow().to_string()],
        )?;
        Ok(())
    }

    pub fn list_anchors_for_node<B: Borrow<Uuid>>(&self, node_id: B) -> Result<Vec<Anchor>> {
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

    pub fn insert_node(&self, node: &ConversationNode) -> Result<()> {
        let mut conn = self.lock()?;

        if let Some(parent_id) = node.parent_id {
            if *parent_id == *node.id {
                return Err(CoreError::CycleDetected(format!(
                    "node {} cannot be its own parent",
                    node.id
                )));
            }
            ensure_node_exists(&conn, &parent_id.0)?;
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

    pub fn get_node<B: Borrow<Uuid>>(&self, id: B) -> Result<Option<ConversationNode>> {
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

    pub fn get_children<B: Borrow<Uuid>>(&self, parent_id: B) -> Result<Vec<ConversationNode>> {
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

    pub fn list_nodes(&self) -> Result<Vec<ConversationNode>> {
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

    pub fn list_root_nodes(&self) -> Result<Vec<ConversationNode>> {
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

    pub fn list_recent_user_prompts(&self, limit: usize) -> Result<Vec<String>> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let conn = self.lock()?;
        let mut stmt =
            conn.prepare("SELECT content FROM nodes WHERE role = ?1 ORDER BY rowid DESC")?;
        let mut rows = stmt.query(params![AuthorRole::User.as_str()])?;

        let initial_cap = limit.min(256);
        let mut prompts = Vec::with_capacity(initial_cap);
        let mut seen = HashSet::with_capacity(initial_cap);
        while let Some(row) = rows.next()? {
            let content: String = row.get(0)?;
            let trimmed = content.trim();
            if trimmed.is_empty() || seen.contains(trimmed) {
                continue;
            }
            let prompt = trimmed.to_string();
            seen.insert(prompt.clone());
            prompts.push(prompt);
            if prompts.len() >= limit {
                break;
            }
        }
        Ok(prompts)
    }

    pub fn get_trunk_path<B: Borrow<Uuid>>(&self, leaf_id: B) -> Result<Vec<ConversationNode>> {
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

    pub fn insert_blackboard_entry(&self, entry: &BlackboardEntry) -> Result<()> {
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

    pub fn get_blackboard_entry(&self, key: &str) -> Result<Option<BlackboardEntry>> {
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

    pub fn list_blackboard_entries(&self) -> Result<Vec<BlackboardEntry>> {
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

    pub fn delete_blackboard_entry(&self, key: &str) -> Result<bool> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "DELETE FROM blackboard_entries WHERE key = ?1",
            params![key],
        )?;
        Ok(affected > 0)
    }
}
