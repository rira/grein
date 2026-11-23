use std::collections::{BTreeSet, HashMap};

use crate::app::{
    shorten_terminal_cwd, worktree_display_label, AppState, ConfigPickerModal, PaneFocus,
    PendingQuitConfirm, PendingThreadDelete, PendingWorktreeDelete, SidebarMode, SidebarView,
    WorktreeAutocompleteItem, WorktreeRow,
};
use crate::engine_port::{PtyCell, PtyColor, SubagentSnapshot, SubagentStatus};
use crate::keymap::BindableAction;
use crate::layout::{compute_layout_with_options, subagent_strip_height, ChromeLayout};
use crate::runner::{compute_ordered_thread_entries_with_folds, compute_ordered_thread_indices};
use crate::search::{highlight_lines_with_matches, SearchDirection, TranscriptSearchState};
use crate::theme::{ThemePalette, ThemePickerModal, ThemePreset};
use crate::ui::tree::{compute_tree_scroll_offset, TreeNode};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn border_style(is_focused: bool) -> Style {
    let palette = ThemePalette::from_preset(ThemePreset::TokyoNight);
    themed_border_style(&palette, is_focused)
}

fn themed_border_style(theme: &ThemePalette, is_focused: bool) -> Style {
    if is_focused {
        Style::default()
            .fg(theme.border_focused)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.border_unfocused)
    }
}

fn rounded_block(theme: &ThemePalette, is_focused: bool) -> Block<'static> {
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(themed_border_style(theme, is_focused));
    if theme.panel_bg != Color::Reset {
        block = block.style(Style::default().bg(theme.panel_bg).fg(theme.text));
    }
    block
}

fn max_grein_sidebar_row_width(state: &AppState) -> usize {
    let mut max_w = 26usize;
    if !state.tree.nodes.is_empty() {
        for node in &state.tree.nodes {
            if let TreeNode::Thread(t) = node {
                let prefix_w = state.tree.prefix_for(&t.id).chars().count();
                let title_w = state.display_title_for(&t.id, &t.title).chars().count();
                let branch_w = state
                    .thread_meta
                    .get(&t.id)
                    .and_then(|m| m.git_branch.as_deref())
                    .map(|b| b.chars().count().min(14) + 3)
                    .unwrap_or(0);
                max_w = max_w.max(prefix_w + title_w + branch_w + 10);
            }
        }
    } else {
        let ordered = compute_ordered_thread_entries_with_folds(
            &state.threads,
            &state.thread_meta,
            &state.collapsed_threads,
        );
        for e in ordered {
            let t = &state.threads[e.index];
            let prefix_w = e.prefix.chars().count();
            let title_w = state.display_title_for(&t.id, &t.title).chars().count();
            max_w = max_w.max(prefix_w + title_w + 10);
        }
    }
    max_w + 2
}

