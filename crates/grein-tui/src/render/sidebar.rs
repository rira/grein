use std::collections::{BTreeSet, HashMap};

use crate::app::{
    is_generic_agent_placeholder, worktree_display_label, AppState, PaneFocus, SidebarView,
    WorktreeRow,
};
use crate::runner::{compute_ordered_thread_entries_with_folds, compute_ordered_thread_indices};
use crate::theme::ThemePalette;
use crate::ui::tree::{compute_tree_scroll_offset, TreeNode};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::chrome::truncate_single_line;
use super::composer::clamp_char_boundary;
use super::layout::rounded_block;

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

pub(super) fn selected_tree_thread(state: &AppState) -> Option<(&str, &str)> {
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

#[allow(clippy::too_many_arguments)]
fn render_thread_row(
    id: &str,
    title: &str,
    depth: usize,
    root_number: Option<usize>,
    prefix: &str,
    hidden_descendants: usize,
    is_selected: bool,
    elide_sidebar: bool,
    state: &AppState,
) -> Line<'static> {
    let theme = &state.theme_palette;
    let is_active = id == state.active_thread_id;
    let is_merged = state.thread_meta.get(id).is_some_and(|m| m.merged);
    let is_terminal = state.is_terminal_thread(id);
    let exit_code_opt = state.terminal_exit_code(id);
    let parts = state.display_parts_for_elided(id, title, elide_sidebar);
    let root_badge = if depth == 0 {
        root_number
            .filter(|n| (1..=9).contains(n))
            .map(|n| format!("{n} "))
    } else {
        None
    };
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
        let sel_muted = Style::default().fg(theme.muted).bg(bg);
        let mut spans = vec![
            Span::styled("▌", Style::default().fg(theme.selection_bar).bg(bg)),
            Span::styled(prefix.to_string(), sel_muted),
        ];
        if let Some(num) = root_badge {
            spans.push(Span::styled(num, sel_muted.add_modifier(Modifier::DIM)));
        }
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
        if let Some(name) = parts.custom_name {
            spans.push(Span::styled(format!("[{name}]"), sel_title));
            if is_terminal || !is_generic_agent_placeholder(&parts.auto_label) {
                spans.push(Span::styled(format!(" {}", parts.auto_label), sel_muted));
            }
        } else {
            spans.push(Span::styled(parts.auto_label, sel_title));
        }
        if let Some(badge) = fold_badge {
            spans.push(Span::styled(badge, sel_muted));
        }
        if let Some((badge, color)) = exit_badge {
            spans.push(Span::styled(badge, Style::default().fg(color).bg(bg)));
        }
        if is_merged {
            spans.push(Span::styled(" [joined]", sel_muted));
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
        let muted_style = Style::default().fg(theme.muted);
        let mut spans = vec![
            Span::raw(" "),
            Span::styled(prefix.to_string(), muted_style),
        ];
        if let Some(num) = root_badge {
            spans.push(Span::styled(num, muted_style.add_modifier(Modifier::DIM)));
        }
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
        if let Some(name) = parts.custom_name {
            spans.push(Span::styled(format!("[{name}]"), title_style));
            if is_terminal || !is_generic_agent_placeholder(&parts.auto_label) {
                spans.push(Span::styled(format!(" {}", parts.auto_label), muted_style));
            }
        } else {
            spans.push(Span::styled(parts.auto_label, title_style));
        }
        if let Some(badge) = fold_badge {
            spans.push(Span::styled(badge, muted_style));
        }
        if let Some((badge, color)) = exit_badge {
            spans.push(Span::styled(badge, Style::default().fg(color)));
        }
        if is_merged {
            spans.push(Span::styled(" [joined]", muted_style));
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
            let prefix =
                &state.palette_input[..clamp_char_boundary(&state.palette_input, state.palette_cursor)];
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
            let agent_word = if agents == 1 { "agent" } else { "agents" };
            let shell_word = if shells == 1 { "shell" } else { "shells" };
            let text = format!("{agents} {agent_word} · {shells} {shell_word}");
            truncate_single_line(&text, inner.width as usize)
        }
        SidebarView::Worktrees => {
            let repos: BTreeSet<&str> = state
                .worktrees
                .iter()
                .map(|w| w.repo_root.as_str())
                .collect();
            let target_id = state.active_thread_id();
            let raw_title = state
                .threads
                .iter()
                .find(|t| t.id == target_id)
                .map(|t| t.title.as_str())
                .unwrap_or(target_id);
            let target_title = state.display_title_for(target_id, raw_title);
            let counts = format!(" · {} repos · {} wt", repos.len(), state.worktrees.len());
            let max_title = (inner.width as usize)
                .saturating_sub(8 + counts.chars().count())
                .max(6);
            let short_title = truncate_single_line(&target_title, max_title);
            let text = format!("Target: {short_title}{counts}");
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

pub(super) struct TreeRenderResult {
    pub(super) tree_scroll: u16,
    pub(super) worktree_scroll: usize,
    pub(super) cursor_pos: Option<Position>,
    pub(super) overflow_line: Option<(u16, u16, Line<'static>)>,
}

pub(super) fn render_tree_view_inner(
    f: &mut Frame,
    area: Rect,
    state: &AppState,
) -> TreeRenderResult {
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
    let mut selected_full_line: Option<Line<'static>> = None;

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

        let mut root_count = 0usize;
        for (node_idx, node) in state.tree.nodes.iter().enumerate() {
            if let TreeNode::Thread(t) = node {
                let is_selected = selected == Some(node_idx);
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
                let root_number = if depth == 0 {
                    root_count += 1;
                    (root_count <= 9).then_some(root_count)
                } else {
                    None
                };
                if is_selected {
                    selected_line_idx = Some(lines.len());
                    selected_full_line = Some(render_thread_row(
                        &t.id,
                        &t.title,
                        depth,
                        root_number,
                        prefix,
                        hidden_descendants,
                        true,
                        false,
                        state,
                    ));
                }
                lines.push(render_thread_row(
                    &t.id,
                    &t.title,
                    depth,
                    root_number,
                    prefix,
                    hidden_descendants,
                    is_selected,
                    true,
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
        let mut root_count = 0usize;
        for (row_idx, entry) in ordered.into_iter().enumerate() {
            let thread = &state.threads[entry.index];
            let is_selected = selected == Some(row_idx);
            let root_number = if entry.depth == 0 {
                root_count += 1;
                (root_count <= 9).then_some(root_count)
            } else {
                None
            };
            if is_selected {
                selected_line_idx = Some(lines.len());
                selected_full_line = Some(render_thread_row(
                    &thread.id,
                    &thread.title,
                    entry.depth,
                    root_number,
                    &entry.prefix,
                    entry.hidden_descendants,
                    true,
                    false,
                    state,
                ));
            }
            lines.push(render_thread_row(
                &thread.id,
                &thread.title,
                entry.depth,
                root_number,
                &entry.prefix,
                entry.hidden_descendants,
                is_selected,
                true,
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
        let line = selected_full_line.as_ref().or_else(|| lines.get(sel_idx))?;
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
