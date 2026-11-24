use std::path::Path;

use grein_core::blackboard::{BlackboardEntry, BlackboardEntryKind};
use grein_core::model::{
    AnchorId, AuthorRole, BlackboardEntryId, ConversationNode, NodeId, ThreadId,
};
use grein_core::storage::sqlite::SqliteStorage;
use grein_tui::app::{normalize_custom_name, App, ThreadItem};
use grein_tui::runner::{collect_turns, render_node, sync_thread_blocks_md};
use grein_tui::transcript::{ThreadTranscript, TranscriptBlock};

pub trait AppSqlitePersistenceExt {
    fn persist_state_to_sqlite(&self, db_path: &Path);
    fn load_state_from_sqlite(&mut self, db_path: &Path);
}

impl AppSqlitePersistenceExt for App {
    fn persist_state_to_sqlite(&self, db_path: &Path) {
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let Ok(storage) = SqliteStorage::open(db_path) else {
            return;
        };
        let turns = collect_turns(&self.active_transcript_md);
        if turns.is_empty() {
            return;
        }

        let thread_id = self
            .threads
            .first()
            .and_then(|t| t.id.parse().ok().map(ThreadId))
            .or_else(|| {
                storage
                    .get_blackboard_entry("active_thread_id")
                    .ok()
                    .flatten()
                    .and_then(|e| e.value.parse().ok().map(ThreadId))
            });

        let existing_thread = thread_id.and_then(|tid| storage.get_thread(tid).ok().flatten());

        let existing_trunk = existing_thread
            .as_ref()
            .and_then(|t| storage.get_trunk_path(t.active_leaf_id).ok())
            .unwrap_or_default();

        let common_len = existing_trunk
            .iter()
            .zip(turns.iter())
            .take_while(|(node, (role, content))| node.role == *role && node.content == *content)
            .count();

        let mut parent_id = if common_len > 0 {
            existing_trunk.get(common_len - 1).map(|n| n.id)
        } else {
            None
        };
        let mut root_id = if common_len > 0 {
            existing_trunk.first().map(|n| n.id)
        } else {
            None
        };
        let mut last_node_id = parent_id;

        for (role, content) in turns.into_iter().skip(common_len) {
            let node_id = NodeId::now_v7();
            let node = ConversationNode::new(node_id, parent_id, role, content);
            if storage.insert_node(&node).is_ok() {
                if root_id.is_none() {
                    root_id = Some(node_id);
                }
                parent_id = Some(node_id);
                last_node_id = Some(node_id);
            }
        }

        let (Some(r_id), Some(leaf_id)) = (root_id, last_node_id) else {
            return;
        };

        let real_tid = if let Some(existing) = existing_thread {
            let _ = storage.update_active_leaf(existing.id, leaf_id);
            existing.id
        } else {
            match storage.create_thread(r_id, AnchorId::now_v7()) {
                Ok(tid) => {
                    let _ = storage.update_active_leaf(tid, leaf_id);
                    tid
                }
                Err(_) => return,
            }
        };

        let title = self
            .threads
            .first()
            .map(|t| t.title.clone())
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| "Active Session".to_string());

        let entries = [
            ("active_thread_id".to_string(), real_tid.to_string()),
            ("active_thread_title".to_string(), title),
        ];

        for (key, value) in entries {
            let _ = storage.delete_blackboard_entry(&key);
            let _ = storage.insert_blackboard_entry(&BlackboardEntry {
                id: BlackboardEntryId::now_v7(),
                key,
                value,
                kind: BlackboardEntryKind::Note,
                created_at: 1,
            });
        }
        let _ = storage.delete_blackboard_entry("active_thread_custom_name");
        if let Some(custom_name) = self
            .threads
            .first()
            .and_then(|t| normalize_custom_name(t.custom_name.as_deref()))
        {
            let _ = storage.insert_blackboard_entry(&BlackboardEntry {
                id: BlackboardEntryId::now_v7(),
                key: "active_thread_custom_name".to_string(),
                value: custom_name,
                kind: BlackboardEntryKind::Note,
                created_at: 1,
            });
        }
    }

    fn load_state_from_sqlite(&mut self, db_path: &Path) {
        if !db_path.exists() {
            return;
        }
        let Ok(storage) = SqliteStorage::open(db_path) else {
            return;
        };
        let Some(tid) = storage
            .get_blackboard_entry("active_thread_id")
            .ok()
            .flatten()
            .and_then(|e| e.value.parse().ok().map(ThreadId))
        else {
            return;
        };
        let Some(thread) = storage.get_thread(tid).ok().flatten() else {
            return;
        };
        let Ok(trunk) = storage.get_trunk_path(thread.active_leaf_id) else {
            return;
        };

        let mut reconstructed = String::new();
        let mut blocks = ThreadTranscript::new();
        let mut first_user_content: Option<String> = None;
        for node in &trunk {
            if node.role == AuthorRole::User {
                if first_user_content.is_none() {
                    first_user_content = Some(node.content.clone());
                }
                blocks.push_user(&node.content);
            } else {
                blocks.blocks.push(TranscriptBlock::AssistantMarkdown {
                    content: node.content.clone(),
                });
            }
            reconstructed.push_str(&render_node(&node.role, &node.content));
        }
        self.active_transcript_md = reconstructed;

        let title = storage
            .get_blackboard_entry("active_thread_title")
            .ok()
            .flatten()
            .map(|e| e.value)
            .filter(|v| !v.trim().is_empty())
            .or(first_user_content)
            .unwrap_or_else(|| "Active Session".to_string());
        let custom_name = storage
            .get_blackboard_entry("active_thread_custom_name")
            .ok()
            .flatten()
            .and_then(|e| normalize_custom_name(Some(&e.value)));

        let tid_str = thread.id.to_string();
        self.threads = vec![ThreadItem {
            id: tid_str.clone(),
            title,
            custom_name,
            anchors: Vec::new(),
        }];
        self.active_thread_id = tid_str.clone();
        self.thread_blocks.clear();
        self.thread_blocks_synced_md.clear();

        self.sync_anchors();
        self.thread_blocks.insert(tid_str.clone(), blocks);
        sync_thread_blocks_md(self, &tid_str);
    }
}
