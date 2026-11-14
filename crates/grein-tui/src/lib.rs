use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Asynchronous entrypoint for the TUI application.
pub async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    Ok(())
}

/// Discrete pane identifiers across the 4-pane focus navigation ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaneFocus {
    #[default]
    Tree,
    Thread,
    Composer,
    Anchor,
}

/// Event container for TUI execution lifecycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    Key(KeyEvent),
    Tick,
    Quit,
}

/// Directional traversal vectors for tree navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationDirection {
    Up,
    Down,
}

/// Result outcome from input event routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEventResult {
    Consumed,
    Ignored,
}

/// Manages active pane focus transitions and Tab / BackTab cycling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusManager {
    current: PaneFocus,
    panes: [PaneFocus; 4],
}

impl Default for FocusManager {
    fn default() -> Self {
        Self::new(PaneFocus::Tree)
    }
}

impl FocusManager {
    pub fn new(initial: PaneFocus) -> Self {
        Self {
            current: initial,
            panes: [
                PaneFocus::Tree,
                PaneFocus::Thread,
                PaneFocus::Composer,
                PaneFocus::Anchor,
            ],
        }
    }

    pub fn current_focus(&self) -> PaneFocus {
        self.current
    }

    pub fn focus_pane(&mut self, pane: PaneFocus) {
        self.current = pane;
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> PaneFocus {
        if key.code == KeyCode::BackTab
            || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT))
        {
            self.cycle_backward();
        } else if key.code == KeyCode::Tab {
            self.cycle_forward();
        }
        self.current
    }

    pub fn cycle_forward(&mut self) -> PaneFocus {
        let current_idx = self
            .panes
            .iter()
            .position(|&p| p == self.current)
            .unwrap_or(0);
        let next_idx = (current_idx + 1) % self.panes.len();
        self.current = self.panes[next_idx];
        self.current
    }

    pub fn cycle_backward(&mut self) -> PaneFocus {
        let current_idx = self
            .panes
            .iter()
            .position(|&p| p == self.current)
            .unwrap_or(0);
        let prev_idx = if current_idx == 0 {
            self.panes.len() - 1
        } else {
            current_idx - 1
        };
        self.current = self.panes[prev_idx];
        self.current
    }
}

/// Individual node in the thread & anchor navigator tree view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeNode {
    pub id: String,
    pub label: String,
}

impl TreeNode {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn label(&self) -> &str {
        &self.label
    }
}

/// State model governing tree node elements, selection, and boundary invariants.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeViewState {
    nodes: Vec<TreeNode>,
    selected_index: Option<usize>,
}

impl TreeViewState {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            selected_index: None,
        }
    }

    pub fn set_nodes(&mut self, nodes: Vec<TreeNode>) {
        let is_empty = nodes.is_empty();
        self.nodes = nodes;
        self.selected_index = if is_empty { None } else { Some(0) };
    }

    pub fn nodes(&self) -> &[TreeNode] {
        &self.nodes
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.selected_index
    }

    pub fn select(&mut self, index: Option<usize>) {
        if let Some(idx) = index {
            if idx < self.nodes.len() {
                self.selected_index = Some(idx);
            }
        } else {
            self.selected_index = None;
        }
    }

    pub fn navigate(&mut self, direction: NavigationDirection) {
        if self.nodes.is_empty() {
            self.selected_index = None;
            return;
        }

        match direction {
            NavigationDirection::Up => {
                let current = self.selected_index.unwrap_or(0);
                if current > 0 {
                    self.selected_index = Some(current - 1);
                }
            }
            NavigationDirection::Down => {
                let current = self.selected_index.unwrap_or(0);
                if current + 1 < self.nodes.len() {
                    self.selected_index = Some(current + 1);
                }
            }
        }
    }
}

/// State container for composer input buffer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComposerState {
    pub content: String,
}

impl ComposerState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn append_char(&mut self, c: char) {
        self.content.push(c);
    }

    pub fn set_content(&mut self, content: impl Into<String>) {
        self.content = content.into();
    }

    pub fn clear(&mut self) {
        self.content.clear();
    }
}

/// Aggregated top-level state model for the TUI frontend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppState {
    pub focus_manager: FocusManager,
    pub tree_state: TreeViewState,
    pub composer_state: ComposerState,
}

impl Default for AppState {
    fn default() -> Self {
        let mut tree_state = TreeViewState::new();
        tree_state.set_nodes(vec![
            TreeNode::new("thread-1", "Main Discussion Thread"),
            TreeNode::new("thread-2", "Architecture Decision Review"),
            TreeNode::new("thread-3", "Implementation Notes"),
        ]);
        Self {
            focus_manager: FocusManager::new(PaneFocus::Tree),
            tree_state,
            composer_state: ComposerState::new(),
        }
    }
}

