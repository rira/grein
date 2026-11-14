use ratatui::layout::{Constraint, Direction, Layout, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PaneFocus {
    #[default]
    Workspace,
    Composer,
    Blackboard,
}

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

    let main_horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(top_area);

    let tree_area = main_horizontal[0];
    let right_area = main_horizontal[1];

    if blackboard_collapsed {
        AppLayout {
            tree_area,
            transcript_area: right_area,
            blackboard_area: None,
            composer_area,
        }
    } else {
        let right_split = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(right_area);
        AppLayout {
            tree_area,
            transcript_area: right_split[0],
            blackboard_area: Some(right_split[1]),
            composer_area,
        }
    }
}
