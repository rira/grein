use crate::error::CoreError;
use crate::model::{Anchor, ConversationNode, Thread, ThreadStatus};
pub use crate::storage::sqlite::{SqliteStorage, SqliteStorage as SqliteDagStorage};
use std::collections::HashMap;
use uuid::Uuid;

pub trait DagStorage {
    fn insert_node(&self, node: &ConversationNode) -> Result<(), CoreError>;
    fn get_node(&self, id: &Uuid) -> Result<Option<ConversationNode>, CoreError>;
    fn get_children(&self, parent_id: &Uuid) -> Result<Vec<ConversationNode>, CoreError>;
    fn get_trunk_path(&self, leaf_id: &Uuid) -> Result<Vec<ConversationNode>, CoreError>;
    fn create_thread(&self, parent_node_id: &Uuid, anchor_id: &Uuid) -> Result<Uuid, CoreError>;
    fn get_thread(&self, thread_id: &Uuid) -> Result<Option<Thread>, CoreError>;
    fn list_threads(&self) -> Result<Vec<Thread>, CoreError>;
    fn list_nodes(&self) -> Result<Vec<ConversationNode>, CoreError>;
    fn get_anchor(&self, anchor_id: &Uuid) -> Result<Option<Anchor>, CoreError>;
    fn update_thread_leaf(&self, thread_id: &Uuid, leaf_id: &Uuid) -> Result<(), CoreError>;
    fn update_thread_status(&self, thread_id: &Uuid, status: ThreadStatus)
        -> Result<(), CoreError>;
}

impl DagStorage for SqliteStorage {
    fn insert_node(&self, node: &ConversationNode) -> Result<(), CoreError> {
        SqliteStorage::insert_node(self, node)
    }

    fn get_node(&self, id: &Uuid) -> Result<Option<ConversationNode>, CoreError> {
        SqliteStorage::get_node(self, id)
    }

    fn get_children(&self, parent_id: &Uuid) -> Result<Vec<ConversationNode>, CoreError> {
        SqliteStorage::get_children(self, parent_id)
    }

    fn get_trunk_path(&self, leaf_id: &Uuid) -> Result<Vec<ConversationNode>, CoreError> {
        SqliteStorage::get_trunk_path(self, leaf_id)
    }

    fn create_thread(&self, parent_node_id: &Uuid, anchor_id: &Uuid) -> Result<Uuid, CoreError> {
        SqliteStorage::create_thread(self, parent_node_id, anchor_id).map(|id| id.0)
    }

    fn get_thread(&self, thread_id: &Uuid) -> Result<Option<Thread>, CoreError> {
        SqliteStorage::get_thread(self, thread_id)
    }

    fn list_threads(&self) -> Result<Vec<Thread>, CoreError> {
        SqliteStorage::list_threads(self)
    }

    fn list_nodes(&self) -> Result<Vec<ConversationNode>, CoreError> {
        SqliteStorage::list_nodes(self)
    }

    fn get_anchor(&self, anchor_id: &Uuid) -> Result<Option<Anchor>, CoreError> {
        SqliteStorage::get_anchor(self, anchor_id)
    }

    fn update_thread_leaf(&self, thread_id: &Uuid, leaf_id: &Uuid) -> Result<(), CoreError> {
        SqliteStorage::update_thread_leaf(self, thread_id, leaf_id)
    }

    fn update_thread_status(
        &self,
        thread_id: &Uuid,
        status: ThreadStatus,
    ) -> Result<(), CoreError> {
        SqliteStorage::update_thread_status(self, thread_id, status)
    }
}

pub fn get_trunk_path(
    storage: &impl DagStorage,
    leaf_id: &Uuid,
) -> Result<Vec<ConversationNode>, CoreError> {
    storage.get_trunk_path(leaf_id)
}

pub struct ConversationDag {
    nodes: HashMap<Uuid, ConversationNode>,
}

impl ConversationDag {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
        }
    }

    pub fn insert_node(&mut self, node: ConversationNode) {
        self.nodes.insert(node.id.0, node);
    }

    pub fn get_node(&self, id: &Uuid) -> Result<&ConversationNode, CoreError> {
        self.nodes
            .get(id)
            .ok_or_else(|| CoreError::NodeNotFound(id.to_string()))
    }
}

impl Default for ConversationDag {
    fn default() -> Self {
        Self::new()
    }
}
