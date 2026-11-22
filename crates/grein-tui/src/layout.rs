use ratatui::layout::{Constraint, Direction, Layout, Rect};

pub use crate::app::FocusedPane as PaneFocus;

pub struct LayoutManager;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppLayout {
    pub tree_area: Rect,
    pub transcript_area: Rect,
    pub composer_area: Rect,
}

pub type WorkspaceLayout = AppLayout;

/// Layout partitioning the workspace panes plus bottom subagent strip and 1-line status bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChromeLayout {
    pub tree_area: Rect,
    pub transcript_area: Rect,
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

fn split_top_panes(top_area: Rect) -> (Rect, Rect) {
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(34), Constraint::Percentage(66)])
        .split(top_area);
    (horizontal[0], horizontal[1])
}

pub fn compute_layout(area: Rect) -> AppLayout {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(area);

    let (tree_area, transcript_area) = split_top_panes(vertical[0]);
    AppLayout {
        tree_area,
        transcript_area,
        composer_area: vertical[1],
    }
}

/// Computes the vertical chrome layout placing the optional subagent strip and status bar below Composer.
pub fn compute_layout_with_chrome(area: Rect, subagent_rows: u16) -> ChromeLayout {
    compute_layout_with_chrome_and_sidebar(area, subagent_rows, false)
}

/// Computes the vertical chrome layout with optional sidebar collapse (`sidebar_hidden` gives Transcript 100% width).
pub fn compute_layout_with_chrome_and_sidebar(
    area: Rect,
    subagent_rows: u16,
    sidebar_hidden: bool,
) -> ChromeLayout {
    let (top_area, composer_area, subagent_strip_area, status_bar_area) = if subagent_rows == 0 {
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(area);
        (vertical[0], vertical[1], None, vertical[2])
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
        (vertical[0], vertical[1], Some(vertical[2]), vertical[3])
    };

    let (tree_area, transcript_area) = if sidebar_hidden {
        (
            Rect::new(top_area.x, top_area.y, 0, top_area.height),
            top_area,
        )
    } else {
        split_top_panes(top_area)
    };

    ChromeLayout {
        tree_area,
        transcript_area,
        composer_area,
        subagent_strip_area,
        status_bar_area,
    }
}
