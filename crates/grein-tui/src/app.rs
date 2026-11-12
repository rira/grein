use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    TreeNavigator,
    Transcript,
    Blackboard,
    Composer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorTarget {
    pub thread_id: String,
    pub scrollback: usize,
}

#[derive(Debug)]
pub struct App {
    focused_pane: Pane,
    blackboard_visible: bool,
    active_thread_id: String,
    scrollback_buffers: HashMap<String, usize>,
    anchors: HashMap<u8, AnchorTarget>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    #[must_use]
    pub fn new() -> Self {
        Self {
            focused_pane: Pane::TreeNavigator,
            blackboard_visible: true,
            active_thread_id: String::new(),
            scrollback_buffers: HashMap::new(),
            anchors: HashMap::new(),
        }
    }

    #[must_use]
    pub fn focused_pane(&self) -> Pane {
        self.focused_pane
    }

    #[must_use]
    pub fn is_blackboard_visible(&self) -> bool {
        self.blackboard_visible
    }

    fn focusable_panes(&self) -> Vec<Pane> {
        if self.blackboard_visible {
            vec![
                Pane::TreeNavigator,
                Pane::Transcript,
                Pane::Blackboard,
                Pane::Composer,
            ]
        } else {
            vec![Pane::TreeNavigator, Pane::Transcript, Pane::Composer]
        }
    }

    pub fn focus_next(&mut self) {
        let panes = self.focusable_panes();
        if let Some(pos) = panes.iter().position(|&p| p == self.focused_pane) {
            let next_pos = (pos + 1) % panes.len();
            self.focused_pane = panes[next_pos];
        } else {
            self.focused_pane = Pane::TreeNavigator;
        }
    }

    pub fn focus_prev(&mut self) {
        let panes = self.focusable_panes();
        if let Some(pos) = panes.iter().position(|&p| p == self.focused_pane) {
            let prev_pos = if pos == 0 { panes.len() - 1 } else { pos - 1 };
            self.focused_pane = panes[prev_pos];
        } else {
            self.focused_pane = Pane::TreeNavigator;
        }
    }

    pub fn toggle_blackboard(&mut self) {
        self.blackboard_visible = !self.blackboard_visible;
        if !self.blackboard_visible && self.focused_pane == Pane::Blackboard {
            self.focused_pane = Pane::Transcript;
        }
    }

    pub fn register_anchor(
        &mut self,
        anchor_num: u8,
        thread_id: impl Into<String>,
        scrollback: usize,
    ) {
        if (1..=9).contains(&anchor_num) {
            self.anchors.insert(
                anchor_num,
                AnchorTarget {
                    thread_id: thread_id.into(),
                    scrollback,
                },
            );
        }
    }

    pub fn jump_to_anchor(&mut self, anchor_num: u8) -> bool {
        if !(1..=9).contains(&anchor_num) {
            return false;
        }

        if let Some(target) = self.anchors.get(&anchor_num).cloned() {
            self.active_thread_id = target.thread_id.clone();
            self.scrollback_buffers
                .insert(target.thread_id, target.scrollback);
            true
        } else {
            false
        }
    }

    #[must_use]
    pub fn active_thread_id(&self) -> &str {
        &self.active_thread_id
    }

    #[must_use]
    pub fn active_thread_scrollback(&self) -> usize {
        self.scrollback_buffers
            .get(&self.active_thread_id)
            .copied()
            .unwrap_or(0)
    }

    pub fn select_thread(&mut self, thread_id: impl Into<String>) {
        let id = thread_id.into();
        self.scrollback_buffers.entry(id.clone()).or_insert(0);
        self.active_thread_id = id;
    }

    pub fn scroll_active_thread(&mut self, delta: isize) {
        let current = self.active_thread_scrollback() as isize;
        let new_pos = (current + delta).max(0) as usize;
        self.scrollback_buffers
            .insert(self.active_thread_id.clone(), new_pos);
    }
}
