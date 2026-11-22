use crate::app::{AppState, PaneFocus};
use crate::engine_port::{SubagentSnapshot, SubagentStatus};
use crate::layout::{compute_layout_with_chrome, subagent_strip_height};
use crate::runner::compute_ordered_thread_indices;
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

fn thread_status_glyph(status: Option<&str>) -> (&'static str, Color) {
    match status.unwrap_or("idle") {
        "streaming" | "executing" | "thinking" => ("●", Color::Yellow),
        "permission" | "permission_required" | "error" => ("⚠", Color::Red),
        _ => ("✓", Color::Green),
    }
}

fn wrapped_line_count(lines: &[Line<'_>], inner_width: u16) -> usize {
    let w = inner_width as usize;
    if w == 0 {
        return lines.len();
    }
    let mut total = 0usize;
    for line in lines {
        let lw = line.width();
        if lw == 0 {
            total += 1;
        } else {
            total += lw.div_ceil(w);
        }
    }
    total
}

fn render_thread_row(
    id: &str,
    title: &str,
    depth: usize,
    is_selected: bool,
    state: &AppState,
) -> Line<'static> {
    let is_active = id == state.active_thread_id;
    let (glyph, glyph_color) =
        thread_status_glyph(state.thread_statuses.get(id).map(String::as_str));
    let indent = "  ".repeat(depth);

    if is_selected {
        let style = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED);
        Line::from(vec![Span::styled(
            format!("{indent}{glyph} {title}"),
            style,
        )])
    } else {
        let title_style = if is_active {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        };
        Line::from(vec![
            Span::raw(indent),
            Span::styled(format!("{glyph} "), Style::default().fg(glyph_color)),
            Span::styled(title.to_string(), title_style),
        ])
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
        for (node_idx, node) in state.tree.nodes.iter().enumerate() {
            if let TreeNode::Thread(t) = node {
                let is_selected = selected == Some(node_idx);
                if is_selected {
                    selected_line_idx = Some(lines.len());
                }
                let depth = state.tree.depth_for(&t.id);
                lines.push(render_thread_row(&t.id, &t.title, depth, is_selected, state));
            }
        }
    } else if !state.threads.is_empty() {
        let ordered = compute_ordered_thread_indices(&state.threads, &state.thread_meta);
        for (row_idx, (idx, depth)) in ordered.into_iter().enumerate() {
            let thread = &state.threads[idx];
            let is_selected = selected == Some(row_idx);
            if is_selected {
                selected_line_idx = Some(lines.len());
            }
            lines.push(render_thread_row(
                &thread.id,
                &thread.title,
                depth,
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
        .title(" Tree ");
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

pub fn render_app_mut(f: &mut Frame, state: &mut AppState) {
    let (thread_id, max_scroll, tree_scroll) = render_app_inner(f, state);
    state
        .transcript_viewport_mut(&thread_id)
        .set_max_scroll(max_scroll as usize);
    state.tree.scroll_offset = tree_scroll;
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

fn render_help_modal(f: &mut Frame, area: Rect) {
    let popup_width = area.width.saturating_sub(6).clamp(30, 74).min(area.width);
    let popup_height = 20u16.min(area.height.max(1));
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

    let key_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let rows = [
        ("Tab/Shift+Tab", "Cycle focus across panes"),
        ("Esc", "Cancel streaming turn, or focus Tree"),
        ("Enter", "Send prompt / activate Tree item"),
        ("Shift+Enter", "Insert newline in Composer"),
        ("!cmd", "Run shell command in active context"),
        ("/fork [title]", "Fork new child context (/branch)"),
        ("/merge", "Merge active branch into parent"),
        ("/delete", "Delete active branch (>1 thread)"),
        ("/wb set|del", "Upsert or delete Blackboard entry"),
        ("/?, /help, ?", "Open this keybindings help modal"),
        ("d (Tree)", "Delete selected branch (>1 thread)"),
        ("f / m (Tree)", "Fork / Merge selected context"),
        ("j / k / ↑ / ↓", "Navigate Tree / Blackboard / Transcript"),
        ("Ctrl+B / [ / ]", "Toggle / collapse / expand Blackboard"),
        ("Ctrl+N", "Quick-fork new child context"),
        ("Ctrl+C / Ctrl+Q", "Quit application"),
    ];
    let mut lines: Vec<Line<'static>> = rows
        .into_iter()
        .map(|(k, desc)| {
            Line::from(vec![
                Span::styled(format!(" {k:<16}"), key_style),
                Span::raw(desc),
            ])
        })
        .collect();
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        " Press Esc, q, ?, or Enter to close",
        Style::default().fg(Color::DarkGray),
    )));

    let widget = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(widget, popup_area);
}

fn render_app_inner(f: &mut Frame, state: &AppState) -> (String, u16, u16) {
    let active_id = state.active_thread_id();
    let active_subagents: &[SubagentSnapshot] = state
        .thread_subagents
        .get(active_id)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let strip_h = subagent_strip_height(active_subagents.len());
    let mut layout =
        compute_layout_with_chrome(f.area(), state.is_blackboard_collapsed(), strip_h);

    let line_count = state.input_buffer.split('\n').count().max(1) as u16;
    let max_composer_h = layout
        .tree_area
        .height
        .saturating_add(layout.composer_area.height)
        .saturating_sub(3)
        .max(3);
    let desired_height = (line_count + 2).clamp(3, 10).min(max_composer_h);
    if desired_height > layout.composer_area.height {
        let delta = desired_height - layout.composer_area.height;
        layout.tree_area.height = layout.tree_area.height.saturating_sub(delta);
        layout.transcript_area.height = layout.transcript_area.height.saturating_sub(delta);
        if let Some(ref mut bb_area) = layout.blackboard_area {
            bb_area.height = bb_area.height.saturating_sub(delta);
        }
        layout.composer_area.y = layout.composer_area.y.saturating_sub(delta);
        layout.composer_area.height = desired_height;
    }

    let focused_debug = format!("{:?}", state.focused_pane());
    let transcript_focused = focused_debug.contains("Transcript");
    let blackboard_focused = focused_debug.contains("Blackboard");
    let composer_focused = focused_debug.contains("Composer")
        || focused_debug.contains("Input")
        || state.active_focus == PaneFocus::Composer
        || state.focused_pane == PaneFocus::Composer
        || state.focus == PaneFocus::Composer;

    let tree_scroll = render_tree_view_inner(f, layout.tree_area, state);

    let mut transcript_lines = Vec::new();
    if let Some(meta) = state.thread_meta.get(active_id) {
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
            if !state.active_transcript_md.trim().is_empty() {
                transcript_lines.push(Line::raw(""));
            }
        }
    }
    let transcript_inner_width = layout.transcript_area.width.saturating_sub(2);
    transcript_lines.extend(crate::markdown::render_markdown_lines_with_width(
        &state.active_transcript_md,
        transcript_inner_width as usize,
    ));
    let transcript_inner_height = layout.transcript_area.height.saturating_sub(2);
    let visual_lines = wrapped_line_count(&transcript_lines, transcript_inner_width);
    let max_scroll_u16 = visual_lines
        .saturating_sub(transcript_inner_height as usize)
        .min(u16::MAX as usize) as u16;
    let vp = state.transcript_viewport(active_id);
    let transcript_scroll_y = if vp.auto_scroll() {
        max_scroll_u16
    } else {
        (vp.scroll_offset() as u16).min(max_scroll_u16)
    };
    let transcript_block = Block::default()
        .title(" Transcript ")
        .borders(Borders::ALL)
        .border_style(border_style(transcript_focused));
    let transcript_widget = Paragraph::new(transcript_lines)
        .block(transcript_block)
        .wrap(Wrap { trim: false })
        .scroll((transcript_scroll_y, 0));
    f.render_widget(transcript_widget, layout.transcript_area);

    if let Some(bb_area) = layout.blackboard_area {
        let bb_block = Block::default()
            .title(" Blackboard ")
            .borders(Borders::ALL)
            .border_style(border_style(blackboard_focused));
        let bb_lines: Vec<Line<'static>> = if !state.blackboard_entries.is_empty() {
            state
                .blackboard_entries
                .iter()
                .enumerate()
                .map(|(idx, entry)| {
                    let text = format!("• {}: {}", entry.key, entry.value);
                    if blackboard_focused && idx == state.blackboard_selected {
                        Line::from(Span::styled(
                            text,
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
                        ))
                    } else {
                        Line::from(vec![
                            Span::styled(
                                format!("• {}: ", entry.key),
                                Style::default().add_modifier(Modifier::BOLD),
                            ),
                            Span::raw(entry.value.clone()),
                        ])
                    }
                })
                .collect()
        } else if !state.blackboard_notes.trim().is_empty() {
            state
                .blackboard_notes
                .lines()
                .map(|l| Line::raw(l.to_string()))
                .collect()
        } else {
            vec![Line::from(Span::styled(
                "No shared facts yet. Press 'a' or type /wb set <k> <v>",
                Style::default().fg(Color::DarkGray),
            ))]
        };
        let bb_widget = Paragraph::new(bb_lines)
            .block(bb_block)
            .wrap(Wrap { trim: false });
        f.render_widget(bb_widget, bb_area);
    }

    let composer_block = Block::default()
        .title(" Composer ")
        .borders(Borders::ALL)
        .border_style(border_style(composer_focused));

    let inner_height = layout.composer_area.height.saturating_sub(2);
    let scroll_y = line_count.saturating_sub(inner_height);
    let composer_widget = Paragraph::new(state.input_buffer.as_str())
        .block(composer_block)
        .scroll((scroll_y, 0));
    f.render_widget(composer_widget, layout.composer_area);

    if let Some(strip_area) = layout.subagent_strip_area {
        render_subagent_strip(f, strip_area, active_subagents);
    }

    render_status_bar(f, layout.status_bar_area, state, active_subagents.len());

    if composer_focused && state.pending_permission.is_none() && !state.show_help_modal {
        let (col, row) = state.composer_cursor_2d();
        let inner_width = layout.composer_area.width.saturating_sub(2);
        let visible_row = row.saturating_sub(scroll_y);
        let cursor_x = layout.composer_area.x + 1 + col.min(inner_width.saturating_sub(1));
        let cursor_y = layout.composer_area.y + 1 + visible_row.min(inner_height.saturating_sub(1));
        f.set_cursor_position(Position::new(cursor_x, cursor_y));
    }

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
    } else if state.show_help_modal {
        render_help_modal(f, f.area());
    }

    (active_id.to_string(), max_scroll_u16, tree_scroll)
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
    fn test_render_writes_max_scroll_renders_popup_whiteboard_and_tree_status() {
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
        app.upsert_blackboard_entry("mode", "fast");
        app.sync_anchors_and_blackboard();

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
        assert!(screen.contains("mode: fast"));
        assert!(screen.contains("●"));
        assert!(screen.contains("⚠"));
        assert!(screen.contains("esc to cancel"));
        assert!(!screen.contains("Enter: Send"));
    }
}
