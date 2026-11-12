use crate::status::{RenderMode, Spinner, ThreadStatusIndicator};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeWidgetNode {
    pub id: String,
    pub title: String,
    pub indicator: ThreadStatusIndicator,
}

impl TreeWidgetNode {
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        indicator: ThreadStatusIndicator,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            indicator,
        }
    }

    #[must_use]
    pub fn render_label(&self, mode: RenderMode, spinner: &Spinner) -> String {
        format!("{} {}", self.indicator.render(mode, spinner), self.title)
    }
}
