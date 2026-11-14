use ratatui::layout::{Constraint, Direction, Layout, Rect};

pub use crate::app::FocusedPane as PaneFocus;

pub struct LayoutManager;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppLayout {
    pub tree_area: Rect,
    pub transcript_area: Rect,
    pub blackboard_area: Option<Rect>,
    pub composer_area: Rect,
}

pub type WorkspaceLayout = AppLayout;

pub fn compute_layout(area: Rect, blackboard_collapsed: bool) -> AppLayout {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(area);

    let top_area = vertical[0];
    let composer_area = vertical[1];

    if blackboard_collapsed {
        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(22), Constraint::Percentage(78)])
            .split(top_area);
        AppLayout {
            tree_area: horizontal[0],
            transcript_area: horizontal[1],
            blackboard_area: None,
            composer_area,
        }
    } else {
        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(22),
                Constraint::Percentage(56),
                Constraint::Percentage(22),
            ])
            .split(top_area);
        AppLayout {
            tree_area: horizontal[0],
            transcript_area: horizontal[1],
            blackboard_area: Some(horizontal[2]),
            composer_area,
        }
    }
}
