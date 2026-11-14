/// Target being inspected in the Blackboard pane.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum InspectionTarget {
    #[default]
    None,
    Ticket(String),
    Node(String),
}

/// State model for the collapsible Blackboard pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlackboardState {
    visible: bool,
    inspection_target: InspectionTarget,
    decision_selection: Option<usize>,
    decision_scroll_offset: usize,
    decision_item_count: usize,
}

impl Default for BlackboardState {
    fn default() -> Self {
        Self {
            visible: true,
            inspection_target: InspectionTarget::None,
            decision_selection: None,
            decision_scroll_offset: 0,
            decision_item_count: 0,
        }
    }
}

impl BlackboardState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn toggle_visibility(&mut self) {
        self.visible = !self.visible;
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    #[must_use]
    pub fn inspection_target(&self) -> &InspectionTarget {
        &self.inspection_target
    }

    pub fn set_inspection_target(&mut self, target: InspectionTarget) {
        self.inspection_target = target;
    }

    pub fn clear_inspection_target(&mut self) {
        self.inspection_target = InspectionTarget::None;
    }

    #[must_use]
    pub fn decision_registry_selection(&self) -> Option<usize> {
        self.decision_selection
    }

    pub fn select_decision(&mut self, selection: Option<usize>) {
        self.decision_selection = selection;
    }

    #[must_use]
    pub fn decision_item_count(&self) -> usize {
        self.decision_item_count
    }

    pub fn set_decision_item_count(&mut self, count: usize) {
        self.decision_item_count = count;
    }

    pub fn select_next_decision(&mut self) {
        if self.decision_item_count == 0 {
            self.decision_selection = None;
            return;
        }
        match self.decision_selection {
            Some(idx) => {
                if idx + 1 < self.decision_item_count {
                    self.decision_selection = Some(idx + 1);
                }
            }
            None => {
                self.decision_selection = Some(0);
            }
        }
    }

    pub fn select_previous_decision(&mut self) {
        if self.decision_item_count == 0 {
            self.decision_selection = None;
            return;
        }
        match self.decision_selection {
            Some(idx) => {
                if idx > 0 {
                    self.decision_selection = Some(idx - 1);
                }
            }
            None => {
                self.decision_selection = Some(0);
            }
        }
    }

    #[must_use]
    pub fn decision_registry_scroll_offset(&self) -> usize {
        self.decision_scroll_offset
    }

    pub fn set_decision_registry_scroll_offset(&mut self, offset: usize) {
        self.decision_scroll_offset = offset;
    }

    pub fn scroll_decision_down(&mut self, delta: usize) {
        self.decision_scroll_offset = self.decision_scroll_offset.saturating_add(delta);
    }

    pub fn scroll_decision_up(&mut self, delta: usize) {
        self.decision_scroll_offset = self.decision_scroll_offset.saturating_sub(delta);
    }
}
