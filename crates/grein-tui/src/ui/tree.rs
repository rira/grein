use std::collections::BTreeMap;
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
    pub scroll_offset: u16,
    pub depths: BTreeMap<String, usize>,
    pub prefixes: BTreeMap<String, String>,
}

/// Computes the vertical scroll offset keeping `selected_row` visible within `viewport_height`.
pub fn compute_tree_scroll_offset(
    current_offset: u16,
    selected_row: Option<usize>,
    total_rows: usize,
    viewport_height: u16,
) -> u16 {
    let vp = viewport_height as usize;
    if vp == 0 || total_rows <= vp {
        return 0;
    }
    let max_offset = total_rows - vp;
    let mut offset = (current_offset as usize).min(max_offset);
    if let Some(sel) = selected_row {
        let sel = sel.min(total_rows.saturating_sub(1));
        if sel < offset {
            offset = sel;
        } else if sel >= offset + vp {
            offset = sel + 1 - vp;
        }
    }
    offset.min(u16::MAX as usize) as u16
}

impl TreeState {
    pub fn new(nodes: Vec<TreeNode>) -> Self {
        let selected_index = if nodes.is_empty() { None } else { Some(0) };
        Self {
            nodes,
            selected_index,
            active_anchor_id: None,
            scroll_offset: 0,
            depths: BTreeMap::new(),
            prefixes: BTreeMap::new(),
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

    pub fn scroll_offset(&self) -> u16 {
        self.scroll_offset
    }

    pub fn set_scroll_offset(&mut self, offset: u16) {
        self.scroll_offset = offset;
    }

    pub fn depth_for(&self, thread_id: &str) -> usize {
        self.depths.get(thread_id).copied().unwrap_or(0)
    }

    pub fn set_depths(&mut self, depths: BTreeMap<String, usize>) {
        self.depths = depths;
    }

    pub fn prefix_for(&self, thread_id: &str) -> &str {
        self.prefixes
            .get(thread_id)
            .map(String::as_str)
            .unwrap_or("")
    }

    pub fn set_prefixes(&mut self, prefixes: BTreeMap<String, String>) {
        self.prefixes = prefixes;
    }

    pub fn ensure_selected_visible(&mut self, viewport_height: u16) {
        self.scroll_offset = compute_tree_scroll_offset(
            self.scroll_offset,
            self.selected_index,
            self.nodes.len(),
            viewport_height,
        );
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
