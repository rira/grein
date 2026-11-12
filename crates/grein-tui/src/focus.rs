use crate::status::ThreadId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocusTarget {
    None,
    Composer(ThreadId),
    ThreadList,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusManager {
    current: FocusTarget,
}

impl FocusManager {
    pub fn new() -> Self {
        Self {
            current: FocusTarget::None,
        }
    }

    pub fn set_focus(&mut self, target: FocusTarget) {
        self.current = target;
    }

    pub fn current_focus(&self) -> FocusTarget {
        self.current.clone()
    }
}

impl Default for FocusManager {
    fn default() -> Self {
        Self::new()
    }
}
