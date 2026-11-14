use std::fmt::Debug;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadNode {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorNode {
    pub id: String,
    pub thread_id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeNode {
    Thread(ThreadNode),
    Anchor(AnchorNode),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TreeState {
    pub nodes: Vec<TreeNode>,
    selected_index: Option<usize>,
    active_anchor_id: Option<String>,
}

impl TreeState {
    pub fn new(nodes: Vec<TreeNode>) -> Self {
        let selected_index = if nodes.is_empty() { None } else { Some(0) };
        Self {
            nodes,
            selected_index,
            active_anchor_id: None,
        }
    }

    pub fn nodes(&self) -> &[TreeNode] {
        &self.nodes
    }

    pub fn select_index(&mut self, index: usize) {
        if index < self.nodes.len() {
            self.selected_index = Some(index);
        }
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.selected_index
    }

    pub fn active_anchor_id(&self) -> Option<&str> {
        self.active_anchor_id.as_deref()
    }

    pub fn set_active_anchor_id(&mut self, id: Option<String>) {
        self.active_anchor_id = id;
    }

    pub fn select_next(&mut self) {
        if self.nodes.is_empty() {
            return;
        }
        if let Some(curr) = self.selected_index {
            if curr + 1 < self.nodes.len() {
                self.selected_index = Some(curr + 1);
            }
        } else {
            self.selected_index = Some(0);
        }
    }

    pub fn select_previous(&mut self) {
        if self.nodes.is_empty() {
            return;
        }
        if let Some(curr) = self.selected_index {
            if curr > 0 {
                self.selected_index = Some(curr - 1);
            }
        } else {
            self.selected_index = Some(0);
        }
    }
}
