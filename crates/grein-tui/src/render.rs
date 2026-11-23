use std::collections::HashMap;

use crate::app::{
    worktree_display_label, AppState, ConfigPickerModal, PaneFocus, PendingWorktreeDelete,
    SidebarView, WorktreeAutocompleteItem, WorktreeRow,
};
use crate::engine_port::{PtyCell, PtyColor, SubagentSnapshot, SubagentStatus};
use crate::layout::{compute_layout_with_options, subagent_strip_height};
use crate::runner::{compute_ordered_thread_entries_with_folds, compute_ordered_thread_indices};
use crate::search::{highlight_lines_with_matches, SearchDirection, TranscriptSearchState};
use crate::ui::tree::{compute_tree_scroll_offset, TreeNode};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Returns the border style for a pane depending on whether it currently holds focus.
pub fn border_style(is_focused: bool) -> Style {
    if is_focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

pub fn render_markdown_lines(markdown: &str) -> Vec<Line<'static>> {
    crate::markdown::render_markdown_lines(markdown)
}

fn thread_status_glyph(status: Option<&str>, depth: usize) -> Option<(&'static str, Color)> {
    match status.unwrap_or("idle") {
        "streaming" | "executing" | "thinking" => Some(("●", Color::Yellow)),
        "permission" | "permission_required" | "error" => Some(("⚠", Color::Red)),
        _ if depth == 0 => Some(("ᛉ", Color::Green)),
        _ => None,
    }
}

fn selected_tree_thread(state: &AppState) -> Option<(&str, &str)> {
    let idx = state.tree.selected_index()?;
    if !state.tree.nodes.is_empty() {
        match state.tree.nodes.get(idx)? {
            TreeNode::Thread(t) => Some((t.id.as_str(), t.title.as_str())),
            TreeNode::Anchor(a) => state
                .threads
                .iter()
                .find(|t| t.id == a.thread_id)
                .map(|t| (t.id.as_str(), t.title.as_str())),
        }
    } else {
        let ordered = compute_ordered_thread_indices(&state.threads, &state.thread_meta);
        let &(thread_idx, _) = ordered.get(idx)?;
        let t = state.threads.get(thread_idx)?;
        Some((t.id.as_str(), t.title.as_str()))
    }
}

fn char_display_width(ch: char) -> usize {
    if ch.is_ascii() {
        if ch.is_control() {
            0
        } else {
            1
        }
    } else {
        let mut buf = [0u8; 4];
        Span::raw(&*ch.encode_utf8(&mut buf)).width()
    }
}

fn clamp_char_boundary(text: &str, pos: usize) -> usize {
    let mut idx = pos.min(text.len());
    while idx > 0 && !text.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

struct ComposerTextLayout {
    lines: Vec<Line<'static>>,
    cursor_col: u16,
    cursor_row: u16,
}

fn layout_composer_text(input: &str, cursor_pos: usize, inner_width: usize) -> ComposerTextLayout {
    if inner_width == 0 {
        return ComposerTextLayout {
            lines: vec![Line::raw("")],
            cursor_col: 0,
            cursor_row: 0,
        };
    }

    let cursor_byte = clamp_char_boundary(input, cursor_pos);
    let mut out_lines: Vec<Line<'static>> = Vec::new();
    let mut cursor_col = 0usize;
    let mut cursor_row = 0usize;
    let mut cursor_set = false;
    let mut line_start = 0usize;

    for logical_line in input.split('\n') {
        let mut cur_line = String::new();
        let mut cur_w = 0usize;
        let mut idx = 0usize;
        let bytes = logical_line.as_bytes();

        while idx < bytes.len() {
            let ch = logical_line[idx..].chars().next().unwrap();
            if ch.is_whitespace() {
                let byte_idx = line_start + idx;
                let cw = if ch == '\t' { 1 } else { char_display_width(ch) };
                if cur_w > 0 && cur_w + cw > inner_width {
                    out_lines.push(Line::from(std::mem::take(&mut cur_line)));
                    cur_w = 0;
                }
                if byte_idx == cursor_byte && !cursor_set {
                    cursor_col = cur_w;
                    cursor_row = out_lines.len();
                    cursor_set = true;
                }
                cur_line.push(if ch == '\t' { ' ' } else { ch });
                cur_w += cw;
                idx += ch.len_utf8();
            } else {
                let word_start = idx;
                let mut word_w = 0usize;
                while idx < bytes.len() {
                    let wch = logical_line[idx..].chars().next().unwrap();
                    if wch.is_whitespace() {
                        break;
                    }
                    word_w += char_display_width(wch);
                    idx += wch.len_utf8();
                }
                let word = &logical_line[word_start..idx];
                if cur_w > 0 && cur_w + word_w > inner_width && word_w <= inner_width {
                    out_lines.push(Line::from(std::mem::take(&mut cur_line)));
                    cur_w = 0;
                }
                for (rel_idx, wch) in word.char_indices() {
                    let byte_idx = line_start + word_start + rel_idx;
                    let cw = char_display_width(wch);
                    if cur_w > 0 && cur_w + cw > inner_width {
                        out_lines.push(Line::from(std::mem::take(&mut cur_line)));
                        cur_w = 0;
                    }
                    if byte_idx == cursor_byte && !cursor_set {
                        cursor_col = cur_w;
                        cursor_row = out_lines.len();
                        cursor_set = true;
                    }
                    cur_line.push(wch);
                    cur_w += cw;
                }
            }
        }

        let line_end = line_start + logical_line.len();
        if line_end == cursor_byte && !cursor_set {
            if cur_w >= inner_width {
                out_lines.push(Line::from(std::mem::take(&mut cur_line)));
                cur_w = 0;
            }
            cursor_col = cur_w;
            cursor_row = out_lines.len();
            cursor_set = true;
        }
        out_lines.push(Line::from(cur_line));
        line_start = line_end + 1;
    }

    if out_lines.is_empty() {
        out_lines.push(Line::raw(""));
    }

    ComposerTextLayout {
        lines: out_lines,
        cursor_col: cursor_col.min(u16::MAX as usize) as u16,
        cursor_row: cursor_row.min(u16::MAX as usize) as u16,
    }
}

fn render_thread_row(
    id: &str,
    title: &str,
    depth: usize,
    prefix: &str,
    hidden_descendants: usize,
    is_selected: bool,
    state: &AppState,
) -> Line<'static> {
    let is_active = id == state.active_thread_id;
    let is_merged = state.thread_meta.get(id).is_some_and(|m| m.merged);
    let is_terminal = state.is_terminal_thread(id);
    let exit_code_opt = state.terminal_exit_code(id);
    let glyph_opt = if is_terminal {
        let color = match exit_code_opt {
            Some(Some(c)) if c != 0 => Color::Red,
            Some(_) => Color::DarkGray,
            None => Color::Cyan,
        };
        Some((">_", color))
    } else {
        thread_status_glyph(state.thread_statuses.get(id).map(String::as_str), depth)
    };
    let exit_badge = exit_code_opt.map(|code_opt| {
        let label = match code_opt {
            Some(c) => format!(" [exit {c}]"),
            None => " [exited]".to_string(),
        };
        let color = if code_opt.is_some_and(|c| c != 0) {
            Color::Red
        } else {
            Color::DarkGray
        };
        (label, color)
    });
    let fold_prefix = if hidden_descendants > 0 { "▸ " } else { "" };
    let fold_badge = (hidden_descendants > 0).then(|| format!(" (+{hidden_descendants})"));

    if is_selected {
        let style = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED);
        let label = match glyph_opt {
            Some((glyph, _)) => format!("{prefix}{fold_prefix}{glyph} {title}"),
            None => format!("{prefix}{fold_prefix}{title}"),
        };
        let mut spans = vec![Span::styled(label, style)];
        if let Some(badge) = fold_badge {
            spans.push(Span::styled(
                badge,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::REVERSED),
            ));
        }
        if let Some((badge, color)) = exit_badge {
            spans.push(Span::styled(
                badge,
                Style::default().fg(color).add_modifier(Modifier::REVERSED),
            ));
        }
        if is_merged {
            spans.push(Span::styled(
                " [joined]",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::REVERSED),
            ));
        }
        Line::from(spans)
    } else {
        let title_style = if is_active {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        };
        let mut spans = vec![Span::styled(
            prefix.to_string(),
            Style::default().fg(Color::DarkGray),
        )];
        if !fold_prefix.is_empty() {
            spans.push(Span::styled(
                fold_prefix,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        if let Some((glyph, glyph_color)) = glyph_opt {
            spans.push(Span::styled(
                format!("{glyph} "),
                Style::default().fg(glyph_color),
            ));
        }
        spans.push(Span::styled(title.to_string(), title_style));
        if let Some(badge) = fold_badge {
            spans.push(Span::styled(badge, Style::default().fg(Color::DarkGray)));
        }
        if let Some((badge, color)) = exit_badge {
            spans.push(Span::styled(badge, Style::default().fg(color)));
        }
        if is_merged {
            spans.push(Span::styled(
                " [joined]",
                Style::default().fg(Color::DarkGray),
            ));
        }
        Line::from(spans)
    }
}

fn render_sidebar_title(state: &AppState) -> Line<'static> {
    let active_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let dim_style = Style::default().fg(Color::DarkGray);
    let (grein_style, wt_style) = match state.sidebar_view {
        SidebarView::Grein => (active_style, dim_style),
        SidebarView::Worktrees => (dim_style, active_style),
    };
    let mut spans = vec![
        Span::raw(" "),
        Span::styled("ᛇ Grein", grein_style),
        Span::styled(" │ ", dim_style),
        Span::styled("⎇ Worktrees", wt_style),
    ];
    if state.sidebar_view == SidebarView::Worktrees && !state.worktree_filter_query.is_empty() {
        spans.push(Span::styled(
            format!(" /{}", state.worktree_filter_query),
            Style::default().fg(Color::Yellow),
        ));
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}

fn render_worktree_row(
    row: &WorktreeRow,
    is_selected: bool,
    active_worktree_path: Option<&str>,
) -> Line<'static> {
    match row {
        WorktreeRow::RepoHeader {
            repo_name,
            count,
            collapsed,
            ..
        } => {
            let arrow = if *collapsed { "▸" } else { "▾" };
            let text = format!("{arrow} {repo_name} ({count})");
            let style = if is_selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default().add_modifier(Modifier::BOLD)
            };
            Line::from(Span::styled(text, style))
        }
        WorktreeRow::Entry(entry) => {
            let is_active = active_worktree_path == Some(entry.path.as_str());
            let label = worktree_display_label(entry);
            if is_selected {
                let sel_style = Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED);
                let mut spans = vec![Span::styled("  ", sel_style)];
                if is_active {
                    spans.push(Span::styled(
                        "● ",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD | Modifier::REVERSED),
                    ));
                } else {
                    spans.push(Span::styled("  ", sel_style));
                }
                spans.push(Span::styled(label, sel_style));
                if entry.is_primary {
                    spans.push(Span::styled(
                        " (primary)",
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::REVERSED),
                    ));
                }
                if entry.dirty_count > 0 {
                    spans.push(Span::styled(
                        format!(" *{}", entry.dirty_count),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::REVERSED),
                    ));
                }
                if !entry.attached_thread_ids.is_empty() {
                    spans.push(Span::styled(
                        format!(" [{}]", entry.attached_thread_ids.len()),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::REVERSED),
                    ));
                }
                Line::from(spans)
            } else {
                let label_style = if is_active {
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                let mut spans = vec![Span::raw("  ")];
                if is_active {
                    spans.push(Span::styled(
                        "● ",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ));
                } else {
                    spans.push(Span::raw("  "));
                }
                spans.push(Span::styled(label, label_style));
                if entry.is_primary {
                    spans.push(Span::styled(
                        " (primary)",
                        Style::default().fg(Color::DarkGray),
                    ));
                }
                if entry.dirty_count > 0 {
                    spans.push(Span::styled(
                        format!(" *{}", entry.dirty_count),
                        Style::default().fg(Color::Yellow),
                    ));
                }
                if !entry.attached_thread_ids.is_empty() {
                    spans.push(Span::styled(
                        format!(" [{}]", entry.attached_thread_ids.len()),
                        Style::default().fg(Color::DarkGray),
                    ));
                }
                Line::from(spans)
            }
        }
    }
}