/// Dispatches raw key events to the currently focused pane handler.
pub struct InputRouter;

impl InputRouter {
    pub fn route_key_event(app_state: &mut AppState, key: KeyEvent) -> InputEventResult {
        if key.code == KeyCode::Tab || key.code == KeyCode::BackTab {
            app_state.focus_manager.handle_key(key);
            return InputEventResult::Consumed;
        }

        match app_state.focus_manager.current_focus() {
            PaneFocus::Tree => match key.code {
                KeyCode::Up => {
                    app_state.tree_state.navigate(NavigationDirection::Up);
                    InputEventResult::Consumed
                }
                KeyCode::Down => {
                    app_state.tree_state.navigate(NavigationDirection::Down);
                    InputEventResult::Consumed
                }
                _ => InputEventResult::Ignored,
            },
            PaneFocus::Composer => match key.code {
                KeyCode::Char(c) => {
                    app_state.composer_state.append_char(c);
                    InputEventResult::Consumed
                }
                KeyCode::Backspace => {
                    app_state.composer_state.content.pop();
                    InputEventResult::Consumed
                }
                _ => InputEventResult::Ignored,
            },
            _ => InputEventResult::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn test_focus_cycle_transitions_forward_and_backward() {
        let mut focus_manager = FocusManager::new(PaneFocus::Tree);
        assert_eq!(focus_manager.current_focus(), PaneFocus::Tree);

        // Forward transition (Tab cycle)
        let next_focus = focus_manager.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_ne!(next_focus, PaneFocus::Tree);

        // Exhaustive cycle back to Tree
        let mut cycles = 1;
        while focus_manager.current_focus() != PaneFocus::Tree && cycles < 10 {
            focus_manager.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
            cycles += 1;
        }
        assert_eq!(focus_manager.current_focus(), PaneFocus::Tree);
        assert_eq!(
            cycles, 4,
            "Focus cycle must transition deterministically across all 4 panes"
        );

        // Backward transition (BackTab / Shift+Tab)
        focus_manager.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_ne!(focus_manager.current_focus(), PaneFocus::Tree);

        // Reverse back to Tree
        focus_manager.focus_pane(PaneFocus::Tree);
        assert_eq!(focus_manager.current_focus(), PaneFocus::Tree);
    }

    #[test]
    fn test_tree_pane_node_navigation_invariants() {
        let mut tree_state = TreeViewState::new();
        let root_nodes = vec![
            TreeNode::new("thread-1", "Main Discussion Thread"),
            TreeNode::new("thread-2", "Architecture Decision Review"),
            TreeNode::new("thread-3", "Implementation Notes"),
        ];
        tree_state.set_nodes(root_nodes);

        // Invariant: Initial selection defaults to root index 0
        assert_eq!(tree_state.selected_index(), Some(0));

        // Navigation invariant: Cannot navigate above upper boundary
        tree_state.navigate(NavigationDirection::Up);
        assert_eq!(tree_state.selected_index(), Some(0));

        // Navigation invariant: Sequential downward traversal
        tree_state.navigate(NavigationDirection::Down);
        assert_eq!(tree_state.selected_index(), Some(1));

        tree_state.navigate(NavigationDirection::Down);
        assert_eq!(tree_state.selected_index(), Some(2));

        // Navigation invariant: Cannot exceed lower boundary
        tree_state.navigate(NavigationDirection::Down);
        assert_eq!(tree_state.selected_index(), Some(2));

        // Navigation invariant: Sequential upward traversal
        tree_state.navigate(NavigationDirection::Up);
        assert_eq!(tree_state.selected_index(), Some(1));
    }

    #[test]
    fn test_input_event_delegation_to_tree_when_focused() {
        let mut app_state = AppState::default();
        app_state.focus_manager.focus_pane(PaneFocus::Tree);

        let down_key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let result = InputRouter::route_key_event(&mut app_state, down_key);

        // When focused on Tree, arrow keys must be delegated and consumed by tree navigator
        assert_eq!(result, InputEventResult::Consumed);
        assert_eq!(app_state.tree_state.selected_index(), Some(1));
    }

    #[test]
    fn test_input_event_delegation_bypasses_tree_when_unfocused() {
        let mut app_state = AppState::default();
        // Focus a pane other than Tree (e.g. Composer)
        app_state.focus_manager.focus_pane(PaneFocus::Composer);

        let initial_tree_selection = app_state.tree_state.selected_index();

        let key_event = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        let result = InputRouter::route_key_event(&mut app_state, key_event);

        // Keystroke must be delegated to the active Composer pane, leaving Tree state untouched
        assert_eq!(result, InputEventResult::Consumed);
        assert_eq!(
            app_state.tree_state.selected_index(),
            initial_tree_selection
        );
        assert!(app_state.composer_state.content().contains('j'));
    }
}
