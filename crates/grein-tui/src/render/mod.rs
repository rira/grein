mod chrome;
mod composer;
mod layout;
mod modals;
mod panes;
mod sidebar;
#[cfg(test)]
mod tests;

#[allow(unused_imports)]
pub(crate) use composer::{
    layout_composer_visual_rows, ComposerVisualLayout, ComposerVisualRow,
};
pub use layout::{border_style, compute_app_layout, compute_sidebar_width, render_markdown_lines};
pub use panes::build_transcript_wrapped_lines;
pub use sidebar::render_tree_view;

use crate::app::{AppAction, AppState, PaneFocus, SidebarView};
use crate::engine_port::SubagentSnapshot;
use crate::layout::subagent_strip_height;
use crate::search::{highlight_lines_with_matches, TranscriptSearchState};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;

use chrome::{render_status_bar, render_subagent_strip};
use composer::layout_composer_text;
use layout::{rounded_block, themed_border_style};
use modals::{
    render_config_picker_modal, render_help_modal, render_permission_modal,
    render_quit_confirm_modal, render_theme_picker_modal, render_thread_delete_modal,
    render_worktree_autocomplete_popup, render_worktree_delete_modal,
};
use panes::{recompute_search_for_render, render_terminal_pane, render_worktree_diff_pane};
use sidebar::{render_tree_view_inner, selected_tree_thread, TreeRenderResult};

/// Renders the application state into the terminal frame.
pub fn render(f: &mut Frame, state: &AppState) {
    let _ = render_app_inner(f, state);
}

/// Renders the application state into the terminal frame and updates viewport scroll bounds.
pub fn render_mut(f: &mut Frame, state: &mut AppState) {
    render_app_mut(f, state);
}

/// Renders all active workspace panes, auto-growing the Composer pane for multi-line input.
pub fn render_app(f: &mut Frame, state: &AppState) {
    let _ = render_app_inner(f, state);
}

struct RenderMetrics {
    thread_id: String,
    max_scroll: u16,
    tree_scroll: u16,
    worktree_scroll: usize,
    worktree_diff_scroll: u16,
    worktree_diff_max_scroll: u16,
    transcript_inner_height: u16,
    composer_scroll_y: u16,
    composer_inner_width: u16,
    help_scroll_offset: u16,
    help_max_scroll: u16,
    frame_area: Rect,
    terminal_inner_size: Option<(u16, u16)>,
    display_is_preview: bool,
    updated_search: Option<TranscriptSearchState>,
}

pub fn render_app_mut(f: &mut Frame, state: &mut AppState) {
    let metrics = render_app_inner(f, state);
    state
        .transcript_viewport_mut(&metrics.thread_id)
        .set_max_scroll(metrics.max_scroll as usize);
    state.tree.scroll_offset = metrics.tree_scroll;
    state.worktree_scroll_offset = metrics.worktree_scroll;
    state.worktree_diff_scroll = metrics.worktree_diff_scroll;
    state.worktree_diff_max_scroll = metrics.worktree_diff_max_scroll;
    state.transcript_viewport_height = metrics.transcript_inner_height;
    state.composer_scroll_y = metrics.composer_scroll_y;
    state.last_composer_inner_width = metrics.composer_inner_width;
    state.last_frame_area = Some(metrics.frame_area);
    if let Some(updated_search) = metrics.updated_search {
        state.transcript_search = updated_search;
    }
    if let Some((rows, cols)) = metrics.terminal_inner_size {
        if rows >= 1 && cols >= 1 {
            state.terminal_pane_size = (rows, cols);
            if !metrics.display_is_preview && state.active_is_terminal() {
                let active_id = state.active_thread_id.clone();
                if state.last_pty_sizes.get(&active_id).copied() != Some((rows, cols)) {
                    state.last_pty_sizes.insert(active_id.clone(), (rows, cols));
                    state.pending_actions.push_back(AppAction::PtyResize {
                        thread_id: active_id,
                        rows,
                        cols,
                    });
                }
            }
        }
    }
    if state.show_help_modal {
        state.help_scroll_offset = metrics.help_scroll_offset;
        state.help_max_scroll = metrics.help_max_scroll;
    } else {
        state.help_max_scroll = 0;
    }
}