fn render_worktrees_view(
    f: &mut Frame,
    area: Rect,
    state: &AppState,
    tree_focused: bool,
) -> (usize, Option<Position>) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style(tree_focused))
        .title(render_sidebar_title(state));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return (state.worktree_scroll_offset, None);
    }

    let show_filter_bar = state.worktree_filter_active || !state.worktree_filter_query.is_empty();
    let filter_h = u16::from(show_filter_bar && inner.height >= 2);
    let mut filter_cursor = None;

    if filter_h == 1 {
        let hint = if state.worktree_filter_active {
            " (Enter/Esc)"
        } else {
            " (Esc clear)"
        };
        let filter_line = Line::from(vec![
            Span::styled(
                "/",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(state.worktree_filter_query.clone()),
            Span::styled(hint, Style::default().fg(Color::DarkGray)),
        ]);
        f.render_widget(
            Paragraph::new(filter_line),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        if state.worktree_filter_active
            && tree_focused
            && !state.has_active_modal()
            && !state.transcript_search.editing
        {
            let q_w = Span::raw(&state.worktree_filter_query).width() as u16;
            let cursor_x = inner.x + (1 + q_w).min(inner.width.saturating_sub(1));
            filter_cursor = Some(Position::new(cursor_x, inner.y));
        }
    }

    let list_area = Rect::new(
        inner.x,
        inner.y + filter_h,
        inner.width,
        inner.height.saturating_sub(filter_h),
    );
    let rows = state.visible_worktree_rows();
    let active_path = state.active_worktree_path();
    let (lines, selected_line_idx) = if rows.is_empty() {
        let msg = if state.worktree_list_in_flight {
            "  Loading worktrees..."
        } else if !state.worktree_filter_query.is_empty() {
            "  No matching worktrees"
        } else {
            "  No git worktrees found"
        };
        (
            vec![Line::from(Span::styled(
                msg,
                Style::default().fg(Color::DarkGray),
            ))],
            None,
        )
    } else {
        let sel = state.worktree_selected_idx.min(rows.len() - 1);
        let built = rows
            .iter()
            .enumerate()
            .map(|(idx, row)| render_worktree_row(row, idx == sel, active_path.as_deref()))
            .collect();
        (built, Some(sel))
    };

    let scroll_offset = compute_tree_scroll_offset(
        state.worktree_scroll_offset.min(u16::MAX as usize) as u16,
        selected_line_idx,
        lines.len(),
        list_area.height,
    );
    f.render_widget(
        Paragraph::new(lines).scroll((scroll_offset, 0)),
        list_area,
    );
    (scroll_offset as usize, filter_cursor)
}

pub fn render_tree_view(f: &mut Frame, area: Rect, state: &AppState) {
    let _ = render_tree_view_inner(f, area, state);
}

struct TreeRenderResult {
    tree_scroll: u16,
    worktree_scroll: usize,
    filter_cursor: Option<Position>,
}

fn render_tree_view_inner(f: &mut Frame, area: Rect, state: &AppState) -> TreeRenderResult {
    let tree_focused = state.active_focus == PaneFocus::Tree
        || state.focused_pane == PaneFocus::Tree
        || state.focus == PaneFocus::Tree;

    if state.sidebar_view == SidebarView::Worktrees {
        let (worktree_scroll, filter_cursor) = render_worktrees_view(f, area, state, tree_focused);
        return TreeRenderResult {
            tree_scroll: state.tree.scroll_offset,
            worktree_scroll,
            filter_cursor,
        };
    }

    let mut lines = Vec::new();
    let selected = state.tree.selected_index();
    let mut selected_line_idx: Option<usize> = None;

    if !state.tree.nodes.is_empty() {
        let fallback_layout: Option<HashMap<&str, (usize, String, usize)>> =
            if state.tree.prefixes.is_empty() && !state.threads.is_empty() {
                Some(
                    compute_ordered_thread_entries_with_folds(
                        &state.threads,
                        &state.thread_meta,
                        &state.collapsed_threads,
                    )
                    .into_iter()
                    .map(|e| {
                        (
                            state.threads[e.index].id.as_str(),
                            (e.depth, e.prefix, e.hidden_descendants),
                        )
                    })
                    .collect(),
                )
            } else {
                None
            };

        for (node_idx, node) in state.tree.nodes.iter().enumerate() {
            if let TreeNode::Thread(t) = node {
                let is_selected = selected == Some(node_idx);
                if is_selected {
                    selected_line_idx = Some(lines.len());
                }
                let (depth, prefix, hidden_descendants) = if let Some(ref map) = fallback_layout {
                    map.get(t.id.as_str())
                        .map(|(d, p, h)| (*d, p.as_str(), *h))
                        .unwrap_or((
                            state.tree.depth_for(&t.id),
                            state.tree.prefix_for(&t.id),
                            state.tree_hidden_counts.get(&t.id).copied().unwrap_or(0),
                        ))
                } else {
                    (
                        state.tree.depth_for(&t.id),
                        state.tree.prefix_for(&t.id),
                        state.tree_hidden_counts.get(&t.id).copied().unwrap_or(0),
                    )
                };
                lines.push(render_thread_row(
                    &t.id,
                    &t.title,
                    depth,
                    prefix,
                    hidden_descendants,
                    is_selected,
                    state,
                ));
            }
        }
    } else if !state.threads.is_empty() {
        let ordered = compute_ordered_thread_entries_with_folds(
            &state.threads,
            &state.thread_meta,
            &state.collapsed_threads,
        );
        for (row_idx, entry) in ordered.into_iter().enumerate() {
            let thread = &state.threads[entry.index];
            let is_selected = selected == Some(row_idx);
            if is_selected {
                selected_line_idx = Some(lines.len());
            }
            lines.push(render_thread_row(
                &thread.id,
                &thread.title,
                entry.depth,
                &entry.prefix,
                entry.hidden_descendants,
                is_selected,
                state,
            ));
        }
    }

    let inner_height = area.height.saturating_sub(2);
    let scroll_offset = compute_tree_scroll_offset(
        state.tree.scroll_offset,
        selected_line_idx,
        lines.len(),
        inner_height,
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style(tree_focused))
        .title(render_sidebar_title(state));
    let paragraph = Paragraph::new(lines)
        .block(block)
        .scroll((scroll_offset, 0));
    f.render_widget(paragraph, area);
    TreeRenderResult {
        tree_scroll: scroll_offset,
        worktree_scroll: state.worktree_scroll_offset,
        filter_cursor: None,
    }
}

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
    transcript_inner_height: u16,
    composer_scroll_y: u16,
    help_scroll_offset: u16,
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
    state.transcript_viewport_height = metrics.transcript_inner_height;
    state.composer_scroll_y = metrics.composer_scroll_y;
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
                    state.pending_actions.push_back(crate::app::AppAction::PtyResize {
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
    }
}

fn format_elapsed_ms(started_at_ms: u64, now_ms: u64) -> String {
    if started_at_ms == 0 {
        return "0s".to_string();
    }
    let secs = now_ms.saturating_sub(started_at_ms) / 1000;
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

fn truncate_single_line(text: &str, max_chars: usize) -> String {
    let sanitized: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut chars = sanitized.chars();
    let collected: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() && max_chars > 1 {
        let mut trimmed: String = collected.chars().take(max_chars - 1).collect();
        trimmed.push('…');
        trimmed
    } else {
        collected
    }
}

fn render_subagent_strip(f: &mut Frame, area: Rect, subagents: &[SubagentSnapshot]) {
    let now_ms = crate::runner::now_epoch_ms();
    let mut lines: Vec<Line<'static>> = Vec::new();
    for sa in subagents.iter().take(5) {
        let (glyph, color, default_act) = match sa.status {
            SubagentStatus::Running => ("●", Color::Yellow, "working"),
            SubagentStatus::Completed => ("✓", Color::Green, "done"),
            SubagentStatus::Failed => ("✖", Color::Red, "failed"),
        };
        let activity = sa
            .activity
            .as_deref()
            .filter(|a| !a.trim().is_empty())
            .unwrap_or(default_act);
        let elapsed = format_elapsed_ms(sa.started_at_ms, now_ms);

        let mut spans = vec![
            Span::styled(
                format!("{glyph} "),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("Agent({}: {})", sa.agent_type, sa.role),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ];
        if let Some(ref model) = sa.model {
            if !model.trim().is_empty() {
                spans.push(Span::styled(
                    format!(" · {model}"),
                    Style::default().fg(Color::DarkGray),
                ));
            }
        }
        spans.push(Span::raw(format!("  {activity} · {elapsed}")));
        lines.push(Line::from(spans));
    }

    if subagents.len() > 5 {
        let overflow = subagents.len() - 5;
        lines.push(Line::from(Span::styled(
            format!("+{overflow} more"),
            Style::default().fg(Color::DarkGray),
        )));
    }

    let block = Block::default()
        .title(" Subagents ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let widget = Paragraph::new(lines).block(block);
    f.render_widget(widget, area);
}

fn short_cwd_label(cwd: &str) -> String {
    let trimmed = cwd.trim();
    if trimmed == "/" {
        return "/".to_string();
    }
    std::path::Path::new(trimmed)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .map(|s| truncate_single_line(s, 28))
        .unwrap_or_else(|| truncate_single_line(trimmed, 28))
}

fn render_status_bar(
    f: &mut Frame,
    area: Rect,
    state: &AppState,
    subagent_count: usize,
    search: &TranscriptSearchState,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let active_id = state.active_thread_id();
    let active_is_terminal = state.is_terminal_thread(active_id);
    let status = state
        .thread_statuses
        .get(active_id)
        .map(String::as_str)
        .unwrap_or("idle");
    let active_tool = state
        .thread_active_tool
        .get(active_id)
        .and_then(|t| t.as_deref())
        .filter(|t| !t.trim().is_empty());
    let thought = state
        .thread_thoughts
        .get(active_id)
        .map(String::as_str)
        .filter(|t| !t.trim().is_empty());

    let is_busy = !active_is_terminal
        && (matches!(status, "streaming" | "executing" | "thinking")
            || active_tool.is_some()
            || thought.is_some());

    let terminal_focused = state.active_focus == PaneFocus::Terminal
        || state.focused_pane == PaneFocus::Terminal
        || state.focus == PaneFocus::Terminal;

    let (mode, model, live_status, status_color) = if active_is_terminal {
        let raw_cwd = state
            .pty_screens
            .get(active_id)
            .and_then(|s| s.cwd.as_deref())
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                state
                    .thread_meta
                    .get(active_id)
                    .and_then(|m| m.cwd.as_deref())
                    .filter(|s| !s.trim().is_empty())
            });
        let cwd_label = raw_cwd
            .map(short_cwd_label)
            .unwrap_or_else(|| "pty".to_string());
        let (st, col) = match state.terminal_exit_code(active_id) {
            Some(Some(0)) => ("✓ exit 0".to_string(), Color::DarkGray),
            Some(Some(code)) => (format!("✖ exit {code}"), Color::Red),
            Some(None) => ("✓ exited".to_string(), Color::DarkGray),
            None => ("● running".to_string(), Color::Green),
        };
        ("terminal".to_string(), cwd_label, st, col)
    } else {
        let (model_opt, mode_opt) = state
            .thread_agent_meta
            .get(active_id)
            .map(|(m, md)| (m.as_deref(), md.as_deref()))
            .unwrap_or((None, None));
        let mode = mode_opt
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("agent")
            .to_string();
        let model = model_opt
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("default")
            .to_string();

        let spin = SPINNER_FRAMES[(state.tick_count as usize) % SPINNER_FRAMES.len()];
        let (live_status, status_color) = if let Some(tool) = active_tool {
            (format!("{spin} tool: {tool}"), Color::Yellow)
        } else if let Some(th) = thought {
            let preview = truncate_single_line(th, 24);
            (format!("{spin} thinking: {preview}"), Color::Yellow)
        } else {
            match status {
                "thinking" => (format!("{spin} thinking"), Color::Yellow),
                "executing" => (format!("{spin} executing"), Color::Yellow),
                "streaming" => (format!("{spin} streaming"), Color::Yellow),
                "error" => ("⚠ error".to_string(), Color::Red),
                "permission" | "permission_required" => ("⚠ permission".to_string(), Color::Red),
                _ => ("✓ idle".to_string(), Color::Green),
            }
        };
        (mode, model, live_status, status_color)
    };

    let sa_label = if subagent_count == 1 {
        "1 subagent".to_string()
    } else {
        format!("{subagent_count} subagents")
    };

    let git_meta = state
        .thread_meta
        .get(active_id)
        .and_then(|m| {
            m.git_branch
                .as_deref()
                .map(str::trim)
                .filter(|b| !b.is_empty())
                .map(|b| (b, m.git_dirty))
        });
    let git_badge = git_meta.map(|(branch, dirty)| {
        let text = if dirty > 0 {
            format!("⎇ {branch}*")
        } else {
            format!("⎇ {branch}")
        };
        let color = if dirty > 0 {
            Color::Yellow
        } else {
            Color::Cyan
        };
        (text, color)
    });

    let right_prefix = format!("{mode} · {model} · ");
    let right_suffix = format!(" · {sa_label}");
    let git_prefix_chars = git_badge
        .as_ref()
        .map_or(0, |(badge, _)| badge.chars().count() + 3);
    let total_right_chars = git_prefix_chars
        + right_prefix.chars().count()
        + live_status.chars().count()
        + right_suffix.chars().count();

    let scrollback_badge = if active_is_terminal {
        state
            .pty_screens
            .get(active_id)
            .filter(|s| s.scrollback_offset > 0)
            .map(|s| format!(" · [scroll -{}]", s.scrollback_offset))
            .unwrap_or_default()
    } else {
        String::new()
    };

    let notice_suffix = state
        .status_notice
        .as_deref()
        .map(|n| format!(" · {n}"))
        .unwrap_or_default();

    let mut spans = Vec::new();
    let total_left_chars = if search.is_visible() {
        let prefix = match search.direction {
            SearchDirection::Forward => '/',
            SearchDirection::Reverse => '?',
        };
        let query_part = format!("{prefix}{}", search.query);
        let counter_part = format!(" {}", search.counter_label());
        let search_hint = if search.editing {
            " · Enter/Esc"
        } else {
            " · n/N · Esc"
        };
        let counter_color = if search.matches.is_empty() && !search.query.is_empty() {
            Color::Red
        } else {
            Color::Yellow
        };
        spans.push(Span::styled(
            query_part.clone(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            counter_part.clone(),
            Style::default().fg(counter_color),
        ));
        let mut left_len = query_part.chars().count() + counter_part.chars().count();
        if left_len + search_hint.chars().count() + 1 + total_right_chars <= area.width as usize {
            spans.push(Span::styled(
                search_hint,
                Style::default().fg(Color::DarkGray),
            ));
            left_len += search_hint.chars().count();
        }
        if !scrollback_badge.is_empty() {
            spans.push(Span::styled(
                scrollback_badge.clone(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
            left_len += scrollback_badge.chars().count();
        }
        left_len
    } else {
        let left_hint = if active_is_terminal && terminal_focused {
            if state.terminal_exit_code(active_id).is_some() {
                "exited · Enter restart · Esc/Ctrl+G tree".to_string()
            } else if state.status_notice.is_some() {
                "Ctrl+G for tree".to_string()
            } else {
                "Ctrl+G for tree · Ctrl+T wt · Alt+Z zoom".to_string()
            }
        } else if is_busy {
            format!(
                "Esc cancel · Ctrl+G tree · Alt+T {}",
                crate::app::active_shell_label()
            )
        } else if state.focused_pane == PaneFocus::Tree {
            match state.sidebar_view {
                SidebarView::Grein => {
                    "t/T term · a agent · w worktree · / search · F1 help".to_string()
                }
                SidebarView::Worktrees => {
                    "Enter attach · t term · w/n new · d/x del · / filter · Ctrl+T tree"
                        .to_string()
                }
            }
        } else if state.status_notice.is_some() {
            format!(
                "Ctrl+G tree · Alt+T {} · F1 help",
                crate::app::active_shell_label()
            )
        } else {
            format!(
                "Ctrl+G tree · Alt+T {} · F1 help · Ctrl+T wt · Alt+Z zoom",
                crate::app::active_shell_label()
            )
        };
        let len = left_hint.chars().count()
            + scrollback_badge.chars().count()
            + notice_suffix.chars().count();
        spans.push(Span::styled(
            left_hint,
            Style::default().fg(Color::DarkGray),
        ));
        if !scrollback_badge.is_empty() {
            spans.push(Span::styled(
                scrollback_badge,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        if !notice_suffix.is_empty() {
            spans.push(Span::styled(
                notice_suffix,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        len
    };

    let width = area.width as usize;
    let gap = width.saturating_sub(total_left_chars + total_right_chars);

    if total_left_chars + 1 + total_right_chars <= width {
        spans.push(Span::raw(" ".repeat(gap.max(1))));
        if let Some((badge, badge_color)) = git_badge {
            spans.push(Span::styled(badge, Style::default().fg(badge_color)));
            spans.push(Span::styled(" · ", Style::default().fg(Color::DarkGray)));
        }
        spans.push(Span::styled(
            right_prefix,
            Style::default().fg(Color::DarkGray),
        ));
        spans.push(Span::styled(
            live_status,
            Style::default().fg(status_color),
        ));
        spans.push(Span::styled(
            right_suffix,
            Style::default().fg(Color::DarkGray),
        ));
    } else if total_left_chars < width {
        let remaining = width - total_left_chars;
        if remaining >= 2 {
            spans.push(Span::raw(" "));
            let compact_git = git_badge
                .as_ref()
                .map(|(badge, _)| format!("{badge} · "))
                .unwrap_or_default();
            let compact_right = truncate_single_line(
                &format!("{compact_git}{right_prefix}{live_status}{right_suffix}"),
                remaining - 1,
            );
            spans.push(Span::styled(
                compact_right,
                Style::default().fg(Color::DarkGray),
            ));
        }
    }

    let widget = Paragraph::new(Line::from(spans));
    f.render_widget(widget, area);
}

fn render_config_picker_modal(f: &mut Frame, area: Rect, picker: &ConfigPickerModal) {
    let popup_width = area.width.saturating_sub(6).clamp(32, 74).min(area.width);
    let content_rows = picker.options.len() + 3;
    let popup_height = (content_rows as u16 + 2)
        .min(area.height.saturating_sub(2).max(7))
        .min(area.height.max(1));
    let popup_x = area.x + area.width.saturating_sub(popup_width) / 2;
    let popup_y = area.y + area.height.saturating_sub(popup_height) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    f.render_widget(Clear, popup_area);
    let block = Block::default()
        .title(format!(" Select {} ", picker.title))
        .borders(Borders::ALL)
        .border_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let scope_style = if picker.set_default {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    };
    let scope_label = if picker.set_default {
        "Default for new branches"
    } else {
        "This branch only"
    };
    let scope_alt = if picker.set_default {
        "This branch only"
    } else {
        "Default for new branches"
    };
    let scope_line = Line::from(vec![
        Span::styled(" Scope: ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!("[{scope_label}]"), scope_style),
        Span::styled(
            format!("  (Tab/d: {scope_alt})"),
            Style::default().fg(Color::DarkGray),
        ),
    ]);

    let footer_line = Line::from(Span::styled(
        " ↑/↓/j/k Move · Enter Apply · Tab/d Scope · Esc/q Close",
        Style::default().fg(Color::DarkGray),
    ));

    let opt_height = inner.height.saturating_sub(2) as usize;
    let mut opt_lines = Vec::with_capacity(picker.options.len());
    for (idx, choice) in picker.options.iter().enumerate() {
        let is_selected = idx == picker.selected_index;
        let is_active = choice.value == picker.current_value;
        let marker = if is_selected { " ▶ " } else { "   " };
        let label = if !choice.name.is_empty() && choice.name != choice.value {
            format!("{} ({})", choice.name, choice.value)
        } else {
            choice.value.clone()
        };
        let desc = choice
            .description
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty());

        if is_selected {
            let sel_style = Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED);
            let mut spans = vec![Span::styled(marker, sel_style)];
            if is_active {
                spans.push(Span::styled(
                    "✓ ",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED),
                ));
            } else {
                spans.push(Span::styled("  ", sel_style));
            }
            spans.push(Span::styled(label, sel_style));
            if let Some(d) = desc {
                spans.push(Span::styled(format!(" — {d}"), sel_style));
            }
            opt_lines.push(Line::from(spans));
        } else if is_active {
            let active_style = Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD);
            let mut spans = vec![
                Span::raw(marker),
                Span::styled("✓ ", active_style),
                Span::styled(label, active_style),
            ];
            if let Some(d) = desc {
                spans.push(Span::styled(
                    format!(" — {d}"),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            opt_lines.push(Line::from(spans));
        } else {
            let mut spans = vec![Span::raw(marker), Span::raw("  "), Span::raw(label)];
            if let Some(d) = desc {
                spans.push(Span::styled(
                    format!(" — {d}"),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            opt_lines.push(Line::from(spans));
        }
    }

    let scroll_y = if opt_height == 0 || opt_lines.len() <= opt_height {
        0
    } else {
        let max_s = opt_lines.len() - opt_height;
        if picker.selected_index >= opt_height {
            (picker.selected_index + 1 - opt_height).min(max_s) as u16
        } else {
            0
        }
    };

    f.render_widget(
        Paragraph::new(scope_line),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    if opt_height > 0 {
        f.render_widget(
            Paragraph::new(opt_lines).scroll((scroll_y, 0)),
            Rect::new(inner.x, inner.y + 1, inner.width, opt_height as u16),
        );
    }
    if inner.height >= 2 {
        f.render_widget(
            Paragraph::new(footer_line),
            Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
        );
    }
}

fn render_help_modal(f: &mut Frame, area: Rect, requested_scroll: u16) -> u16 {
    let section_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let key_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let shell_desc = format!(
        "Fork/root/rename; Alt+T >_ {}",
        crate::app::active_shell_label()
    );

    let sections: [(&str, Vec<(&str, String)>); 3] = [
        (
            "── Global ──",
            vec![
                (
                    "Tab / Shift+Tab",
                    "Cycle branches; Alt+Shift+↑/↓, Alt+J/K root".to_string(),
                ),
                (
                    "Alt+↑/↓, PgUp/PgDn",
                    "Scroll Transcript; Shift+PgUp/PgDn >_".to_string(),
                ),
                (
                    "/, ?, Ctrl+F, n / N",
                    "Search Transcript (Esc clear; F1/g? help)".to_string(),
                ),
                (
                    "Esc / Ctrl+G",
                    "Tree/pane; Enter restarts exited >_".to_string(),
                ),
                ("Ctrl+N / Alt+N, Alt+R / F2", shell_desc),
                (
                    "Ctrl+T / Alt+Z / Ctrl+O",
                    "⎇ Worktrees / Zoom / cards (Alt+Y, Ctrl+C)".to_string(),
                ),
            ],
        ),
        (
            "── ᛇ Grein Navigation ──",
            vec![
                (
                    "j/k/↑/↓, Enter/l/Right",
                    "Browse/enter; Space/h/l fold (J/K root)".to_string(),
                ),
                (
                    "Ctrl+U / Ctrl+D, g / G",
                    "Half-page scroll / top/btm; g? opens help".to_string(),
                ),
                (
                    "n / r / m (ᛇ Grein)",
                    "a/n new Agent root, rename/join, f/d/x".to_string(),
                ),
                (
                    "t / T, w, z/y/q (ᛇ Grein)",
                    ">_ term, worktree (Agent/>_), zoom/copy/q".to_string(),
                ),
            ],
        ),
        (
            "── Composer & Slash Commands ──",
            vec![
                (
                    "Shift+Enter / Ctrl+J",
                    "Newline in Composer (Enter sends)".to_string(),
                ),
                (
                    "Ctrl+A / Ctrl+E",
                    "Line start/end (Home/End, Ctrl+B / ← / →)".to_string(),
                ),
                (
                    "Alt+←/→, Ctrl+←/→, Alt+B/F",
                    "Word move, Ctrl+K/U/W, Ctrl+Y/Ctrl+D/Alt+D".to_string(),
                ),
                (
                    "!cmd, /term [path]",
                    "Cmd / >_ (/terminal /zsh /sh /shell /bash)".to_string(),
                ),
                (
                    "/fork, /grein, /root, /new",
                    "Fork (/branch) / root (/worktree ⎇ branch*)".to_string(),
                ),
                (
                    "/rename, /join, /merge",
                    "Rename / join (/close, /delete, /quit)".to_string(),
                ),
                (
                    "/model, /mode, /thinking",
                    "Pick model/mode (/effort), /wt, /worktrees".to_string(),
                ),
                (
                    "/commands, /copy, /zoom",
                    "Cmds, /copy [code|all], zoom, /?, /help, ?".to_string(),
                ),
            ],
        ),
    ];

    let mut raw_body: Vec<Line<'static>> = Vec::new();
    for (title, rows) in sections {
        raw_body.push(Line::from(Span::styled(format!(" {title}"), section_style)));
        for (k, desc) in rows {
            raw_body.push(Line::from(vec![
                Span::styled(format!(" {k:<27}"), key_style),
                Span::raw(desc),
            ]));
        }
    }

    let popup_width = area.width.saturating_sub(6).clamp(30, 74).min(area.width);
    let inner_width = popup_width.saturating_sub(2) as usize;
    let wrapped_body = crate::markdown::wrap_styled_lines(raw_body, inner_width);

    let popup_height = (wrapped_body.len() as u16 + 3).min(area.height.max(1));
    let popup_x = area.x + area.width.saturating_sub(popup_width) / 2;
    let popup_y = area.y + area.height.saturating_sub(popup_height) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    f.render_widget(Clear, popup_area);
    let block = Block::default()
        .title(" Keybindings & Commands ")
        .borders(Borders::ALL)
        .border_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    if inner.width == 0 || inner.height == 0 {
        return 0;
    }

    let body_height = inner.height.saturating_sub(1) as usize;
    let max_scroll = wrapped_body
        .len()
        .saturating_sub(body_height)
        .min(u16::MAX as usize) as u16;
    let scroll_y = requested_scroll.min(max_scroll);

    if body_height > 0 {
        f.render_widget(
            Paragraph::new(wrapped_body).scroll((scroll_y, 0)),
            Rect::new(inner.x, inner.y, inner.width, body_height as u16),
        );
    }

    let footer_text = if max_scroll > 0 {
        " Press Esc, q, ?, or Enter to close (↑/↓/j/k scroll)"
    } else {
        " Press Esc, q, ?, or Enter to close"
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            footer_text,
            Style::default().fg(Color::DarkGray),
        ))),
        Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
    );
    scroll_y
}

fn map_pty_color(color: PtyColor) -> Color {
    match color {
        PtyColor::Default => Color::Reset,
        PtyColor::Indexed(idx) => Color::Indexed(idx),
        PtyColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

fn pty_cell_style(cell: &PtyCell) -> Style {
    let mut style = Style::default()
        .fg(map_pty_color(cell.fg))
        .bg(map_pty_color(cell.bg));
    if cell.bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.inverse {
        style = style.add_modifier(Modifier::REVERSED);
    }
    style
}

fn render_terminal_pane(
    f: &mut Frame,
    area: Rect,
    state: &AppState,
    display_thread_id: &str,
    preview_title: Option<&str>,
    terminal_focused: bool,
) -> Option<(u16, u16)> {
    if area.width == 0 || area.height == 0 {
        return None;
    }

    let borders = if state.sidebar_hidden {
        Borders::TOP | Borders::BOTTOM
    } else {
        Borders::ALL
    };
    let thread_title = state
        .threads
        .iter()
        .find(|t| t.id == display_thread_id)
        .map(|t| t.title.as_str())
        .unwrap_or("Terminal");
    let exit_suffix = match state.terminal_exit_code(display_thread_id) {
        Some(Some(c)) => format!(" [exit {c}]"),
        Some(None) => " [exited]".to_string(),
        None => String::new(),
    };
    let title = match preview_title {
        Some(t) => format!(" Terminal (Preview: {t}){exit_suffix} "),
        None => {
            if let Some(cwd) = state
                .pty_screens
                .get(display_thread_id)
                .and_then(|s| s.cwd.as_deref())
                .filter(|s| !s.trim().is_empty())
                .or_else(|| {
                    state
                        .thread_meta
                        .get(display_thread_id)
                        .and_then(|m| m.cwd.as_deref())
                        .filter(|s| !s.trim().is_empty())
                })
            {
                format!(" Terminal: {thread_title} ({cwd}){exit_suffix} ")
            } else {
                format!(" Terminal: {thread_title}{exit_suffix} ")
            }
        }
    };

    let block = Block::default()
        .title(title)
        .borders(borders)
        .border_style(border_style(terminal_focused));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return None;
    }

    if let Some(snap) = state.pty_screens.get(display_thread_id) {
        let buf = f.buffer_mut();
        for r in 0..inner.height {
            let Some(row) = snap.cells.get(r as usize) else {
                break;
            };
            let y = inner.y + r;
            for c in 0..inner.width {
                let Some(cell) = row.get(c as usize) else {
                    break;
                };
                if cell.is_wide_continuation {
                    continue;
                }
                let x = inner.x + c;
                let style = pty_cell_style(cell);
                let sym = if cell.contents.is_empty() {
                    " "
                } else {
                    cell.contents.as_str()
                };
                let w = Span::raw(sym).width();
                let remaining = (inner.width - c) as usize;
                if w == 0 || w > remaining {
                    buf[(x, y)].set_symbol(" ").set_style(style);
                } else {
                    buf.set_stringn(x, y, sym, remaining, style);
                }
            }
        }

        if terminal_focused
            && preview_title.is_none()
            && !snap.hide_cursor
            && !snap.exited
            && !state.has_active_modal()
            && !state.transcript_search.editing
        {
            let cursor_x = inner.x + snap.cursor_col.min(inner.width.saturating_sub(1));
            let cursor_y = inner.y + snap.cursor_row.min(inner.height.saturating_sub(1));
            f.set_cursor_position(Position::new(cursor_x, cursor_y));
        }
    } else {
        let msg = if state.terminal_exit_code(display_thread_id).is_some() {
            "Terminal process exited"
        } else {
            "Starting terminal..."
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                msg,
                Style::default().fg(Color::DarkGray),
            ))),
            inner,
        );
    }

    Some((inner.height, inner.width))
}

/// Builds the pre-wrapped styled transcript lines for `display_thread_id` at `inner_width`.
pub fn build_transcript_wrapped_lines(
    state: &AppState,
    display_thread_id: &str,
    inner_width: usize,
) -> Vec<Line<'static>> {
    let active_id = state.active_thread_id();
    let display_md: &str = if display_thread_id == active_id {
        &state.active_transcript_md
    } else {
        state
            .thread_transcripts
            .get(display_thread_id)
            .map(String::as_str)
            .unwrap_or("")
    };
    let use_blocks = state.has_synchronized_blocks(display_thread_id, display_md);

    let mut transcript_lines = Vec::new();
    if let Some(meta) = state.thread_meta.get(display_thread_id) {
        if let Some(ref parent_id) = meta.parent_thread_id {
            let parent_title = state
                .threads
                .iter()
                .find(|t| t.id == *parent_id)
                .map(|t| t.title.as_str())
                .unwrap_or(parent_id.as_str());
            let banner = format!(
                "> ↳ Forked from {parent_title} ({} ancestor turns in LLM context)",
                meta.inherited_turn_count
            );
            transcript_lines.push(Line::from(Span::styled(
                banner,
                Style::default().fg(Color::DarkGray),
            )));
            let has_body = if use_blocks {
                state
                    .thread_blocks
                    .get(display_thread_id)
                    .is_some_and(|b| !b.is_empty())
            } else {
                !display_md.trim().is_empty()
            };
            if has_body {
                transcript_lines.push(Line::raw(""));
            }
        }
    }

    if use_blocks {
        if let Some(blocks) = state.thread_blocks.get(display_thread_id) {
            transcript_lines.extend(blocks.render_lines(inner_width));
        }
    } else {
        transcript_lines.extend(crate::markdown::render_markdown_lines_with_width(
            display_md,
            inner_width,
        ));
    }

    crate::markdown::wrap_styled_lines(transcript_lines, inner_width)
}

fn recompute_search_for_render(
    state: &AppState,
    display_thread_id: &str,
    lines: &[Line<'static>],
    inner_height: u16,
) -> Option<TranscriptSearchState> {
    if !state.transcript_search.is_visible() || state.transcript_search.query.is_empty() {
        return None;
    }
    let mut fresh = state.transcript_search.clone();
    let prev_active = state
        .transcript_search
        .current_index
        .and_then(|i| state.transcript_search.matches.get(i))
        .cloned();
    let vp = state.transcript_viewport(display_thread_id);
    let anchor = match fresh.direction {
        SearchDirection::Forward => Some(vp.scroll_offset()),
        SearchDirection::Reverse => {
            let vp_h = inner_height.max(1) as usize;
            Some(
                vp.scroll_offset()
                    .saturating_add(vp_h.saturating_sub(1))
                    .min(lines.len().saturating_sub(1)),
            )
        }
    };
    fresh.recompute_matches(lines, anchor);
    if fresh.matches == state.transcript_search.matches {
        fresh.current_index = state.transcript_search.current_index;
    } else if let Some(prev_m) = prev_active {
        if let Some(pos) = fresh.matches.iter().position(|m| *m == prev_m) {
            fresh.current_index = Some(pos);
        } else if let (Some(old_idx), false) = (
            state.transcript_search.current_index,
            fresh.matches.is_empty(),
        ) {
            fresh.current_index = Some(old_idx.min(fresh.matches.len() - 1));
        }
    }
    Some(fresh)
}

fn render_worktree_delete_modal(f: &mut Frame, area: Rect, pending: &PendingWorktreeDelete) {
    let popup_width = area.width.saturating_sub(8).clamp(28, 72).min(area.width);
    let mut lines = Vec::new();
    lines.push(Line::raw(""));
    if pending.dirty_count > 0 {
        lines.push(Line::from(Span::styled(
            format!(
                "Dirty ({} files)! Force delete {}? [y/N]",
                pending.dirty_count, pending.label
            ),
            Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            format!("Delete worktree {}? [y/N]", pending.label),
            Style::default().add_modifier(Modifier::BOLD),
        )));
    }
    lines.push(Line::from(Span::styled(
        format!("Path: {}", pending.path),
        Style::default().fg(Color::DarkGray),
    )));
    if pending.attached_count > 0 {
        lines.push(Line::from(Span::styled(
            format!(
                "{} attached thread(s) will reset to repo root",
                pending.attached_count
            ),
            Style::default().fg(Color::Yellow),
        )));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "[y] Delete  [n/Esc] Cancel",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )));

    let popup_height = (lines.len() as u16 + 3)
        .max(8)
        .min(area.height.max(1));
    let popup_x = area.x + area.width.saturating_sub(popup_width) / 2;
    let popup_y = area.y + area.height.saturating_sub(popup_height) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    f.render_widget(Clear, popup_area);
    let border_color = if pending.dirty_count > 0 {
        Color::Red
    } else {
        Color::Yellow
    };
    let block = Block::default()
        .title(" Delete Worktree ")
        .borders(Borders::ALL)
        .border_style(
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        );
    let widget = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(widget, popup_area);
}

fn render_worktree_autocomplete_popup(f: &mut Frame, composer_area: Rect, state: &AppState) {
    if composer_area.y < 3 || composer_area.width < 20 {
        return;
    }
    let candidates = state.worktree_autocomplete_candidates();
    if candidates.is_empty() {
        return;
    }
    let max_visible = candidates.len().min(8);
    let popup_height = (max_visible as u16 + 2).min(composer_area.y);
    if popup_height < 3 {
        return;
    }
    let popup_y = composer_area.y.saturating_sub(popup_height);
    let popup_area = Rect::new(composer_area.x, popup_y, composer_area.width, popup_height);

    f.render_widget(Clear, popup_area);
    let block = Block::default()
        .title(" Worktrees (Tab complete · Enter attach) ")
        .borders(Borders::ALL)
        .border_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let selected = state.worktree_autocomplete_idx.min(candidates.len() - 1);
    let scroll = compute_tree_scroll_offset(0, Some(selected), candidates.len(), inner.height);
    let lines: Vec<Line<'static>> = candidates
        .iter()
        .enumerate()
        .map(|(idx, item)| {
            let is_selected = idx == selected;
            let sel_style = Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED);
            match item {
                WorktreeAutocompleteItem::CreateNew => {
                    if is_selected {
                        Line::from(vec![
                            Span::styled(" ▶ + New isolated worktree", sel_style),
                            Span::styled(
                                " — create branch worktree",
                                Style::default()
                                    .fg(Color::DarkGray)
                                    .add_modifier(Modifier::REVERSED),
                            ),
                        ])
                    } else {
                        Line::from(vec![
                            Span::styled(
                                "   + New isolated worktree",
                                Style::default()
                                    .fg(Color::Green)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                " — create branch worktree",
                                Style::default().fg(Color::DarkGray),
                            ),
                        ])
                    }
                }
                WorktreeAutocompleteItem::Existing(entry) => {
                    let branch = worktree_display_label(entry);
                    let primary_tag = if entry.is_primary { " (primary)" } else { "" };
                    if is_selected {
                        let mut spans = vec![Span::styled(
                            format!(" ▶ {} │ {branch}{primary_tag}", entry.repo_name),
                            sel_style,
                        )];
                        if entry.dirty_count > 0 {
                            spans.push(Span::styled(" │ ", sel_style));
                            spans.push(Span::styled(
                                format!("*{}", entry.dirty_count),
                                Style::default()
                                    .fg(Color::Yellow)
                                    .add_modifier(Modifier::BOLD | Modifier::REVERSED),
                            ));
                        }
                        if !entry.attached_thread_ids.is_empty() {
                            spans.push(Span::styled(
                                format!(" [{}]", entry.attached_thread_ids.len()),
                                Style::default()
                                    .fg(Color::DarkGray)
                                    .add_modifier(Modifier::REVERSED),
                            ));
                        }
                        Line::from(spans)
                    } else {
                        let mut spans = vec![
                            Span::raw("   "),
                            Span::styled(
                                entry.repo_name.clone(),
                                Style::default().fg(Color::Cyan),
                            ),
                            Span::styled(" │ ", Style::default().fg(Color::DarkGray)),
                            Span::raw(format!("{branch}{primary_tag}")),
                        ];
                        if entry.dirty_count > 0 {
                            spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
                            spans.push(Span::styled(
                                format!("*{}", entry.dirty_count),
                                Style::default().fg(Color::Yellow),
                            ));
                        }
                        if !entry.attached_thread_ids.is_empty() {
                            spans.push(Span::styled(
                                format!(" [{}]", entry.attached_thread_ids.len()),
                                Style::default().fg(Color::DarkGray),
                            ));
                        }
                        Line::from(spans)
                    }
                }
            }
        })
        .collect();

    f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);
}

fn render_app_inner(f: &mut Frame, state: &AppState) -> RenderMetrics {
    let active_id = state.active_thread_id();
    let active_is_terminal = state.is_terminal_thread(active_id);
    let tree_focused = state.active_focus == PaneFocus::Tree
        || state.focused_pane == PaneFocus::Tree
        || state.focus == PaneFocus::Tree;
    let preview = if tree_focused && !state.sidebar_hidden && state.sidebar_view == SidebarView::Grein {
        selected_tree_thread(state).filter(|(sel_id, _)| *sel_id != active_id)
    } else {
        None
    };
    let display_thread_id = preview.map(|(id, _)| id).unwrap_or(active_id);
    let display_is_terminal = state.is_terminal_thread(display_thread_id);
    let show_composer =
        (!active_is_terminal && !display_is_terminal) || state.pending_rename_thread_id.is_some();
    let active_subagents: &[SubagentSnapshot] = state
        .thread_subagents
        .get(active_id)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let strip_h = subagent_strip_height(active_subagents.len());
    let mut layout = compute_layout_with_options(
        f.area(),
        strip_h,
        state.sidebar_hidden,
        !show_composer,
    );

    let (composer_layout, visual_line_count) = if show_composer && layout.composer_area.height > 0
    {
        let composer_inner_width = layout.composer_area.width.saturating_sub(2);
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
            layout.tree_area.height = layout.tree_area.height.saturating_sub(delta);
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

    let tree_result = if state.sidebar_hidden {
        TreeRenderResult {
            tree_scroll: state.tree.scroll_offset,
            worktree_scroll: state.worktree_scroll_offset,
            filter_cursor: None,
        }
    } else {
        render_tree_view_inner(f, layout.tree_area, state)
    };

    let mut max_scroll_u16 = 0u16;
    let mut transcript_inner_height = layout.transcript_area.height.saturating_sub(2);
    let mut terminal_inner_size = None;
    let mut updated_search: Option<TranscriptSearchState> = None;

    if display_is_terminal {
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
            Some((_, title)) => format!(" Transcript (Preview: {title}) "),
            None => " Transcript ".to_string(),
        };
        let transcript_block = Block::default()
            .title(transcript_title)
            .borders(transcript_borders)
            .border_style(border_style(transcript_focused));
        let transcript_widget = Paragraph::new(wrapped_lines)
            .block(transcript_block)
            .scroll((transcript_scroll_y, 0));
        f.render_widget(transcript_widget, layout.transcript_area);
    }

    let mut composer_scroll_y = state.composer_scroll_y;
    if let Some(composer_layout) = composer_layout {
        let composer_inner_width = layout.composer_area.width.saturating_sub(2);
        let composer_block = Block::default()
            .title(" Composer ")
            .borders(Borders::ALL)
            .border_style(border_style(composer_focused));

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

        if composer_focused
            && !state.has_active_modal()
            && !state.transcript_search.editing
        {
            let visible_row = composer_layout.cursor_row.saturating_sub(composer_scroll_y);
            let cursor_x = layout.composer_area.x
                + 1
                + composer_layout
                    .cursor_col
                    .min(composer_inner_width.saturating_sub(1));
            let cursor_y = layout.composer_area.y
                + 1
                + visible_row.min(inner_height.saturating_sub(1));
            f.set_cursor_position(Position::new(cursor_x, cursor_y));
        }

        if composer_focused && state.worktree_autocomplete_open && !state.has_active_modal() {
            render_worktree_autocomplete_popup(f, layout.composer_area, state);
        }
    }

    if let Some(strip_area) = layout.subagent_strip_area {
        render_subagent_strip(f, strip_area, active_subagents);
    }

    let effective_search = updated_search
        .as_ref()
        .unwrap_or(&state.transcript_search);
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
        let cursor_x = layout.status_bar_area.x
            + (1 + query_w).min(layout.status_bar_area.width.saturating_sub(1));
        f.set_cursor_position(Position::new(cursor_x, layout.status_bar_area.y));
    } else if let Some(filter_pos) = tree_result.filter_cursor {
        if !state.has_active_modal() {
            f.set_cursor_position(filter_pos);
        }
    }

    let mut rendered_help_scroll = state.help_scroll_offset;
    if let Some(perm) = &state.pending_permission {
        let area = f.area();
        let popup_width = area.width.saturating_sub(8).clamp(20, 64).min(area.width);
        let popup_height = 9u16.min(area.height.max(1));
        let popup_x = area.x + area.width.saturating_sub(popup_width) / 2;
        let popup_y = area.y + area.height.saturating_sub(popup_height) / 2;
        let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

        f.render_widget(Clear, popup_area);
        let popup_block = Block::default()
            .title(" Permission Required ")
            .borders(Borders::ALL)
            .border_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            );
        let popup_lines = vec![
            Line::raw(""),
            Line::from(perm.description.clone()),
            Line::raw(""),
            Line::from(Span::styled(
                "[y] Allow  [n] Deny",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
        ];
        let popup_widget = Paragraph::new(popup_lines)
            .block(popup_block)
            .wrap(Wrap { trim: false });
        f.render_widget(popup_widget, popup_area);
    } else if let Some(ref pending) = state.pending_worktree_delete {
        render_worktree_delete_modal(f, f.area(), pending);
    } else if let Some(ref picker) = state.config_picker {
        render_config_picker_modal(f, f.area(), picker);
    } else if state.show_help_modal {
        rendered_help_scroll = render_help_modal(f, f.area(), state.help_scroll_offset);
    }

    RenderMetrics {
        thread_id: display_thread_id.to_string(),
        max_scroll: max_scroll_u16,
        tree_scroll: tree_result.tree_scroll,
        worktree_scroll: tree_result.worktree_scroll,
        transcript_inner_height,
        composer_scroll_y,
        help_scroll_offset: rendered_help_scroll,
        frame_area: f.area(),
        terminal_inner_size,
        display_is_preview: preview.is_some(),
        updated_search,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn test_composer_multiline_auto_grow_and_scroll() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = App::new();
        app.input_buffer = "line1\nline2\nline3\nline4".to_string();

        terminal.draw(|f| render_app(f, &app)).unwrap();

        let buf = terminal.backend().buffer();
        let mut rendered = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                rendered.push_str(buf[(x, y)].symbol());
            }
            rendered.push('\n');
        }

        assert!(rendered.contains("line1"));
        assert!(rendered.contains("line2"));
        assert!(rendered.contains("line3"));
        assert!(rendered.contains("line4"));
    }

    #[test]
    fn test_render_writes_max_scroll_renders_popup_and_tree_status() {
        let backend = TestBackend::new(110, 28);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = App::new();
        app.active_thread_id = "t-1".to_string();
        app.threads = vec![
            crate::app::ThreadItem {
                id: "t-1".to_string(),
                title: "Trunk".to_string(),
                anchors: Vec::new(),
            },
            crate::app::ThreadItem {
                id: "t-2".to_string(),
                title: "Side".to_string(),
                anchors: Vec::new(),
            },
        ];
        app.thread_statuses
            .insert("t-1".to_string(), "streaming".to_string());
        app.thread_statuses
            .insert("t-2".to_string(), "error".to_string());
        app.active_transcript_md = (0..40)
            .map(|i| format!("Line {i}: long wrapped content for testing transcript scroll"))
            .collect::<Vec<_>>()
            .join("\n");
        app.sync_anchors();

        terminal.draw(|f| render_app_mut(f, &mut app)).unwrap();
        let vp = app.transcript_viewport("t-1");
        assert!(vp.max_scroll() > 0);
        assert_eq!(vp.scroll_offset(), vp.max_scroll());

        let expected_before = vp.max_scroll();
        app.set_focus(crate::app::FocusedPane::Transcript);
        app.handle_key_event(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Up,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(app.active_transcript_scroll_offset(), expected_before - 1);

        app.push_permission(crate::app::PendingPermission {
            request_id: "r1".to_string(),
            description: "Allow shell execution?".to_string(),
        });
        terminal.draw(|f| render_app_mut(f, &mut app)).unwrap();

        let buf = terminal.backend().buffer();
        let mut screen = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                screen.push_str(buf[(x, y)].symbol());
            }
            screen.push('\n');
        }
        assert!(screen.contains("Permission Required"));
        assert!(screen.contains("[y] Allow  [n] Deny"));
        assert!(!screen.contains("Blackboard"));
        assert!(screen.contains("●"));
        assert!(screen.contains("⚠"));
        assert!(screen.contains(&format!(
            "Esc cancel · Ctrl+G tree · Alt+T {}",
            crate::app::active_shell_label()
        )));
        assert!(!screen.contains("Enter: Send"));
    }
}
