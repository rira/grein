use std::collections::HashMap;

use crate::app::{AppState, ConfigPickerModal, PaneFocus};
use crate::engine_port::{SubagentSnapshot, SubagentStatus};
use crate::layout::{compute_layout_with_chrome_and_sidebar, subagent_strip_height};
use crate::runner::{compute_ordered_thread_entries, compute_ordered_thread_indices};
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
    is_selected: bool,
    state: &AppState,
) -> Line<'static> {
    let is_active = id == state.active_thread_id;
    let is_merged = state.thread_meta.get(id).is_some_and(|m| m.merged);
    let glyph_opt =
        thread_status_glyph(state.thread_statuses.get(id).map(String::as_str), depth);

    if is_selected {
        let style = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED);
        let label = match glyph_opt {
            Some((glyph, _)) => format!("{prefix}{glyph} {title}"),
            None => format!("{prefix}{title}"),
        };
        let mut spans = vec![Span::styled(label, style)];
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
        if let Some((glyph, glyph_color)) = glyph_opt {
            spans.push(Span::styled(
                format!("{glyph} "),
                Style::default().fg(glyph_color),
            ));
        }
        spans.push(Span::styled(title.to_string(), title_style));
        if is_merged {
            spans.push(Span::styled(
                " [joined]",
                Style::default().fg(Color::DarkGray),
            ));
        }
        Line::from(spans)
    }
}

pub fn render_tree_view(f: &mut Frame, area: Rect, state: &AppState) {
    let _ = render_tree_view_inner(f, area, state);
}

fn render_tree_view_inner(f: &mut Frame, area: Rect, state: &AppState) -> u16 {
    let mut lines = Vec::new();
    let selected = state.tree.selected_index();
    let mut selected_line_idx: Option<usize> = None;

    if !state.tree.nodes.is_empty() {
        let fallback_layout: Option<HashMap<&str, (usize, String)>> =
            if state.tree.prefixes.is_empty() && !state.threads.is_empty() {
                Some(
                    compute_ordered_thread_entries(&state.threads, &state.thread_meta)
                        .into_iter()
                        .map(|e| (state.threads[e.index].id.as_str(), (e.depth, e.prefix)))
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
                let (depth, prefix) = if let Some(ref map) = fallback_layout {
                    map.get(t.id.as_str())
                        .map(|(d, p)| (*d, p.as_str()))
                        .unwrap_or((state.tree.depth_for(&t.id), state.tree.prefix_for(&t.id)))
                } else {
                    (state.tree.depth_for(&t.id), state.tree.prefix_for(&t.id))
                };
                lines.push(render_thread_row(
                    &t.id,
                    &t.title,
                    depth,
                    prefix,
                    is_selected,
                    state,
                ));
            }
        }
    } else if !state.threads.is_empty() {
        let ordered = compute_ordered_thread_entries(&state.threads, &state.thread_meta);
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

    let tree_focused = state.active_focus == PaneFocus::Tree
        || state.focused_pane == PaneFocus::Tree
        || state.focus == PaneFocus::Tree;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style(tree_focused))
        .title(" ᛇ Grein ");
    let paragraph = Paragraph::new(lines)
        .block(block)
        .scroll((scroll_offset, 0));
    f.render_widget(paragraph, area);
    scroll_offset
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
    transcript_inner_height: u16,
    composer_scroll_y: u16,
    help_scroll_offset: u16,
}