fn render_app_inner(f: &mut Frame, state: &AppState) -> RenderMetrics {
    let theme = &state.theme_palette;
    if theme.bg != Color::Reset {
        f.render_widget(
            Block::default().style(Style::default().bg(theme.bg)),
            f.area(),
        );
    }

    let active_id = state.active_thread_id();
    let active_is_terminal = state.is_terminal_thread(active_id);
    let tree_focused = state.active_focus == PaneFocus::Tree
        || state.focused_pane == PaneFocus::Tree
        || state.focus == PaneFocus::Tree;
    let showing_worktrees_diff =
        !state.sidebar_hidden && state.sidebar_view == SidebarView::Worktrees;
    let preview =
        if tree_focused && !state.sidebar_hidden && state.sidebar_view == SidebarView::Grein {
            selected_tree_thread(state).filter(|(sel_id, _)| *sel_id != active_id)
        } else {
            None
        };
    let display_thread_id = preview.map(|(id, _)| id).unwrap_or(active_id);
    let display_is_terminal = state.is_terminal_thread(display_thread_id);
    let show_composer = (!active_is_terminal && !display_is_terminal && !showing_worktrees_diff)
        || state.pending_rename_thread_id.is_some();
    let active_subagents: &[SubagentSnapshot] = state
        .thread_subagents
        .get(active_id)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let strip_h = subagent_strip_height(active_subagents.len());
    let mut layout = compute_app_layout(state, f.area(), strip_h, !show_composer);

    let mut composer_inner_width = 0u16;
    let (composer_layout, visual_line_count) = if show_composer && layout.composer_area.height > 0
    {
        composer_inner_width = layout.composer_area.width.saturating_sub(2);
        let composer_layout = layout_composer_text(
            &state.input_buffer,
            state.cursor_position,
            composer_inner_width as usize,
        );
        let visual_line_count = composer_layout.lines.len().max(1) as u16;
        let max_composer_h = layout
            .transcript_area
            .height
            .saturating_add(layout.composer_area.height)
            .saturating_sub(3)
            .max(3);
        let desired_height = (visual_line_count + 2).clamp(3, 10).min(max_composer_h);
        if desired_height > layout.composer_area.height {
            let delta = desired_height - layout.composer_area.height;
            layout.transcript_area.height = layout.transcript_area.height.saturating_sub(delta);
            layout.composer_area.y = layout.composer_area.y.saturating_sub(delta);
            layout.composer_area.height = desired_height;
        }
        (Some(composer_layout), visual_line_count)
    } else {
        (None, 0)
    };

    let focused_debug = format!("{:?}", state.focused_pane());
    let transcript_focused = focused_debug.contains("Transcript");
    let terminal_focused = state.active_focus == PaneFocus::Terminal
        || state.focused_pane == PaneFocus::Terminal
        || state.focus == PaneFocus::Terminal;
    let composer_focused = show_composer
        && (focused_debug.contains("Composer")
            || focused_debug.contains("Input")
            || state.active_focus == PaneFocus::Composer
            || state.focused_pane == PaneFocus::Composer
            || state.focus == PaneFocus::Composer);

    let tree_result = if state.sidebar_hidden || layout.tree_area.width == 0 {
        TreeRenderResult {
            tree_scroll: state.tree.scroll_offset,
            worktree_scroll: state.worktree_scroll_offset,
            cursor_pos: None,
            overflow_line: None,
        }
    } else {
        render_tree_view_inner(f, layout.tree_area, state)
    };

    let mut max_scroll_u16 = 0u16;
    let mut transcript_inner_height = layout.transcript_area.height.saturating_sub(2);
    let mut terminal_inner_size = None;
    let mut updated_search: Option<TranscriptSearchState> = None;
    let mut worktree_diff_scroll = state.worktree_diff_scroll;
    let mut worktree_diff_max_scroll = state.worktree_diff_max_scroll;

    if showing_worktrees_diff {
        let (ws, wmax) = render_worktree_diff_pane(f, layout.transcript_area, state);
        worktree_diff_scroll = ws;
        worktree_diff_max_scroll = wmax;
    } else if display_is_terminal {
        terminal_inner_size = render_terminal_pane(
            f,
            layout.transcript_area,
            state,
            display_thread_id,
            preview.map(|(_, title)| title),
            terminal_focused,
        );
    } else {
        let transcript_borders = if state.sidebar_hidden {
            Borders::TOP | Borders::BOTTOM
        } else {
            Borders::ALL
        };
        let transcript_inner_width = if state.sidebar_hidden {
            layout.transcript_area.width
        } else {
            layout.transcript_area.width.saturating_sub(2)
        };
        transcript_inner_height = layout.transcript_area.height.saturating_sub(2);

        let wrapped_lines = build_transcript_wrapped_lines(
            state,
            display_thread_id,
            transcript_inner_width as usize,
        );
        updated_search = recompute_search_for_render(
            state,
            display_thread_id,
            &wrapped_lines,
            transcript_inner_height,
        );
        let wrapped_lines = if let Some(ref s) = updated_search {
            highlight_lines_with_matches(wrapped_lines, &s.matches, s.current_index)
        } else if state.transcript_search.is_visible() {
            highlight_lines_with_matches(
                wrapped_lines,
                &state.transcript_search.matches,
                state.transcript_search.current_index,
            )
        } else {
            wrapped_lines
        };

        let visual_lines = wrapped_lines.len();
        max_scroll_u16 = visual_lines
            .saturating_sub(transcript_inner_height as usize)
            .min(u16::MAX as usize) as u16;
        let vp = state.transcript_viewport(display_thread_id);
        let transcript_scroll_y = if vp.auto_scroll() {
            max_scroll_u16
        } else {
            (vp.scroll_offset() as u16).min(max_scroll_u16)
        };
        let transcript_title = match preview {
            Some((_, title)) => {
                format!(
                    " Transcript (Preview: {}) ",
                    state.display_title_for(display_thread_id, title)
                )
            }
            None => " Transcript ".to_string(),
        };
        let mut transcript_block = Block::default()
            .title(transcript_title)
            .borders(transcript_borders)
            .border_type(BorderType::Rounded)
            .border_style(themed_border_style(theme, transcript_focused));
        if theme.panel_bg != Color::Reset {
            transcript_block = transcript_block.style(Style::default().bg(theme.panel_bg));
        }
        let transcript_widget = Paragraph::new(wrapped_lines)
            .block(transcript_block)
            .scroll((transcript_scroll_y, 0));
        f.render_widget(transcript_widget, layout.transcript_area);
    }

    let mut composer_scroll_y = state.composer_scroll_y;
    if let Some(composer_layout) = composer_layout {
        let composer_inner_width = layout.composer_area.width.saturating_sub(2);
        let composer_block = rounded_block(theme, composer_focused).title(" Composer ");

        let inner_height = layout.composer_area.height.saturating_sub(2).max(1);
        let max_composer_scroll = visual_line_count.saturating_sub(inner_height);
        composer_scroll_y = state.composer_scroll_y.min(max_composer_scroll);
        if composer_layout.cursor_row < composer_scroll_y {
            composer_scroll_y = composer_layout.cursor_row;
        } else if composer_layout.cursor_row >= composer_scroll_y + inner_height {
            composer_scroll_y = composer_layout.cursor_row + 1 - inner_height;
        }

        let composer_widget = Paragraph::new(composer_layout.lines)
            .block(composer_block)
            .scroll((composer_scroll_y, 0));
        f.render_widget(composer_widget, layout.composer_area);

        if composer_focused && !state.has_active_modal() && !state.transcript_search.editing {
            let visible_row = composer_layout.cursor_row.saturating_sub(composer_scroll_y);
            let cursor_x = layout.composer_area.x
                + 1
                + composer_layout
                    .cursor_col
                    .min(composer_inner_width.saturating_sub(1));
            let cursor_y =
                layout.composer_area.y + 1 + visible_row.min(inner_height.saturating_sub(1));
            f.set_cursor_position(Position::new(cursor_x, cursor_y));
        }

        if composer_focused && state.worktree_autocomplete_open && !state.has_active_modal() {
            render_worktree_autocomplete_popup(f, layout.composer_area, state);
        }
    }

    // 1-row MANAGE hover overflow overlay across sidebar divider (never resizes PTY/layout)
    if tree_focused
        && !state.sidebar_hidden
        && !state.has_active_modal()
        && !state.palette_active
        && state.rename_prompt.is_none()
        && state.sidebar_layout.overflow
    {
        if let Some((ov_x, ov_y, ov_line)) = tree_result.overflow_line {
            let max_x = f.area().x + f.area().width;
            let avail_w = max_x.saturating_sub(ov_x);
            let desired_w = (ov_line.width() as u16 + 1).min(avail_w);
            if desired_w > 0 && ov_y < f.area().y + f.area().height {
                let ov_area = Rect::new(ov_x, ov_y, desired_w, 1);
                f.render_widget(
                    Paragraph::new(ov_line).style(Style::default().bg(theme.selection_bg)),
                    ov_area,
                );
            }
        }
    }

    if let Some(strip_area) = layout.subagent_strip_area {
        render_subagent_strip(f, strip_area, active_subagents, theme);
    }

    let effective_search = updated_search.as_ref().unwrap_or(&state.transcript_search);
    render_status_bar(
        f,
        layout.status_bar_area,
        state,
        active_subagents.len(),
        effective_search,
    );

    if effective_search.editing
        && !state.has_active_modal()
        && layout.status_bar_area.width > 1
        && layout.status_bar_area.height > 0
    {
        let query_w = Span::raw(&effective_search.query).width() as u16;
        let pill_w = if tree_focused {
            match state.sidebar_view {
                SidebarView::Grein => 17u16,
                SidebarView::Worktrees => 21u16,
            }
        } else {
            16u16
        };
        let cursor_x = layout.status_bar_area.x
            + (pill_w + 1 + query_w).min(layout.status_bar_area.width.saturating_sub(1));
        f.set_cursor_position(Position::new(cursor_x, layout.status_bar_area.y));
    } else if let Some(filter_pos) = tree_result.cursor_pos {
        if !state.has_active_modal() {
            f.set_cursor_position(filter_pos);
        }
    }

    let mut rendered_help_scroll = state.help_scroll_offset;
    let mut help_max_scroll = state.help_max_scroll;
    if let Some(ref perm) = state.pending_permission {
        render_permission_modal(f, f.area(), perm, theme);
    } else if let Some(ref pending) = state.pending_worktree_delete {
        render_worktree_delete_modal(f, f.area(), pending, theme);
    } else if let Some(ref pending) = state.pending_thread_delete {
        render_thread_delete_modal(f, f.area(), pending, theme);
    } else if let Some(ref pending) = state.pending_quit_confirm {
        render_quit_confirm_modal(f, f.area(), pending, theme);
    } else if let Some(ref picker) = state.theme_picker {
        render_theme_picker_modal(f, f.area(), picker, state);
    } else if let Some(ref picker) = state.config_picker {
        render_config_picker_modal(f, f.area(), picker, theme);
    } else if state.show_help_modal {
        let (scroll, max_s) = render_help_modal(f, f.area(), state);
        rendered_help_scroll = scroll;
        help_max_scroll = max_s;
    }

    RenderMetrics {
        thread_id: display_thread_id.to_string(),
        max_scroll: max_scroll_u16,
        tree_scroll: tree_result.tree_scroll,
        worktree_scroll: tree_result.worktree_scroll,
        worktree_diff_scroll,
        worktree_diff_max_scroll,
        transcript_inner_height,
        composer_scroll_y,
        composer_inner_width,
        help_scroll_offset: rendered_help_scroll,
        help_max_scroll,
        frame_area: f.area(),
        terminal_inner_size,
        display_is_preview: preview.is_some(),
        updated_search,
    }
}
