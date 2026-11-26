use crate::app::{AppState, SidebarMode};
use crate::layout::{compute_layout_with_options, ChromeLayout};
use crate::runner::compute_ordered_thread_entries_with_folds;
use crate::theme::{ThemePalette, ThemePreset};
use crate::ui::tree::TreeNode;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders};

pub fn border_style(is_focused: bool) -> Style {
    let palette = ThemePalette::from_preset(ThemePreset::TokyoNight);
    themed_border_style(&palette, is_focused)
}

pub(super) fn themed_border_style(theme: &ThemePalette, is_focused: bool) -> Style {
    if is_focused {
        Style::default()
            .fg(theme.border_focused)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.border_unfocused)
    }
}

pub(super) fn rounded_block(theme: &ThemePalette, is_focused: bool) -> Block<'static> {
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(themed_border_style(theme, is_focused));
    let mut style = Style::default();
    let mut has_style = false;
    if theme.panel_bg != Color::Reset {
        style = style.bg(theme.panel_bg);
        has_style = true;
    }
    if theme.text != Color::Reset {
        style = style.fg(theme.text);
        has_style = true;
    }
    if has_style {
        block = block.style(style);
    }
    block
}

fn max_grein_sidebar_row_width(state: &AppState) -> usize {
    let mut max_w = 26usize;
    if !state.tree.nodes.is_empty() {
        let mut root_count = 0usize;
        for node in &state.tree.nodes {
            if let TreeNode::Thread(t) = node {
                let depth = state.tree.depth_for(&t.id);
                let root_w = if depth == 0 {
                    root_count += 1;
                    if root_count <= 9 { 2 } else { 0 }
                } else {
                    0
                };
                let prefix_w = state.tree.prefix_for(&t.id).chars().count();
                let title_w = state.display_title_for(&t.id, &t.title).chars().count();
                let branch_w = state
                    .thread_meta
                    .get(&t.id)
                    .and_then(|m| m.git_branch.as_deref())
                    .map(|b| b.chars().count().min(14) + 3)
                    .unwrap_or(0);
                max_w = max_w.max(root_w + prefix_w + title_w + branch_w + 10);
            }
        }
    } else {
        let ordered = compute_ordered_thread_entries_with_folds(
            &state.threads,
            &state.thread_meta,
            &state.collapsed_threads,
        );
        let mut root_count = 0usize;
        for e in ordered {
            let t = &state.threads[e.index];
            let root_w = if e.depth == 0 {
                root_count += 1;
                if root_count <= 9 { 2 } else { 0 }
            } else {
                0
            };
            let prefix_w = e.prefix.chars().count();
            let title_w = state.display_title_for(&t.id, &t.title).chars().count();
            max_w = max_w.max(root_w + prefix_w + title_w + 10);
        }
    }
    max_w + 2
}

fn max_worktree_sidebar_row_width(state: &AppState) -> usize {
    let mut max_w = 26usize;
    for row in state.visible_worktree_rows() {
        let row_w = match row {
            crate::app::WorktreeRow::RepoHeader {
                repo_name, count, ..
            } => repo_name.chars().count() + format!(" ({count})").len() + 4,
            crate::app::WorktreeRow::Entry(entry) => {
                let label_w = crate::app::worktree_display_label(&entry).chars().count();
                let primary_w = if entry.is_primary { 10 } else { 0 };
                let dirty_w = if entry.dirty_count > 0 { 4 } else { 0 };
                let attached_w = if entry.attached_thread_ids.is_empty() {
                    0
                } else {
                    4
                };
                5 + label_w + primary_w + dirty_w + attached_w
            }
        };
        max_w = max_w.max(row_w);
    }
    max_w + 2
}

pub fn compute_sidebar_width(state: &AppState, total_w: u16) -> u16 {
    if state.sidebar_hidden || total_w < 20 {
        return 0;
    }
    let max_allowed = total_w.saturating_sub(14).max(1);
    let min_floor = 16u16.min(max_allowed);

    match state.sidebar_layout.mode {
        SidebarMode::Fixed => {
            let desired = if let Some(pct) = state.sidebar_layout.fixed_pct {
                ((total_w as u32 * pct as u32) / 100) as u16
            } else {
                state.sidebar_layout.fixed_cols
            };
            desired.clamp(min_floor, max_allowed)
        }
        SidebarMode::Dynamic => {
            let min_cols = (((total_w as u32 * state.sidebar_layout.min_pct as u32) / 100) as u16)
                .clamp(min_floor, max_allowed);
            let max_cols = (((total_w as u32 * state.sidebar_layout.max_pct as u32) / 100) as u16)
                .clamp(min_cols, max_allowed);
            let content_w = max_grein_sidebar_row_width(state)
                .max(max_worktree_sidebar_row_width(state))
                .min(u16::MAX as usize) as u16;
            content_w.clamp(min_cols, max_cols)
        }
    }
}

pub fn compute_app_layout(
    state: &AppState,
    area: Rect,
    subagent_rows: u16,
    composer_hidden: bool,
) -> ChromeLayout {
    let mut layout =
        compute_layout_with_options(area, subagent_rows, state.sidebar_hidden, composer_hidden);
    if !state.sidebar_hidden && area.width >= 20 {
        let tree_w = compute_sidebar_width(state, area.width).min(area.width.saturating_sub(10));
        let right_w = area.width.saturating_sub(tree_w);
        let right_x = area.x + tree_w;
        layout.tree_area.width = tree_w;
        layout.tree_area.height = layout.status_bar_area.y.saturating_sub(layout.tree_area.y);
        layout.transcript_area.x = right_x;
        layout.transcript_area.width = right_w;
        if !composer_hidden && layout.composer_area.width > 0 {
            layout.composer_area.x = right_x;
            layout.composer_area.width = right_w;
        }
        if let Some(ref mut sa) = layout.subagent_strip_area {
            sa.x = right_x;
            sa.width = right_w;
        }
    }
    layout
}

pub fn render_markdown_lines(markdown: &str) -> Vec<Line<'static>> {
    crate::markdown::render_markdown_lines(markdown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::widgets::Paragraph;
    use ratatui::Terminal;

    #[test]
    fn rounded_block_applies_text_fg_even_when_panel_bg_is_reset() {
        let mut theme = ThemePalette::from_preset(ThemePreset::Ansi);
        theme.text = Color::White;
        assert_eq!(theme.panel_bg, Color::Reset);

        let mut terminal = Terminal::new(TestBackend::new(10, 4)).unwrap();
        terminal
            .draw(|f| {
                let widget = Paragraph::new("hi").block(rounded_block(&theme, false));
                f.render_widget(widget, Rect::new(0, 0, 10, 4));
            })
            .unwrap();
        let cell = &terminal.backend().buffer()[(1, 1)];
        assert_eq!(cell.symbol(), "h");
        assert_eq!(cell.fg, Color::White);
        assert_eq!(cell.bg, Color::Reset);
    }
}