pub fn render_app_mut(f: &mut Frame, state: &mut AppState) {
    let metrics = render_app_inner(f, state);
    state
        .transcript_viewport_mut(&metrics.thread_id)
        .set_max_scroll(metrics.max_scroll as usize);
    state.tree.scroll_offset = metrics.tree_scroll;
    state.transcript_viewport_height = metrics.transcript_inner_height;
    state.composer_scroll_y = metrics.composer_scroll_y;
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

fn render_status_bar(f: &mut Frame, area: Rect, state: &AppState, subagent_count: usize) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let active_id = state.active_thread_id();
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

    let is_busy = matches!(status, "streaming" | "executing" | "thinking")
        || active_tool.is_some()
        || thought.is_some();

    let left_hint = if is_busy {
        "esc to cancel"
    } else {
        "? for help"
    };

    let (model_opt, mode_opt) = state
        .thread_agent_meta
        .get(active_id)
        .map(|(m, md)| (m.as_deref(), md.as_deref()))
        .unwrap_or((None, None));
    let mode = mode_opt.filter(|s| !s.trim().is_empty()).unwrap_or("agent");
    let model = model_opt
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("default");

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

    let sa_label = if subagent_count == 1 {
        "1 subagent".to_string()
    } else {
        format!("{subagent_count} subagents")
    };

    let right_prefix = format!("{mode} · {model} · ");
    let right_suffix = format!(" · {sa_label}");
    let total_right_chars =
        right_prefix.chars().count() + live_status.chars().count() + right_suffix.chars().count();

    let notice_suffix = state
        .status_notice
        .as_deref()
        .map(|n| format!(" · {n}"))
        .unwrap_or_default();
    let total_left_chars = left_hint.chars().count() + notice_suffix.chars().count();

    let width = area.width as usize;
    let gap = width.saturating_sub(total_left_chars + total_right_chars);

    let mut spans = vec![Span::styled(
        left_hint.to_string(),
        Style::default().fg(Color::DarkGray),
    )];
    if !notice_suffix.is_empty() {
        spans.push(Span::styled(
            notice_suffix,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if total_left_chars + 1 + total_right_chars <= width {
        spans.push(Span::raw(" ".repeat(gap.max(1))));
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
            let compact_right = truncate_single_line(
                &format!("{right_prefix}{live_status}{right_suffix}"),
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

    let sections: [(&str, &[(&str, &str)]); 3] = [
        (
            "── Global ──",
            &[
                ("Tab / Shift+Tab", "Cycle branches in active root"),
                ("Alt+Shift+↑/↓, Alt+J/K", "Switch between top-level roots"),
                (
                    "Alt+↑/↓, PgUp/PgDn",
                    "Scroll Transcript (Alt+Home/End top/btm)",
                ),
                ("Esc / Ctrl+G", "Toggle Composer / ᛇ Grein (Esc cancels)"),
                (
                    "Ctrl+N / Alt+N, Alt+R / F2",
                    "Fork branch / new root / rename context",
                ),
                (
                    "Ctrl+T / Ctrl+O / Alt+Y",
                    "Zoom / tool cards / copy (Ctrl+C/Ctrl+Q)",
                ),
            ],
        ),
        (
            "── ᛇ Grein Navigation ──",
            &[
                (
                    "j/k/↑/↓, Enter, J / K",
                    "Browse (preview) / activate / switch root",
                ),
                (
                    "Ctrl+U / Ctrl+D, g / G",
                    "Half-page scroll / Transcript top/bottom",
                ),
                (
                    "n / r / m (ᛇ Grein)",
                    "New root/rename/join (f fork, d/x close)",
                ),
                ("z / y / q (ᛇ Grein)", "Copy zoom / copy last reply / quit"),
            ],
        ),
        (
            "── Composer & Slash Commands ──",
            &[
                ("Shift+Enter / Ctrl+J", "Newline in Composer (Enter sends)"),
                (
                    "Ctrl+A / Ctrl+E",
                    "Line start/end (Home/End, Ctrl+F / Ctrl+B)",
                ),
                (
                    "Alt+←/→, Ctrl+←/→, Alt+B/F",
                    "Word move, Ctrl+K/U/W, Ctrl+Y/Ctrl+D/Alt+D",
                ),
                ("!cmd", "Run shell command in active context"),
                (
                    "/fork, /grein, /root, /new",
                    "Fork (/branch) / create blank root",
                ),
                (
                    "/rename, /join, /merge",
                    "Rename / join (/close, /delete, /quit)",
                ),
                (
                    "/model, /mode, /thinking",
                    "Pick model/mode/thinking (/effort)",
                ),
                (
                    "/commands, /copy, /zoom",
                    "Cmds, /copy [code|all], zoom, /?, /help, ?",
                ),
            ],
        ),
    ];

    let mut raw_body: Vec<Line<'static>> = Vec::new();
    for (title, rows) in sections {
        raw_body.push(Line::from(Span::styled(format!(" {title}"), section_style)));
        for &(k, desc) in rows {
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

fn render_app_inner(f: &mut Frame, state: &AppState) -> RenderMetrics {
    let active_id = state.active_thread_id();
    let active_subagents: &[SubagentSnapshot] = state
        .thread_subagents
        .get(active_id)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let strip_h = subagent_strip_height(active_subagents.len());
    let mut layout =
        compute_layout_with_chrome_and_sidebar(f.area(), strip_h, state.sidebar_hidden);

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

    let focused_debug = format!("{:?}", state.focused_pane());
    let tree_focused = state.active_focus == PaneFocus::Tree
        || state.focused_pane == PaneFocus::Tree
        || state.focus == PaneFocus::Tree;
    let transcript_focused = focused_debug.contains("Transcript");
    let composer_focused = focused_debug.contains("Composer")
        || focused_debug.contains("Input")
        || state.active_focus == PaneFocus::Composer
        || state.focused_pane == PaneFocus::Composer
        || state.focus == PaneFocus::Composer;

    let tree_scroll = if state.sidebar_hidden {
        state.tree.scroll_offset
    } else {
        render_tree_view_inner(f, layout.tree_area, state)
    };

    let preview = if tree_focused && !state.sidebar_hidden {
        selected_tree_thread(state).filter(|(sel_id, _)| *sel_id != active_id)
    } else {
        None
    };
    let display_thread_id = preview.map(|(id, _)| id).unwrap_or(active_id);
    let display_md: &str = if display_thread_id == active_id {
        &state.active_transcript_md
    } else {
        state
            .thread_transcripts
            .get(display_thread_id)
            .map(String::as_str)
            .unwrap_or("")
    };

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
    let transcript_inner_height = layout.transcript_area.height.saturating_sub(2);
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
            transcript_lines.extend(blocks.render_lines(transcript_inner_width as usize));
        }
    } else {
        transcript_lines.extend(crate::markdown::render_markdown_lines_with_width(
            display_md,
            transcript_inner_width as usize,
        ));
    }

    let wrapped_lines =
        crate::markdown::wrap_styled_lines(transcript_lines, transcript_inner_width as usize);
    let visual_lines = wrapped_lines.len();
    let max_scroll_u16 = visual_lines
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

    let composer_block = Block::default()
        .title(" Composer ")
        .borders(Borders::ALL)
        .border_style(border_style(composer_focused));

    let inner_height = layout.composer_area.height.saturating_sub(2).max(1);
    let max_composer_scroll = visual_line_count.saturating_sub(inner_height);
    let mut composer_scroll_y = state.composer_scroll_y.min(max_composer_scroll);
    if composer_layout.cursor_row < composer_scroll_y {
        composer_scroll_y = composer_layout.cursor_row;
    } else if composer_layout.cursor_row >= composer_scroll_y + inner_height {
        composer_scroll_y = composer_layout.cursor_row + 1 - inner_height;
    }

    let composer_widget = Paragraph::new(composer_layout.lines)
        .block(composer_block)
        .scroll((composer_scroll_y, 0));
    f.render_widget(composer_widget, layout.composer_area);

    if let Some(strip_area) = layout.subagent_strip_area {
        render_subagent_strip(f, strip_area, active_subagents);
    }

    render_status_bar(f, layout.status_bar_area, state, active_subagents.len());

    if composer_focused
        && state.pending_permission.is_none()
        && state.config_picker.is_none()
        && !state.show_help_modal
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
    } else if let Some(ref picker) = state.config_picker {
        render_config_picker_modal(f, f.area(), picker);
    } else if state.show_help_modal {
        rendered_help_scroll = render_help_modal(f, f.area(), state.help_scroll_offset);
    }

    RenderMetrics {
        thread_id: display_thread_id.to_string(),
        max_scroll: max_scroll_u16,
        tree_scroll,
        transcript_inner_height,
        composer_scroll_y,
        help_scroll_offset: rendered_help_scroll,
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
        assert!(screen.contains("esc to cancel"));
        assert!(!screen.contains("Enter: Send"));
    }
}
