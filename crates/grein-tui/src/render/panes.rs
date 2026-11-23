use crate::app::{shorten_terminal_cwd, worktree_display_label, AppState};
use crate::engine_port::{PtyCell, PtyColor};
use crate::search::{SearchDirection, TranscriptSearchState};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;

use super::layout::{rounded_block, themed_border_style};

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

pub(super) fn render_terminal_pane(
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
    let parts = state.display_parts_for_elided(display_thread_id, raw_title, false);
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

    let label = match (parts.custom_name.as_deref(), short_cwd.as_deref()) {
        (Some(name), Some(cwd)) => format!("[{name}] · {cwd}"),
        (Some(name), None) => format!("[{name}]"),
        (None, Some(cwd)) => cwd.to_string(),
        (None, None) => parts.auto_label,
    };
    let left_title = match preview_title {
        Some(_) => format!(" >_ {label} (Preview) "),
        None => format!(" >_ {label} "),
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

pub(super) fn render_worktree_diff_pane(
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
                    " Press Enter to attach · s for shell · p to prune stale worktrees.",
                    Style::default().fg(theme.muted),
                )),
            ],
        )
    };

    let block = rounded_block(theme, false).title(title).title_top(
        Line::from(Span::styled(
            right_badge.0,
            Style::default()
                .fg(right_badge.1)
                .add_modifier(Modifier::BOLD),
        ))
        .right_aligned(),
    );
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return (0, 0);
    }

    let wrapped =
        crate::markdown::wrap_styled_lines(std::mem::take(&mut raw_lines), inner.width as usize);
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

pub(super) fn recompute_search_for_render(
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
