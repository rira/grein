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

/// Layout partitioning the workspace panes plus bottom subagent strip and 1-line status bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChromeLayout {
    pub tree_area: Rect,
    pub transcript_area: Rect,
    pub blackboard_area: Option<Rect>,
    pub composer_area: Rect,
    pub subagent_strip_area: Option<Rect>,
    pub status_bar_area: Rect,
}

/// Computes total vertical rows (including border frame) for the auto-hiding subagent strip.
pub fn subagent_strip_height(subagent_count: usize) -> u16 {
    if subagent_count == 0 {
        0
    } else if subagent_count <= 5 {
        subagent_count as u16 + 2
    } else {
        8
    }
}

fn split_top_panes(top_area: Rect, blackboard_collapsed: bool) -> (Rect, Rect, Option<Rect>) {
    if blackboard_collapsed {
        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(34), Constraint::Percentage(66)])
            .split(top_area);
        (horizontal[0], horizontal[1], None)
    } else {
        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(34),
                Constraint::Percentage(46),
                Constraint::Percentage(20),
            ])
            .split(top_area);
        (horizontal[0], horizontal[1], Some(horizontal[2]))
    }
}

pub fn compute_layout(area: Rect, blackboard_collapsed: bool) -> AppLayout {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(area);

    let (tree_area, transcript_area, blackboard_area) =
        split_top_panes(vertical[0], blackboard_collapsed);
    AppLayout {
        tree_area,
        transcript_area,
        blackboard_area,
        composer_area: vertical[1],
    }
}

/// Computes the vertical chrome layout placing the optional subagent strip and status bar below Composer.
pub fn compute_layout_with_chrome(
    area: Rect,
    blackboard_collapsed: bool,
    subagent_rows: u16,
) -> ChromeLayout {
    if subagent_rows == 0 {
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(area);
        let (tree_area, transcript_area, blackboard_area) =
            split_top_panes(vertical[0], blackboard_collapsed);
        ChromeLayout {
            tree_area,
            transcript_area,
            blackboard_area,
            composer_area: vertical[1],
            subagent_strip_area: None,
            status_bar_area: vertical[2],
        }
    } else {
        let max_strip = area.height.saturating_sub(7).max(1);
        let strip_h = subagent_rows.min(max_strip);
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(3),
                Constraint::Length(strip_h),
                Constraint::Length(1),
            ])
            .split(area);
        let (tree_area, transcript_area, blackboard_area) =
            split_top_panes(vertical[0], blackboard_collapsed);
        ChromeLayout {
            tree_area,
            transcript_area,
            blackboard_area,
            composer_area: vertical[1],
            subagent_strip_area: Some(vertical[2]),
            status_bar_area: vertical[3],
        }
    }
}
