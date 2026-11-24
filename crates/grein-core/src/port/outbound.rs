use std::collections::BTreeMap;

use uuid::Uuid;

use crate::blackboard::BlackboardEntry;
use crate::dag::DagStorage;
use crate::error::Result;
use crate::model::{
    Anchor, AnchorId, ConversationNode, NodeId, Thread, ThreadId, ThreadKind, ThreadStatus,
};
use crate::prefs::{ImportReport, SettingsFormat, SettingsPatch, SettingsSnapshot};

/// Outbound persistence port for session DAG nodes, threads, anchors, and blackboard state.
pub trait SessionRepository: DagStorage + Send + Sync {
    fn insert_anchor(&self, anchor_id: &Uuid, anchor: &Anchor) -> Result<()>;
    fn list_anchors(&self) -> Result<Vec<(AnchorId, Anchor)>>;
    fn link_node_anchor(&self, node_id: &Uuid, anchor_id: &Uuid) -> Result<()>;
    fn list_anchors_for_node(&self, node_id: &Uuid) -> Result<Vec<Anchor>>;

    fn list_root_nodes(&self) -> Result<Vec<ConversationNode>>;
    fn list_recent_user_prompts(&self, limit: usize) -> Result<Vec<String>>;

    fn insert_thread(&self, thread: &Thread) -> Result<()>;
    fn create_thread_with_kind(
        &self,
        parent_node_id: NodeId,
        anchor_id: AnchorId,
        title: Option<&str>,
        parent_thread_id: Option<ThreadId>,
        kind: ThreadKind,
        cwd: Option<&str>,
    ) -> Result<ThreadId>;
    fn create_thread_with_parent(
        &self,
        parent_node_id: NodeId,
        anchor_id: AnchorId,
        title: Option<&str>,
        parent_thread_id: Option<ThreadId>,
    ) -> Result<ThreadId>;
    fn create_thread_with_title(
        &self,
        parent_node_id: NodeId,
        anchor_id: AnchorId,
        title: &str,
    ) -> Result<ThreadId>;
    fn reparent_child_threads(
        &self,
        old_parent_id: &ThreadId,
        new_parent_id: Option<&ThreadId>,
    ) -> Result<usize>;
    fn update_thread_title(&self, thread_id: &ThreadId, title: &str) -> Result<()>;
    fn update_thread_acp_session_id(
        &self,
        thread_id: &ThreadId,
        session_id: Option<&str>,
    ) -> Result<()>;
    fn mark_thread_merged(&self, thread_id: &ThreadId, merged: bool) -> Result<()>;
    fn update_thread_cwd(&self, thread_id: &ThreadId, cwd: Option<&str>) -> Result<()>;
    fn update_thread_custom_name(
        &self,
        thread_id: &ThreadId,
        custom_name: Option<&str>,
    ) -> Result<()>;
    fn update_thread_agent_provider(
        &self,
        thread_id: &ThreadId,
        provider: Option<&str>,
    ) -> Result<()>;
    fn delete_thread(&self, thread_id: &ThreadId) -> Result<bool>;
    fn get_thread_leaf(&self, thread_id: &ThreadId) -> Result<NodeId>;
    fn get_thread_state(&self, thread_id: &ThreadId) -> Result<ThreadStatus>;

    fn insert_blackboard_entry(&self, entry: &BlackboardEntry) -> Result<()>;
    fn get_blackboard_entry(&self, key: &str) -> Result<Option<BlackboardEntry>>;
    fn list_blackboard_entries(&self) -> Result<Vec<BlackboardEntry>>;
    fn delete_blackboard_entry(&self, key: &str) -> Result<bool>;
}

/// Outbound persistence port for user preferences and settings import/export.
pub trait PreferenceRepository: Send + Sync {
    fn get(&self, namespace: &str, key: &str) -> Result<Option<String>>;
    fn set(&self, namespace: &str, key: &str, value: &str) -> Result<()>;
    fn delete(&self, namespace: &str, key: &str) -> Result<bool>;
    fn list_namespace(&self, namespace: &str) -> Result<BTreeMap<String, String>>;

    fn keybindings(&self) -> Result<BTreeMap<String, String>>;
    fn set_keybinding(&self, action: &str, binding: &str) -> Result<()>;
    fn remove_keybinding(&self, action: &str) -> Result<bool>;
    fn clear_keybindings(&self) -> Result<usize>;

    fn theme(&self) -> Result<Option<String>>;
    fn set_theme(&self, name: &str) -> Result<()>;
    fn theme_prefs(&self) -> Result<BTreeMap<String, String>>;
    fn set_theme_pref(&self, key: &str, value: &str) -> Result<()>;
    fn clear_theme_prefs(&self) -> Result<usize>;

    fn ui_prefs(&self) -> Result<BTreeMap<String, String>>;
    fn set_ui_pref(&self, key: &str, value: &str) -> Result<()>;
    fn clear_ui_prefs(&self) -> Result<usize>;

    fn export_snapshot(&self, only_sections: Option<&[String]>) -> Result<SettingsSnapshot>;
    fn export_string(
        &self,
        format: SettingsFormat,
        only_sections: Option<&[String]>,
    ) -> Result<String>;
    fn import_patch(
        &self,
        patch: &SettingsPatch,
        only_sections: Option<&[String]>,
        replace_all: bool,
    ) -> Result<ImportReport>;
    fn import_str(
        &self,
        raw: &str,
        format: Option<SettingsFormat>,
        only_sections: Option<&[String]>,
        replace_all: bool,
    ) -> Result<ImportReport>;
    fn import_theme_str(
        &self,
        raw: &str,
        format: Option<SettingsFormat>,
        replace_theme: bool,
    ) -> Result<ImportReport>;
}

/// Outbound codec port for encoding and decoding serialized settings formats.
pub trait SettingsCodec: Send + Sync {
    fn format(&self) -> SettingsFormat;
    fn format_name(&self) -> &'static str;
    fn encode(&self, snapshot: &SettingsSnapshot) -> Result<String>;
    fn encode_section(&self, section: &str, entries: &BTreeMap<String, String>) -> Result<String>;
    fn decode(&self, raw: &str) -> Result<SettingsPatch>;
    fn decode_theme(&self, raw: &str) -> Result<SettingsPatch>;
}