fn max_worktree_sidebar_row_width(state: &AppState) -> usize {
    let mut max_w = 26usize;
    for row in state.visible_worktree_rows() {
        let row_w = match row {
            WorktreeRow::RepoHeader {
                repo_name, count, ..
            } => repo_name.chars().count() + format!(" ({count})").len() + 4,
            WorktreeRow::Entry(entry) => {
                let label_w = worktree_display_label(&entry).chars().count();
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

fn thread_status_glyph(
    theme: &ThemePalette,
    status: Option<&str>,
    depth: usize,
) -> Option<(&'static str, Color)> {
    match status.unwrap_or("idle") {
        "streaming" | "executing" | "thinking" => Some(("●", theme.warning)),
        "permission" | "permission_required" | "error" => Some(("⚠", theme.error)),
        _ if depth == 0 => Some(("ᛉ", theme.success)),
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
    let theme = &state.theme_palette;
    let is_active = id == state.active_thread_id;
    let is_merged = state.thread_meta.get(id).is_some_and(|m| m.merged);
    let is_terminal = state.is_terminal_thread(id);
    let exit_code_opt = state.terminal_exit_code(id);
    let display_title = state.display_title_for(id, title);
    let glyph_opt = if is_terminal {
        let color = match exit_code_opt {
            Some(Some(c)) if c != 0 => theme.error,
            Some(_) => theme.muted,
            None => theme.accent,
        };
        Some((">_", color))
    } else {
        thread_status_glyph(
            theme,
            state.thread_statuses.get(id).map(String::as_str),
            depth,
        )
    };
    let exit_badge = exit_code_opt.map(|code_opt| {
        let (label, color) = match code_opt {
            Some(0) => (" ✓ 0".to_string(), theme.muted),
            Some(c) => (format!(" ✗ {c}"), theme.error),
            None => (" ✓ exited".to_string(), theme.muted),
        };
        (label, color)
    });
    let fold_prefix = if hidden_descendants > 0 { "▸ " } else { "" };
    let fold_badge = (hidden_descendants > 0).then(|| format!(" (+{hidden_descendants})"));

    if is_selected {
        let bg = theme.selection_bg;
        let sel_title = Style::default()
            .fg(theme.selection_fg)
            .bg(bg)
            .add_modifier(Modifier::BOLD);
        let mut spans = vec![
            Span::styled("▌", Style::default().fg(theme.selection_bar).bg(bg)),
            Span::styled(prefix.to_string(), Style::default().fg(theme.muted).bg(bg)),
        ];
        if !fold_prefix.is_empty() {
            spans.push(Span::styled(
                fold_prefix,
                Style::default()
                    .fg(theme.accent)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        if let Some((glyph, glyph_color)) = glyph_opt {
            spans.push(Span::styled(
                format!("{glyph} "),
                Style::default().fg(glyph_color).bg(bg),
            ));
        }
        spans.push(Span::styled(display_title, sel_title));
        if let Some(badge) = fold_badge {
            spans.push(Span::styled(badge, Style::default().fg(theme.muted).bg(bg)));
        }
        if let Some((badge, color)) = exit_badge {
            spans.push(Span::styled(badge, Style::default().fg(color).bg(bg)));
        }
        if is_merged {
            spans.push(Span::styled(
                " [joined]",
                Style::default().fg(theme.muted).bg(bg),
            ));
        }
        Line::from(spans)
    } else {
        let title_style = if is_active {
            Style::default()
                .fg(theme.user_accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(theme.text)
                .add_modifier(Modifier::BOLD)
        };
        let mut spans = vec![
            Span::raw(" "),
            Span::styled(prefix.to_string(), Style::default().fg(theme.muted)),
        ];
        if !fold_prefix.is_empty() {
            spans.push(Span::styled(
                fold_prefix,
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        if let Some((glyph, glyph_color)) = glyph_opt {
            spans.push(Span::styled(
                format!("{glyph} "),
                Style::default().fg(glyph_color),
            ));
        }
        spans.push(Span::styled(display_title, title_style));
        if let Some(badge) = fold_badge {
            spans.push(Span::styled(badge, Style::default().fg(theme.muted)));
        }
        if let Some((badge, color)) = exit_badge {
            spans.push(Span::styled(badge, Style::default().fg(color)));
        }
        if is_merged {
            spans.push(Span::styled(" [joined]", Style::default().fg(theme.muted)));
        }
        Line::from(spans)
    }
}

fn render_sidebar_title(state: &AppState) -> Line<'static> {
    let theme = &state.theme_palette;
    let active_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let dim_style = Style::default().fg(theme.muted);
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
            Style::default().fg(theme.warning),
        ));
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}

fn render_worktree_row(
    row: &WorktreeRow,
    is_selected: bool,
    active_worktree_path: Option<&str>,
    theme: &ThemePalette,
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
            if is_selected {
                Line::from(vec![
                    Span::styled(
                        "▌",
                        Style::default()
                            .fg(theme.selection_bar)
                            .bg(theme.selection_bg),
                    ),
                    Span::styled(
                        text,
                        Style::default()
                            .fg(theme.selection_fg)
                            .bg(theme.selection_bg)
                            .add_modifier(Modifier::BOLD),
                    ),
                ])
            } else {
                Line::from(vec![
                    Span::raw(" "),
                    Span::styled(
                        text,
                        Style::default()
                            .fg(theme.text)
                            .add_modifier(Modifier::BOLD),
                    ),
                ])
            }
        }
        WorktreeRow::Entry(entry) => {
            let is_active = active_worktree_path == Some(entry.path.as_str());
            let label = worktree_display_label(entry);
            if is_selected {
                let bg = theme.selection_bg;
                let sel_style = Style::default()
                    .fg(theme.selection_fg)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD);
                let mut spans = vec![
                    Span::styled("▌", Style::default().fg(theme.selection_bar).bg(bg)),
                    Span::styled(" ", Style::default().bg(bg)),
                ];
                if is_active {
                    spans.push(Span::styled(
                        "● ",
                        Style::default()
                            .fg(theme.success)
                            .bg(bg)
                            .add_modifier(Modifier::BOLD),
                    ));
                } else {
                    spans.push(Span::styled("  ", Style::default().bg(bg)));
                }
                spans.push(Span::styled(label, sel_style));
                if entry.is_primary {
                    spans.push(Span::styled(
                        " (primary)",
                        Style::default().fg(theme.muted).bg(bg),
                    ));
                }
                if entry.dirty_count > 0 {
                    spans.push(Span::styled(
                        format!(" *{}", entry.dirty_count),
                        Style::default().fg(theme.dirty).bg(bg),
                    ));
                }
                if !entry.attached_thread_ids.is_empty() {
                    spans.push(Span::styled(
                        format!(" [{}]", entry.attached_thread_ids.len()),
                        Style::default().fg(theme.muted).bg(bg),
                    ));
                }
                Line::from(spans)
            } else {
                let label_style = if is_active {
                    Style::default()
                        .fg(theme.user_accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.text)
                };
                let mut spans = vec![Span::raw("  ")];
                if is_active {
                    spans.push(Span::styled(
                        "● ",
                        Style::default()
                            .fg(theme.success)
                            .add_modifier(Modifier::BOLD),
                    ));
                } else {
                    spans.push(Span::raw("  "));
                }
                spans.push(Span::styled(label, label_style));
                if entry.is_primary {
                    spans.push(Span::styled(" (primary)", Style::default().fg(theme.muted)));
                }
                if entry.dirty_count > 0 {
                    spans.push(Span::styled(
                        format!(" *{}", entry.dirty_count),
                        Style::default().fg(theme.dirty),
                    ));
                }
                if !entry.attached_thread_ids.is_empty() {
                    spans.push(Span::styled(
                        format!(" [{}]", entry.attached_thread_ids.len()),
                        Style::default().fg(theme.muted),
                    ));
                }
                Line::from(spans)
            }
        }
    }
}

fn render_sidebar_bottom_panel(
    f: &mut Frame,
    inner: Rect,
    state: &AppState,
    tree_focused: bool,
) -> (u16, Option<Position>) {
    let theme = &state.theme_palette;
    if inner.height < 2 || inner.width == 0 {
        return (0, None);
    }
    if state.palette_active {
        let suggestions = state.palette_suggestions();
        let max_sugg = suggestions
            .len()
            .min(5)
            .min(inner.height.saturating_sub(2) as usize);
        let reserved = 1 + max_sugg as u16;
        let top_y = inner.y + inner.height - reserved;

        for (i, (cmd, desc)) in suggestions.iter().take(max_sugg).enumerate() {
            let row_rect = Rect::new(inner.x, top_y + i as u16, inner.width, 1);
            let is_sel = i == state.palette_match_idx.min(suggestions.len().saturating_sub(1));
            let line = if is_sel {
                Line::from(vec![
                    Span::styled(
                        "▌",
                        Style::default()
                            .fg(theme.selection_bar)
                            .bg(theme.selection_bg),
                    ),
                    Span::styled(
                        format!(":{cmd}"),
                        Style::default()
                            .fg(theme.accent)
                            .bg(theme.selection_bg)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" {desc}"),
                        Style::default().fg(theme.muted).bg(theme.selection_bg),
                    ),
                ])
            } else {
                Line::from(vec![
                    Span::raw(" "),
                    Span::styled(format!(":{cmd}"), Style::default().fg(theme.text)),
                    Span::styled(format!(" {desc}"), Style::default().fg(theme.muted)),
                ])
            };
            let mut p = Paragraph::new(line);
            if is_sel {
                p = p.style(Style::default().bg(theme.selection_bg));
            }
            f.render_widget(p, row_rect);
        }

        let prompt_rect = Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1);
        let prompt_line = Line::from(vec![
            Span::styled(
                ":",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                state.palette_input.clone(),
                Style::default().fg(theme.text),
            ),
        ]);
        let prompt_bg = if theme.status_bar_bg != Color::Reset {
            theme.status_bar_bg
        } else {
            theme.selection_bg
        };
        f.render_widget(
            Paragraph::new(prompt_line).style(Style::default().bg(prompt_bg)),
            prompt_rect,
        );
        let cursor_pos = if tree_focused && !state.has_active_modal() {
            let prefix = &state.palette_input[..clamp_char_boundary(&state.palette_input, state.palette_cursor)];
            let cw = Span::raw(prefix).width() as u16;
            Some(Position::new(
                prompt_rect.x + (1 + cw).min(inner.width.saturating_sub(1)),
                prompt_rect.y,
            ))
        } else {
            None
        };
        return (reserved, cursor_pos);
    }

    if let Some(ref rename) = state.rename_prompt {
        let prompt_rect = Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1);
        let prompt_line = Line::from(vec![
            Span::styled(
                "Rename: ",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(rename.buffer.clone(), Style::default().fg(theme.text)),
        ]);
        let prompt_bg = if theme.status_bar_bg != Color::Reset {
            theme.status_bar_bg
        } else {
            theme.selection_bg
        };
        f.render_widget(
            Paragraph::new(prompt_line).style(Style::default().bg(prompt_bg)),
            prompt_rect,
        );
        let cursor_pos = if tree_focused && !state.has_active_modal() {
            let prefix = &rename.buffer[..clamp_char_boundary(&rename.buffer, rename.cursor)];
            let cw = Span::raw(prefix).width() as u16;
            Some(Position::new(
                prompt_rect.x + (8 + cw).min(inner.width.saturating_sub(1)),
                prompt_rect.y,
            ))
        } else {
            None
        };
        return (1, cursor_pos);
    }

    let footer_text = match state.sidebar_view {
        SidebarView::Grein => {
            let shells = state
                .threads
                .iter()
                .filter(|t| state.is_terminal_thread(&t.id))
                .count();
            let agents = state.threads.len().saturating_sub(shells);
            let text = format!("{agents} agents · {shells} shells · : cmd · ? help");
            truncate_single_line(&text, inner.width as usize)
        }
        SidebarView::Worktrees => {
            let repos: BTreeSet<&str> = state
                .worktrees
                .iter()
                .map(|w| w.repo_root.as_str())
                .collect();
            let text = format!(
                "{} repos · {} worktrees · p prune",
                repos.len(),
                state.worktrees.len()
            );
            truncate_single_line(&text, inner.width as usize)
        }
    };
    let footer_rect = Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            footer_text,
            Style::default().fg(theme.muted),
        ))),
        footer_rect,
    );
    (1, None)
}

