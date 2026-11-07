use crate::error::CoreError;
use crate::model::ConversationNode;
use std::collections::HashMap;
use uuid::Uuid;

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
        self.nodes.insert(node.id, node);
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
