use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, StatefulWidget, Widget};
use std::collections::HashSet;

/// Represents a hierarchical node in the Thread & Anchor tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeNode {
    id: String,
    label: String,
    children: Vec<TreeNode>,
}

impl TreeNode {
    pub fn branch(
        id: impl Into<String>,
        label: impl Into<String>,
        children: Vec<TreeNode>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            children,
        }
    }

    pub fn leaf(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            children: Vec::new(),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn children(&self) -> &[TreeNode] {
        &self.children
    }

    pub fn is_leaf(&self) -> bool {
        self.children.is_empty()
    }
}

/// State tracking tree expansion, visible nodes, and active selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeNavigatorState {
    nodes: Vec<TreeNode>,
    expanded: HashSet<String>,
    selected_index: usize,
}

impl Default for TreeNavigatorState {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl TreeNavigatorState {
    pub fn new(nodes: Vec<TreeNode>) -> Self {
        let mut expanded = HashSet::new();
        Self::expand_branches_recursive(&nodes, &mut expanded);
        Self {
            nodes,
            expanded,
            selected_index: 0,
        }
    }

    fn expand_branches_recursive(nodes: &[TreeNode], expanded: &mut HashSet<String>) {
        for node in nodes {
            if !node.children.is_empty() {
                expanded.insert(node.id.clone());
                Self::expand_branches_recursive(&node.children, expanded);
            }
        }
    }

    pub fn is_expanded(&self, id: &str) -> bool {
        self.expanded.contains(id)
    }

    pub fn toggle_expanded(&mut self, id: &str) {
        if self.expanded.contains(id) {
            self.expanded.remove(id);
        } else {
            self.expanded.insert(id.to_string());
        }
        self.clamp_selection();
    }

    pub fn set_expanded(&mut self, id: impl Into<String>, expanded: bool) {
        let id_str = id.into();
        if expanded {
            self.expanded.insert(id_str);
        } else {
            self.expanded.remove(&id_str);
        }
        self.clamp_selection();
    }

    pub fn visible_nodes(&self) -> Vec<&TreeNode> {
        let mut result = Vec::new();
        for node in &self.nodes {
            self.collect_visible(node, &mut result);
        }
        result
    }

    fn collect_visible<'a>(&self, node: &'a TreeNode, acc: &mut Vec<&'a TreeNode>) {
        acc.push(node);
        if !node.children.is_empty() && self.is_expanded(&node.id) {
            for child in &node.children {
                self.collect_visible(child, acc);
            }
        }
    }

    pub fn visible_nodes_with_depth(&self) -> Vec<(&TreeNode, usize)> {
        let mut result = Vec::new();
        for node in &self.nodes {
            self.collect_visible_with_depth(node, 0, &mut result);
        }
        result
    }

    fn collect_visible_with_depth<'a>(
        &self,
        node: &'a TreeNode,
        depth: usize,
        acc: &mut Vec<(&'a TreeNode, usize)>,
    ) {
        acc.push((node, depth));
        if !node.children.is_empty() && self.is_expanded(&node.id) {
            for child in &node.children {
                self.collect_visible_with_depth(child, depth + 1, acc);
            }
        }
    }

    pub fn selected_id(&self) -> Option<&str> {
        let visible = self.visible_nodes();
        if visible.is_empty() {
            None
        } else {
            let idx = self.selected_index.min(visible.len() - 1);
            Some(visible[idx].id())
        }
    }

    pub fn select_next(&mut self) {
        let count = self.visible_nodes().len();
        if count > 0 && self.selected_index + 1 < count {
            self.selected_index += 1;
        }
    }

    pub fn select_previous(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    pub fn select_index(&mut self, index: usize) {
        self.selected_index = index;
        self.clamp_selection();
    }

    fn clamp_selection(&mut self) {
        let count = self.visible_nodes().len();
        if count == 0 {
            self.selected_index = 0;
        } else if self.selected_index >= count {
            self.selected_index = count - 1;
        }
    }

    pub fn nodes(&self) -> &[TreeNode] {
        &self.nodes
    }

    pub fn set_nodes(&mut self, nodes: Vec<TreeNode>) {
        self.nodes = nodes;
        self.clamp_selection();
    }
}

/// Stateful widget rendering the Thread & Anchor tree navigation pane.
#[derive(Debug, Clone)]
pub struct TreeNavigator<'a> {
    title: &'a str,
    focused: bool,
}

impl<'a> Default for TreeNavigator<'a> {
    fn default() -> Self {
        Self {
            title: "Navigator",
            focused: false,
        }
    }
}

impl<'a> TreeNavigator<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn title(mut self, title: &'a str) -> Self {
        self.title = title;
        self
    }

    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }
}

impl StatefulWidget for TreeNavigator<'_> {
    type State = TreeNavigatorState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if area.width < 2 || area.height < 2 {
            return;
        }

        let (border_style, border_type) = if self.focused {
            (
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
                BorderType::Rounded,
            )
        } else {
            (Style::default().fg(Color::DarkGray), BorderType::Plain)
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(border_type)
            .border_style(border_style)
            .title(self.title);

        let inner_area = block.inner(area);
        block.render(area, buf);

        if inner_area.height == 0 || inner_area.width == 0 {
            return;
        }

        let visible_items = state.visible_nodes_with_depth();
        let selected_idx = state.selected_index;

        for (i, (node, depth)) in visible_items.into_iter().enumerate() {
            let y = inner_area.y + i as u16;
            if y >= inner_area.bottom() {
                break;
            }

            let is_selected = i == selected_idx;
            let icon = if node.is_leaf() {
                "  "
            } else if state.is_expanded(&node.id) {
                "▼ "
            } else {
                "▶ "
            };

            let indent = "  ".repeat(depth);
            let display_text = format!("{}{}{}", indent, icon, node.label);

            let style = if is_selected {
                if self.focused {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                } else {
                    Style::default().fg(Color::White).bg(Color::DarkGray)
                }
            } else {
                Style::default()
            };

            let max_len = inner_area.width as usize;
            let truncated: String = display_text.chars().take(max_len).collect();
            buf.set_string(inner_area.x, y, &truncated, style);
        }
    }
}