fn render_worktrees_view(
    f: &mut Frame,
    area: Rect,
    state: &AppState,
    tree_focused: bool,
) -> TreeRenderResult {
    let theme = &state.theme_palette;
    let block = rounded_block(theme, tree_focused).title(render_sidebar_title(state));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return TreeRenderResult {
            tree_scroll: state.tree.scroll_offset,
            worktree_scroll: state.worktree_scroll_offset,
            cursor_pos: None,
            overflow_line: None,
        };
    }

    let show_filter_bar = state.worktree_filter_active || !state.worktree_filter_query.is_empty();
    let filter_h = u16::from(show_filter_bar && inner.height >= 2);
    let mut cursor_pos = None;

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
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                state.worktree_filter_query.clone(),
                Style::default().fg(theme.text),
            ),
            Span::styled(hint, Style::default().fg(theme.muted)),
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
            cursor_pos = Some(Position::new(cursor_x, inner.y));
        }
    }

    let avail_for_bottom = inner.height.saturating_sub(filter_h);
    let bottom_area = Rect::new(inner.x, inner.y + filter_h, inner.width, avail_for_bottom);
    let (bottom_h, prompt_cursor) =
        render_sidebar_bottom_panel(f, bottom_area, state, tree_focused);
    if let Some(pc) = prompt_cursor {
        cursor_pos = Some(pc);
    }

    let list_area = Rect::new(
        inner.x,
        inner.y + filter_h,
        inner.width,
        avail_for_bottom.saturating_sub(bottom_h),
    );
    if list_area.height == 0 {
        return TreeRenderResult {
            tree_scroll: state.tree.scroll_offset,
            worktree_scroll: state.worktree_scroll_offset,
            cursor_pos,
            overflow_line: None,
        };
    }

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
                Style::default().fg(theme.muted),
            ))],
            None,
        )
    } else {
        let sel = state.worktree_selected_idx.min(rows.len() - 1);
        let built: Vec<Line<'static>> = rows
            .iter()
            .enumerate()
            .map(|(idx, row)| render_worktree_row(row, idx == sel, active_path.as_deref(), theme))
            .collect();
        (built, Some(sel))
    };

    let scroll_offset = compute_tree_scroll_offset(
        state.worktree_scroll_offset.min(u16::MAX as usize) as u16,
        selected_line_idx,
        lines.len(),
        list_area.height,
    );
    let overflow_line = selected_line_idx.and_then(|sel_idx| {
        if sel_idx < scroll_offset as usize {
            return None;
        }
        let rel_y = (sel_idx - scroll_offset as usize) as u16;
        if rel_y >= list_area.height {
            return None;
        }
        let line = lines.get(sel_idx)?;
        (line.width() > list_area.width as usize)
            .then(|| (list_area.x, list_area.y + rel_y, line.clone()))
    });

    f.render_widget(
        Paragraph::new(lines).scroll((scroll_offset, 0)),
        list_area,
    );
    TreeRenderResult {
        tree_scroll: state.tree.scroll_offset,
        worktree_scroll: scroll_offset as usize,
        cursor_pos,
        overflow_line,
    }
}

pub fn render_tree_view(f: &mut Frame, area: Rect, state: &AppState) {
    let _ = render_tree_view_inner(f, area, state);
}

struct TreeRenderResult {
    tree_scroll: u16,
    worktree_scroll: usize,
    cursor_pos: Option<Position>,
    overflow_line: Option<(u16, u16, Line<'static>)>,
}

fn render_tree_view_inner(f: &mut Frame, area: Rect, state: &AppState) -> TreeRenderResult {
    let tree_focused = state.active_focus == PaneFocus::Tree
        || state.focused_pane == PaneFocus::Tree
        || state.focus == PaneFocus::Tree;
    let theme = &state.theme_palette;

    if state.sidebar_view == SidebarView::Worktrees {
        return render_worktrees_view(f, area, state, tree_focused);
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

    let block = rounded_block(theme, tree_focused).title(render_sidebar_title(state));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return TreeRenderResult {
            tree_scroll: state.tree.scroll_offset,
            worktree_scroll: state.worktree_scroll_offset,
            cursor_pos: None,
            overflow_line: None,
        };
    }

    let (bottom_h, cursor_pos) = render_sidebar_bottom_panel(f, inner, state, tree_focused);
    let list_area = Rect::new(
        inner.x,
        inner.y,
        inner.width,
        inner.height.saturating_sub(bottom_h),
    );
    let scroll_offset = compute_tree_scroll_offset(
        state.tree.scroll_offset,
        selected_line_idx,
        lines.len(),
        list_area.height,
    );
    let overflow_line = selected_line_idx.and_then(|sel_idx| {
        if sel_idx < scroll_offset as usize {
            return None;
        }
        let rel_y = (sel_idx - scroll_offset as usize) as u16;
        if rel_y >= list_area.height {
            return None;
        }
        let line = lines.get(sel_idx)?;
        (line.width() > list_area.width as usize)
            .then(|| (list_area.x, list_area.y + rel_y, line.clone()))
    });

    if list_area.height > 0 {
        f.render_widget(Paragraph::new(lines).scroll((scroll_offset, 0)), list_area);
    }
    TreeRenderResult {
        tree_scroll: scroll_offset,
        worktree_scroll: state.worktree_scroll_offset,
        cursor_pos,
        overflow_line,
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
    worktree_diff_scroll: u16,
    worktree_diff_max_scroll: u16,
    transcript_inner_height: u16,
    composer_scroll_y: u16,
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
        state.help_max_scroll = metrics.help_max_scroll;
    } else {
        state.help_max_scroll = 0;
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

fn render_subagent_strip(
    f: &mut Frame,
    area: Rect,
    subagents: &[SubagentSnapshot],
    theme: &ThemePalette,
) {
    let now_ms = crate::runner::now_epoch_ms();
    let mut lines: Vec<Line<'static>> = Vec::new();
    for sa in subagents.iter().take(5) {
        let (glyph, color, default_act) = match sa.status {
            SubagentStatus::Running => ("●", theme.warning, "working"),
            SubagentStatus::Completed => ("✓", theme.success, "done"),
            SubagentStatus::Failed => ("✖", theme.error, "failed"),
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
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
        ];
        if let Some(ref model) = sa.model {
            if !model.trim().is_empty() {
                spans.push(Span::styled(
                    format!(" · {model}"),
                    Style::default().fg(theme.muted),
                ));
            }
        }
        spans.push(Span::styled(
            format!("  {activity} · {elapsed}"),
            Style::default().fg(theme.text),
        ));
        lines.push(Line::from(spans));
    }

    if subagents.len() > 5 {
        let overflow = subagents.len() - 5;
        lines.push(Line::from(Span::styled(
            format!("+{overflow} more"),
            Style::default().fg(theme.muted),
        )));
    }

    let block = rounded_block(theme, false).title(" Subagents ");
    let widget = Paragraph::new(lines).block(block);
    f.render_widget(widget, area);
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

    let theme = &state.theme_palette;
    let bar_bg = theme.status_bar_bg;
    let active_id = state.active_thread_id();
    let active_is_terminal = state.is_terminal_thread(active_id);
    let tree_focused = state.active_focus == PaneFocus::Tree
        || state.focused_pane == PaneFocus::Tree
        || state.focus == PaneFocus::Tree;

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

    let (pill_text, pill_bg, pill_fg) = if tree_focused {
        match state.sidebar_view {
            SidebarView::Grein => (
                " MANAGE · GREIN ",
                theme.pill_manage_bg,
                theme.pill_manage_fg,
            ),
            SidebarView::Worktrees => (
                " MANAGE · WORKTREES ",
                theme.pill_manage_bg,
                theme.pill_manage_fg,
            ),
        }
    } else if active_is_terminal {
        (" WRITE · SHELL ", theme.pill_write_bg, theme.pill_write_fg)
    } else {
        (" WRITE · AGENT ", theme.pill_write_bg, theme.pill_write_fg)
    };

    let (mode, model, live_status, status_color) = if active_is_terminal {
        let (st, col) = match state.terminal_exit_code(active_id) {
            Some(Some(0)) => ("✓ exit 0".to_string(), theme.muted),
            Some(Some(code)) => (format!("✗ exit {code}"), theme.error),
            Some(None) => ("✓ exited".to_string(), theme.muted),
            None => ("● live".to_string(), theme.success),
        };
        (
            "shell".to_string(),
            crate::app::active_shell_label(),
            st,
            col,
        )
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
            (format!("{spin} tool: {tool}"), theme.warning)
        } else if let Some(th) = thought {
            let preview = truncate_single_line(th, 24);
            (format!("{spin} thinking: {preview}"), theme.warning)
        } else {
            match status {
                "thinking" => (format!("{spin} thinking"), theme.warning),
                "executing" => (format!("{spin} executing"), theme.warning),
                "streaming" => (format!("{spin} streaming"), theme.warning),
                "error" => ("⚠ error".to_string(), theme.error),
                "permission" | "permission_required" => {
                    ("⚠ permission".to_string(), theme.error)
                }
                _ => ("✓ idle".to_string(), theme.success),
            }
        };
        (mode, model, live_status, status_color)
    };

    let sa_label = if subagent_count == 1 {
        "1 subagent".to_string()
    } else {
        format!("{subagent_count} subagents")
    };

    let git_meta = state.thread_meta.get(active_id).and_then(|m| {
        m.git_branch
            .as_deref()
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .map(|b| (b, m.git_dirty))
    });
    let git_badge = git_meta.map(|(branch, dirty)| {
        let raw = if dirty > 0 {
            format!("⎇ {branch}*")
        } else {
            format!("⎇ {branch}")
        };
        let capped = truncate_single_line(&raw, 28);
        let color = if dirty > 0 { theme.dirty } else { theme.accent };
        (capped, color)
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

    let mut spans = vec![
        Span::styled(
            pill_text,
            Style::default()
                .fg(pill_fg)
                .bg(pill_bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ", Style::default().bg(bar_bg)),
    ];
    let mut left_len = pill_text.chars().count() + 1;

    if search.is_visible() {
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
            theme.error
        } else {
            theme.warning
        };
        spans.push(Span::styled(
            query_part.clone(),
            Style::default()
                .fg(theme.accent)
                .bg(bar_bg)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            counter_part.clone(),
            Style::default().fg(counter_color).bg(bar_bg),
        ));
        left_len += query_part.chars().count() + counter_part.chars().count();
        if left_len + search_hint.chars().count() + 1 + total_right_chars <= area.width as usize {
            spans.push(Span::styled(
                search_hint,
                Style::default().fg(theme.muted).bg(bar_bg),
            ));
            left_len += search_hint.chars().count();
        }
        if !scrollback_badge.is_empty() {
            spans.push(Span::styled(
                scrollback_badge.clone(),
                Style::default()
                    .fg(theme.warning)
                    .bg(bar_bg)
                    .add_modifier(Modifier::BOLD),
            ));
            left_len += scrollback_badge.chars().count();
        }
    } else {
        let manage_key = state
            .keymap
            .chords_for(BindableAction::EnterManageMode)
            .first()
            .map(|c| c.display_label())
            .unwrap_or_else(|| "Ctrl+G".to_string());
        let zoom_key = state
            .keymap
            .chords_for(BindableAction::ToggleZoom)
            .first()
            .map(|c| c.display_label())
            .unwrap_or_else(|| "Alt+Z".to_string());

        let left_hint = if tree_focused {
            match state.sidebar_view {
                SidebarView::Grein => {
                    "Enter write · a new · f fork · t shell · r rename · d del · : cmd · ? help"
                        .to_string()
                }
                SidebarView::Worktrees => {
                    "Enter attach · p prune · y copy · d del · / filter · : cmd · ? help"
                        .to_string()
                }
            }
        } else if active_is_terminal {
            if state.terminal_exit_code(active_id).is_some() {
                format!("exited · Enter restart · Esc/{manage_key} manage")
            } else {
                format!("{manage_key} manage · {zoom_key} zoom · Shift+PgUp/PgDn scroll")
            }
        } else if is_busy {
            format!("Esc cancel · {manage_key} manage · {zoom_key} zoom")
        } else {
            format!("{manage_key} manage · {zoom_key} zoom · /theme · /help")
        };

        if let Some(notice) = state.status_notice.as_deref() {
            let notice_text = format!(" {notice} ");
            left_len += notice_text.chars().count();
            spans.push(Span::styled(
                notice_text,
                Style::default()
                    .fg(theme.warning)
                    .bg(bar_bg)
                    .add_modifier(Modifier::BOLD),
            ));
            let remaining_for_hint = (area.width as usize)
                .saturating_sub(left_len + total_right_chars + scrollback_badge.chars().count() + 4);
            if remaining_for_hint >= 16 {
                let compact_hint = truncate_single_line(&format!("· {left_hint}"), remaining_for_hint);
                left_len += compact_hint.chars().count();
                spans.push(Span::styled(
                    compact_hint,
                    Style::default().fg(theme.muted).bg(bar_bg),
                ));
            }
        } else {
            let max_hint = (area.width as usize)
                .saturating_sub(left_len + total_right_chars + scrollback_badge.chars().count() + 1)
                .max(12);
            let fitted_hint = truncate_single_line(&left_hint, max_hint);
            left_len += fitted_hint.chars().count();
            spans.push(Span::styled(
                fitted_hint,
                Style::default().fg(theme.muted).bg(bar_bg),
            ));
        }
        if !scrollback_badge.is_empty() {
            left_len += scrollback_badge.chars().count();
            spans.push(Span::styled(
                scrollback_badge,
                Style::default()
                    .fg(theme.warning)
                    .bg(bar_bg)
                    .add_modifier(Modifier::BOLD),
            ));
        }
    }

    let width = area.width as usize;
    if left_len + 1 + total_right_chars <= width {
        let gap = width - left_len - total_right_chars;
        spans.push(Span::styled(" ".repeat(gap), Style::default().bg(bar_bg)));
        if let Some((badge, badge_color)) = git_badge {
            spans.push(Span::styled(
                badge,
                Style::default().fg(badge_color).bg(bar_bg),
            ));
            spans.push(Span::styled(
                " · ",
                Style::default().fg(theme.muted).bg(bar_bg),
            ));
        }
        spans.push(Span::styled(
            right_prefix,
            Style::default().fg(theme.muted).bg(bar_bg),
        ));
        spans.push(Span::styled(
            live_status,
            Style::default().fg(status_color).bg(bar_bg),
        ));
        spans.push(Span::styled(
            right_suffix,
            Style::default().fg(theme.muted).bg(bar_bg),
        ));
    } else if left_len < width {
        let remaining = width - left_len;
        if remaining >= 2 {
            spans.push(Span::styled(" ", Style::default().bg(bar_bg)));
            let compact_git = git_badge
                .as_ref()
                .map(|(badge, _)| format!("{badge} · "))
                .unwrap_or_default();
            let compact_right = truncate_single_line(
                &format!("{compact_git}{right_prefix}{live_status}{right_suffix}"),
                remaining - 1,
            );
            let pad = remaining
                .saturating_sub(1 + compact_right.chars().count());
            spans.push(Span::styled(
                compact_right,
                Style::default().fg(theme.muted).bg(bar_bg),
            ));
            if pad > 0 {
                spans.push(Span::styled(" ".repeat(pad), Style::default().bg(bar_bg)));
            }
        }
    }

    let widget = Paragraph::new(Line::from(spans)).style(Style::default().bg(bar_bg));
    f.render_widget(widget, area);
}

fn render_config_picker_modal(
    f: &mut Frame,
    area: Rect,
    picker: &ConfigPickerModal,
    theme: &ThemePalette,
) {
    let popup_width = area.width.saturating_sub(6).clamp(32, 74).min(area.width);
    let content_rows = picker.options.len() + 3;
    let popup_height = (content_rows as u16 + 2)
        .min(area.height.saturating_sub(2).max(7))
        .min(area.height.max(1));
    let popup_x = area.x + area.width.saturating_sub(popup_width) / 2;
    let popup_y = area.y + area.height.saturating_sub(popup_height) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    f.render_widget(Clear, popup_area);
    let block = rounded_block(theme, true).title(format!(" Select {} ", picker.title));
    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let scope_style = if picker.set_default {
        Style::default()
            .fg(theme.warning)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme.accent)
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
        Span::styled(" Scope: ", Style::default().fg(theme.muted)),
        Span::styled(format!("[{scope_label}]"), scope_style),
        Span::styled(
            format!("  (Tab/d: {scope_alt})"),
            Style::default().fg(theme.muted),
        ),
    ]);

    let footer_line = Line::from(Span::styled(
        " ↑/↓/j/k Move · Enter Apply · Tab/d Scope · Esc/q Close",
        Style::default().fg(theme.muted),
    ));

    let opt_height = inner.height.saturating_sub(2) as usize;
    let mut opt_lines = Vec::with_capacity(picker.options.len());
    for (idx, choice) in picker.options.iter().enumerate() {
        let is_selected = idx == picker.selected_index;
        let is_active = choice.value == picker.current_value;
        let marker = if is_selected { "▌ " } else { "  " };
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
            let bg = theme.selection_bg;
            let sel_style = Style::default()
                .fg(theme.selection_fg)
                .bg(bg)
                .add_modifier(Modifier::BOLD);
            let mut spans = vec![Span::styled(
                marker,
                Style::default().fg(theme.selection_bar).bg(bg),
            )];
            if is_active {
                spans.push(Span::styled(
                    "✓ ",
                    Style::default()
                        .fg(theme.success)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled("  ", Style::default().bg(bg)));
            }
            spans.push(Span::styled(label, sel_style));
            if let Some(d) = desc {
                spans.push(Span::styled(
                    format!(" — {d}"),
                    Style::default().fg(theme.muted).bg(bg),
                ));
            }
            opt_lines.push(Line::from(spans));
        } else if is_active {
            let active_style = Style::default()
                .fg(theme.success)
                .add_modifier(Modifier::BOLD);
            let mut spans = vec![
                Span::raw(marker),
                Span::styled("✓ ", active_style),
                Span::styled(label, active_style),
            ];
            if let Some(d) = desc {
                spans.push(Span::styled(
                    format!(" — {d}"),
                    Style::default().fg(theme.muted),
                ));
            }
            opt_lines.push(Line::from(spans));
        } else {
            let mut spans = vec![
                Span::raw(marker),
                Span::raw("  "),
                Span::styled(label, Style::default().fg(theme.text)),
            ];
            if let Some(d) = desc {
                spans.push(Span::styled(
                    format!(" — {d}"),
                    Style::default().fg(theme.muted),
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

fn render_theme_picker_modal(
    f: &mut Frame,
    area: Rect,
    picker: &ThemePickerModal,
    state: &AppState,
) {
    let theme = &state.theme_palette;
    let presets = ThemePreset::all();
    let popup_width = area.width.saturating_sub(6).clamp(32, 62).min(area.width);
    let popup_height = (presets.len() as u16 + 5)
        .min(area.height.saturating_sub(2).max(7))
        .min(area.height.max(1));
    let popup_x = area.x + area.width.saturating_sub(popup_width) / 2;
    let popup_y = area.y + area.height.saturating_sub(popup_height) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    f.render_widget(Clear, popup_area);
    let block = rounded_block(theme, true).title(" Select Theme (Live Preview) ");
    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let header = Line::from(vec![
        Span::styled(" Saved: ", Style::default().fg(theme.muted)),
        Span::styled(
            picker.original_preset.display_name().to_string(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
    ]);

    let mut lines = Vec::with_capacity(presets.len());
    for (idx, preset) in presets.iter().enumerate() {
        let is_selected = idx == picker.selected_idx;
        let is_original = *preset == picker.original_preset;
        let suffix = if is_original { " (saved)" } else { "" };
        if is_selected {
            lines.push(Line::from(vec![
                Span::styled(
                    "▌ ",
                    Style::default()
                        .fg(theme.selection_bar)
                        .bg(theme.selection_bg),
                ),
                Span::styled(
                    format!("{} [{}]{suffix}", preset.display_name(), preset.as_str()),
                    Style::default()
                        .fg(theme.selection_fg)
                        .bg(theme.selection_bg)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!("{} [{}]", preset.display_name(), preset.as_str()),
                    Style::default().fg(theme.text),
                ),
                Span::styled(suffix, Style::default().fg(theme.muted)),
            ]));
        }
    }

    let footer = Line::from(Span::styled(
        " ↑/↓/j/k Preview · Enter Confirm · Esc/q Revert",
        Style::default().fg(theme.muted),
    ));

    f.render_widget(
        Paragraph::new(header),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let list_h = inner.height.saturating_sub(2);
    if list_h > 0 {
        f.render_widget(
            Paragraph::new(lines),
            Rect::new(inner.x, inner.y + 1, inner.width, list_h),
        );
    }
    if inner.height >= 2 {
        f.render_widget(
            Paragraph::new(footer),
            Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
        );
    }
}

fn render_help_modal(f: &mut Frame, area: Rect, state: &AppState) -> (u16, u16) {
    let theme = &state.theme_palette;
    let section_style = Style::default()
        .fg(theme.warning)
        .add_modifier(Modifier::BOLD);
    let key_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);

    let km = &state.keymap;
    let enter_manage = km.display_for(BindableAction::EnterManageMode);
    let zoom_key = km.display_for(BindableAction::ToggleZoom);
    let scroll_up = km.display_for(BindableAction::ScrollUp);
    let scroll_down = km.display_for(BindableAction::ScrollDown);
    let scroll_label = if scroll_up == "Alt+PgUp / Shift+PgUp"
        && scroll_down == "Alt+PgDn / Shift+PgDn"
    {
        "Alt/Shift+PgUp/PgDn".to_string()
    } else {
        format!("{scroll_up} / {scroll_down}")
    };

    let sections: [(&str, Vec<(String, String)>); 4] = [
        (
            "── WRITE Mode (Terminal & Composer) ──",
            vec![
                (
                    enter_manage,
                    "Toggle WRITE <-> MANAGE (sidebar)".to_string(),
                ),
                (zoom_key, "Toggle sidebar zoom (full width)".to_string()),
                (scroll_label, "Scroll active pane viewport".to_string()),
                (
                    "Ctrl+C (Composer)".to_string(),
                    "Cancel turn / clear input / exit hint".to_string(),
                ),
                (
                    "Shift+Enter / Ctrl+J".to_string(),
                    "Newline in Composer (Enter sends)".to_string(),
                ),
                (
                    "Ctrl+A/E/B/F/W/U/K/Y".to_string(),
                    "Readline editing; AltGr chars preserved".to_string(),
                ),
                (
                    "Shell (WRITE)".to_string(),
                    format!(
                        "All keys pass straight to >_ {}",
                        crate::app::active_shell_label()
                    ),
                ),
            ],
        ),
        (
            "── MANAGE Mode (Sidebar) ──",
            vec![
                (
                    "Tab / Ctrl+T".to_string(),
                    "Switch ᛇ Grein <-> ⎇ Worktrees".to_string(),
                ),
                (
                    "j/k/↑/↓, Enter/l, Esc".to_string(),
                    "Browse / enter WRITE (Space/h/l fold)".to_string(),
                ),
                (
                    "gg / G, J / K".to_string(),
                    "Top/bottom item; J/K switch root".to_string(),
                ),
                (
                    "PgUp/PgDn, Ctrl+U/D".to_string(),
                    "Scroll right-pane preview (diff/chat/>_)".to_string(),
                ),
                (
                    "a / f / r / d / x".to_string(),
                    "New root / fork / inline rename / delete".to_string(),
                ),
                (
                    "t / T, w / m / o / y".to_string(),
                    ">_ jump/new, worktree/join/cards/copy".to_string(),
                ),
                (
                    ": , /, ? / F1, z, q".to_string(),
                    "Command palette / search / help / zoom / quit".to_string(),
                ),
            ],
        ),
        (
            "── Worktrees View ──",
            vec![
                (
                    "Enter / l, y".to_string(),
                    "Attach worktree & write / copy path".to_string(),
                ),
                (
                    "t, w/a, p, r".to_string(),
                    "Shell here / allocate / prune / refresh".to_string(),
                ),
                (
                    "d / x, /".to_string(),
                    "Delete worktree (confirm) / filter".to_string(),
                ),
            ],
        ),
        (
            "── Palette & Slash Commands ──",
            vec![
                (
                    "/theme [name]".to_string(),
                    "Live theme picker or set preset".to_string(),
                ),
                (
                    "/bind /unbind /keybinds".to_string(),
                    "Customize WRITE keybindings".to_string(),
                ),
                (
                    "/sidebar [dynamic|fixed]".to_string(),
                    "Configure sidebar sizing & overflow".to_string(),
                ),
                (
                    "/model /mode /thinking".to_string(),
                    "Configure agent model, mode, effort".to_string(),
                ),
                (
                    "/root /fork /join /term".to_string(),
                    "Context & shell (!cmd, /wt, /prune)".to_string(),
                ),
                (
                    "/copy [code|all], /quit".to_string(),
                    "Clipboard export, /commands, /zoom, /q".to_string(),
                ),
            ],
        ),
    ];

    let mut raw_body: Vec<Line<'static>> = Vec::new();
    for (title, rows) in sections {
        raw_body.push(Line::from(Span::styled(format!(" {title}"), section_style)));
        for (k, desc) in rows {
            raw_body.push(Line::from(vec![
                Span::styled(format!(" {k:<25} "), key_style),
                Span::styled(desc, Style::default().fg(theme.text)),
            ]));
        }
    }

    let popup_width = area.width.saturating_sub(6).clamp(30, 76).min(area.width);
    let inner_width = popup_width.saturating_sub(2) as usize;
    let wrapped_body = crate::markdown::wrap_styled_lines(raw_body, inner_width);

    let popup_height = (wrapped_body.len() as u16 + 3).min(area.height.max(1));
    let popup_x = area.x + area.width.saturating_sub(popup_width) / 2;
    let popup_y = area.y + area.height.saturating_sub(popup_height) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    f.render_widget(Clear, popup_area);
    let block = rounded_block(theme, true).title(" Keybindings & Commands ");
    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    if inner.width == 0 || inner.height == 0 {
        return (0, 0);
    }

    let body_height = inner.height.saturating_sub(1) as usize;
    let max_scroll = wrapped_body
        .len()
        .saturating_sub(body_height)
        .min(u16::MAX as usize) as u16;
    let scroll_y = state.help_scroll_offset.min(max_scroll);

    if body_height > 0 {
        f.render_widget(
            Paragraph::new(wrapped_body).scroll((scroll_y, 0)),
            Rect::new(inner.x, inner.y, inner.width, body_height as u16),
        );
    }

    let footer_text = if max_scroll > 0 {
        " Esc/q/?/F1 Close · ↑/↓/j/k/PgUp/PgDn/Ctrl+U/D/g/G Scroll"
    } else {
        " Esc, q, ?, F1, or Enter to close"
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            footer_text,
            Style::default().fg(theme.muted),
        ))),
        Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
    );
    (scroll_y, max_scroll)
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

    let theme = &state.theme_palette;
    let borders = if state.sidebar_hidden {
        Borders::TOP | Borders::BOTTOM
    } else {
        Borders::ALL
    };
    let raw_title = state
        .threads
        .iter()
        .find(|t| t.id == display_thread_id)
        .map(|t| t.title.as_str())
        .unwrap_or("Terminal");
    let display_title = state.display_title_for(display_thread_id, raw_title);
    let short_cwd = state
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
        .map(shorten_terminal_cwd);

    let left_title = match preview_title {
        Some(_) => match short_cwd {
            Some(cwd) => format!(" >_ {display_title} · {cwd} (Preview) "),
            None => format!(" >_ {display_title} (Preview) "),
        },
        None => match short_cwd {
            Some(cwd) => format!(" >_ {display_title} · {cwd} "),
            None => format!(" >_ {display_title} "),
        },
    };
    let (status_pill, status_color) = match state.terminal_exit_code(display_thread_id) {
        Some(Some(0)) => (" ✓ exit 0 ".to_string(), theme.success),
        Some(Some(c)) => (format!(" ✗ exit {c} "), theme.error),
        Some(None) => (" ✓ exited ".to_string(), theme.muted),
        None => (" ● live ".to_string(), theme.success),
    };

    let mut block = Block::default()
        .title(left_title)
        .title_top(
            Line::from(Span::styled(
                status_pill,
                Style::default()
                    .fg(status_color)
                    .add_modifier(Modifier::BOLD),
            ))
            .right_aligned(),
        )
        .borders(borders)
        .border_type(BorderType::Rounded)
        .border_style(themed_border_style(theme, terminal_focused));
    if theme.panel_bg != Color::Reset {
        block = block.style(Style::default().bg(theme.panel_bg).fg(theme.text));
    }
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
                Style::default().fg(theme.muted),
            ))),
            inner,
        );
    }

    Some((inner.height, inner.width))
}

fn render_worktree_diff_pane(
    f: &mut Frame,
    area: Rect,
    state: &AppState,
) -> (u16, u16) {
    if area.width == 0 || area.height == 0 {
        return (0, 0);
    }
    let theme = &state.theme_palette;
    let selected = state.selected_worktree_entry();
    let (title, right_badge, mut raw_lines) = if let Some(entry) = selected {
        let label = worktree_display_label(&entry);
        let right = if entry.dirty_count > 0 {
            (format!(" *{} modified ", entry.dirty_count), theme.dirty)
        } else {
            (" ✓ clean ".to_string(), theme.success)
        };
        let mut lines = vec![
            Line::from(vec![
                Span::styled(" Repo: ", Style::default().fg(theme.muted)),
                Span::styled(
                    entry.repo_name.clone(),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("  Path: ", Style::default().fg(theme.muted)),
                Span::styled(entry.path.clone(), Style::default().fg(theme.text)),
            ]),
            Line::from(vec![
                Span::styled(" Attached: ", Style::default().fg(theme.muted)),
                Span::styled(
                    if entry.attached_thread_ids.is_empty() {
                        "none".to_string()
                    } else {
                        entry.attached_thread_ids.join(", ")
                    },
                    Style::default().fg(theme.text),
                ),
            ]),
            Line::raw(""),
        ];
        if state.worktree_diff_in_flight.as_deref() == Some(entry.path.as_str())
            && state.worktree_diff_preview.is_empty()
        {
            lines.push(Line::from(Span::styled(
                " Loading git diff preview...",
                Style::default().fg(theme.muted),
            )));
        } else if state.worktree_diff_preview.trim().is_empty() {
            lines.push(Line::from(Span::styled(
                " Clean working tree (no uncommitted changes)",
                Style::default().fg(theme.muted),
            )));
        } else {
            for diff_line in state.worktree_diff_preview.lines() {
                let style = if diff_line.starts_with("diff ")
                    || diff_line.starts_with("index ")
                    || diff_line.starts_with("+++")
                    || diff_line.starts_with("---")
                {
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD)
                } else if diff_line.starts_with("@@") {
                    Style::default().fg(theme.diff_hunk)
                } else if diff_line.starts_with('+') {
                    Style::default().fg(theme.diff_add)
                } else if diff_line.starts_with('-') {
                    Style::default().fg(theme.diff_del)
                } else {
                    Style::default().fg(theme.text)
                };
                lines.push(Line::from(Span::styled(diff_line.to_string(), style)));
            }
        }
        (format!(" ⎇ Diff · {label} "), right, lines)
    } else {
        (
            " ⎇ Worktree Preview ".to_string(),
            (
                format!(" {} worktrees ", state.worktrees.len()),
                theme.muted,
            ),
            vec![
                Line::raw(""),
                Line::from(Span::styled(
                    " Select a worktree in the sidebar to inspect its uncommitted diff.",
                    Style::default().fg(theme.muted),
                )),
                Line::from(Span::styled(
                    " Press Enter to attach · t for shell · p to prune stale worktrees.",
                    Style::default().fg(theme.muted),
                )),
            ],
        )
    };

    let block = rounded_block(theme, false)
        .title(title)
        .title_top(Line::from(Span::styled(
            right_badge.0,
            Style::default()
                .fg(right_badge.1)
                .add_modifier(Modifier::BOLD),
        )).right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return (0, 0);
    }

    let wrapped = crate::markdown::wrap_styled_lines(
        std::mem::take(&mut raw_lines),
        inner.width as usize,
    );
    let max_scroll = wrapped
        .len()
        .saturating_sub(inner.height as usize)
        .min(u16::MAX as usize) as u16;
    let scroll_y = state.worktree_diff_scroll.min(max_scroll);
    f.render_widget(Paragraph::new(wrapped).scroll((scroll_y, 0)), inner);
    (scroll_y, max_scroll)
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

fn render_worktree_delete_modal(
    f: &mut Frame,
    area: Rect,
    pending: &PendingWorktreeDelete,
    theme: &ThemePalette,
) {
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
                .fg(theme.error)
                .add_modifier(Modifier::BOLD),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            format!("Delete worktree {}? [y/N]", pending.label),
            Style::default()
                .fg(theme.text)
                .add_modifier(Modifier::BOLD),
        )));
    }
    lines.push(Line::from(Span::styled(
        format!("Path: {}", pending.path),
        Style::default().fg(theme.muted),
    )));
    if pending.attached_count > 0 {
        lines.push(Line::from(Span::styled(
            format!(
                "{} attached thread(s) will reset to repo root",
                pending.attached_count
            ),
            Style::default().fg(theme.warning),
        )));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "[y] Delete  [n/Esc] Cancel",
        Style::default()
            .fg(theme.accent)
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
        theme.error
    } else {
        theme.warning
    };
    let mut block = Block::default()
        .title(" Delete Worktree ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        );
    if theme.panel_bg != Color::Reset {
        block = block.style(Style::default().bg(theme.panel_bg).fg(theme.text));
    }
    let widget = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(widget, popup_area);
}

fn render_thread_delete_modal(
    f: &mut Frame,
    area: Rect,
    pending: &PendingThreadDelete,
    theme: &ThemePalette,
) {
    let popup_width = area.width.saturating_sub(8).clamp(28, 64).min(area.width);
    let display = if pending.title.trim().is_empty() {
        pending.thread_id.clone()
    } else {
        pending.title.clone()
    };
    let lines = vec![
        Line::raw(""),
        Line::from(Span::styled(
            format!("Delete context \"{display}\"?"),
            Style::default()
                .fg(theme.text)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "Child branches/terminals under it may also be removed.",
            Style::default().fg(theme.muted),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            "[y] Delete  [n/Esc] Cancel",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
    ];
    let popup_height = (lines.len() as u16 + 3)
        .max(8)
        .min(area.height.max(1));
    let popup_x = area.x + area.width.saturating_sub(popup_width) / 2;
    let popup_y = area.y + area.height.saturating_sub(popup_height) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    f.render_widget(Clear, popup_area);
    let mut block = Block::default()
        .title(" Delete Context ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        );
    if theme.panel_bg != Color::Reset {
        block = block.style(Style::default().bg(theme.panel_bg).fg(theme.text));
    }
    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: false }),
        popup_area,
    );
}

fn render_quit_confirm_modal(
    f: &mut Frame,
    area: Rect,
    pending: &PendingQuitConfirm,
    theme: &ThemePalette,
) {
    let popup_width = area.width.saturating_sub(8).clamp(28, 60).min(area.width);
    let mut details = Vec::new();
    if pending.active_terminals > 0 {
        details.push(format!("{} live shell(s)", pending.active_terminals));
    }
    if pending.busy_agents > 0 {
        details.push(format!("{} running agent turn(s)", pending.busy_agents));
    }
    let summary = details.join(" and ");
    let lines = vec![
        Line::raw(""),
        Line::from(Span::styled(
            format!("Quit Grein while {summary} active?"),
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            "[y] Quit  [n/Esc] Cancel",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
    ];
    let popup_height = (lines.len() as u16 + 3)
        .max(7)
        .min(area.height.max(1));
    let popup_x = area.x + area.width.saturating_sub(popup_width) / 2;
    let popup_y = area.y + area.height.saturating_sub(popup_height) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    f.render_widget(Clear, popup_area);
    let mut block = Block::default()
        .title(" Confirm Quit ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        );
    if theme.panel_bg != Color::Reset {
        block = block.style(Style::default().bg(theme.panel_bg).fg(theme.text));
    }
    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: false }),
        popup_area,
    );
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
    let theme = &state.theme_palette;

    f.render_widget(Clear, popup_area);
    let block = rounded_block(theme, true).title(" Worktrees (Tab complete · Enter attach) ");
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
            let bg = theme.selection_bg;
            let sel_style = Style::default()
                .fg(theme.selection_fg)
                .bg(bg)
                .add_modifier(Modifier::BOLD);
            match item {
                WorktreeAutocompleteItem::CreateNew => {
                    if is_selected {
                        Line::from(vec![
                            Span::styled(
                                "▌ ",
                                Style::default().fg(theme.selection_bar).bg(bg),
                            ),
                            Span::styled("+ New isolated worktree", sel_style),
                            Span::styled(
                                " — create branch worktree",
                                Style::default().fg(theme.muted).bg(bg),
                            ),
                        ])
                    } else {
                        Line::from(vec![
                            Span::styled(
                                "  + New isolated worktree",
                                Style::default()
                                    .fg(theme.success)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                " — create branch worktree",
                                Style::default().fg(theme.muted),
                            ),
                        ])
                    }
                }
                WorktreeAutocompleteItem::Existing(entry) => {
                    let branch = worktree_display_label(entry);
                    let primary_tag = if entry.is_primary { " (primary)" } else { "" };
                    if is_selected {
                        let mut spans = vec![
                            Span::styled(
                                "▌ ",
                                Style::default().fg(theme.selection_bar).bg(bg),
                            ),
                            Span::styled(
                                format!("{} │ {branch}{primary_tag}", entry.repo_name),
                                sel_style,
                            ),
                        ];
                        if entry.dirty_count > 0 {
                            spans.push(Span::styled(" │ ", sel_style));
                            spans.push(Span::styled(
                                format!("*{}", entry.dirty_count),
                                Style::default()
                                    .fg(theme.dirty)
                                    .bg(bg)
                                    .add_modifier(Modifier::BOLD),
                            ));
                        }
                        if !entry.attached_thread_ids.is_empty() {
                            spans.push(Span::styled(
                                format!(" [{}]", entry.attached_thread_ids.len()),
                                Style::default().fg(theme.muted).bg(bg),
                            ));
                        }
                        Line::from(spans)
                    } else {
                        let mut spans = vec![
                            Span::raw("  "),
                            Span::styled(
                                entry.repo_name.clone(),
                                Style::default().fg(theme.accent),
                            ),
                            Span::styled(" │ ", Style::default().fg(theme.muted)),
                            Span::styled(
                                format!("{branch}{primary_tag}"),
                                Style::default().fg(theme.text),
                            ),
                        ];
                        if entry.dirty_count > 0 {
                            spans.push(Span::styled(" │ ", Style::default().fg(theme.muted)));
                            spans.push(Span::styled(
                                format!("*{}", entry.dirty_count),
                                Style::default().fg(theme.dirty),
                            ));
                        }
                        if !entry.attached_thread_ids.is_empty() {
                            spans.push(Span::styled(
                                format!(" [{}]", entry.attached_thread_ids.len()),
                                Style::default().fg(theme.muted),
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
            transcript_block =
                transcript_block.style(Style::default().bg(theme.panel_bg));
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
    if let Some(perm) = &state.pending_permission {
        let area = f.area();
        let popup_width = area.width.saturating_sub(8).clamp(20, 64).min(area.width);
        let popup_height = 9u16.min(area.height.max(1));
        let popup_x = area.x + area.width.saturating_sub(popup_width) / 2;
        let popup_y = area.y + area.height.saturating_sub(popup_height) / 2;
        let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

        f.render_widget(Clear, popup_area);
        let mut popup_block = Block::default()
            .title(" Permission Required ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(
                Style::default()
                    .fg(theme.warning)
                    .add_modifier(Modifier::BOLD),
            );
        if theme.panel_bg != Color::Reset {
            popup_block = popup_block.style(Style::default().bg(theme.panel_bg).fg(theme.text));
        }
        let popup_lines = vec![
            Line::raw(""),
            Line::from(Span::styled(
                perm.description.clone(),
                Style::default().fg(theme.text),
            )),
            Line::raw(""),
            Line::from(Span::styled(
                "[y] Allow  [n] Deny",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )),
        ];
        let popup_widget = Paragraph::new(popup_lines)
            .block(popup_block)
            .wrap(Wrap { trim: false });
        f.render_widget(popup_widget, popup_area);
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
        help_scroll_offset: rendered_help_scroll,
        help_max_scroll,
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
        assert!(screen.contains("WRITE · AGENT"));
        assert!(screen.contains("Esc cancel · Ctrl+G manage · Alt+Z zoom"));
        assert!(!screen.contains("Enter: Send"));
    }
}
