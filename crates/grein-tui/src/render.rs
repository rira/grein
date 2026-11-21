use crate::app::{AppState, PaneFocus};
use crate::layout::compute_layout;
use crate::ui::tree::TreeNode;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

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

fn parse_inline_spans(mut text: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    while !text.is_empty() {
        let bold_pos = text.find("**");
        let code_pos = text.find('`');

        match (bold_pos, code_pos) {
            (Some(b), Some(c)) if b < c => {
                if b > 0 {
                    spans.push(Span::raw(text[..b].to_string()));
                }
                let rest = &text[b + 2..];
                if let Some(end) = rest.find("**") {
                    spans.push(Span::styled(
                        rest[..end].to_string(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ));
                    text = &rest[end + 2..];
                } else {
                    spans.push(Span::raw(text[b..].to_string()));
                    break;
                }
            }
            (Some(b), Some(c)) if c < b => {
                if c > 0 {
                    spans.push(Span::raw(text[..c].to_string()));
                }
                let rest = &text[c + 1..];
                if let Some(end) = rest.find('`') {
                    spans.push(Span::styled(
                        rest[..end].to_string(),
                        Style::default().fg(Color::Yellow),
                    ));
                    text = &rest[end + 1..];
                } else {
                    spans.push(Span::raw(text[c..].to_string()));
                    break;
                }
            }
            (Some(b), None) => {
                if b > 0 {
                    spans.push(Span::raw(text[..b].to_string()));
                }
                let rest = &text[b + 2..];
                if let Some(end) = rest.find("**") {
                    spans.push(Span::styled(
                        rest[..end].to_string(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ));
                    text = &rest[end + 2..];
                } else {
                    spans.push(Span::raw(text[b..].to_string()));
                    break;
                }
            }
            (None, Some(c)) => {
                if c > 0 {
                    spans.push(Span::raw(text[..c].to_string()));
                }
                let rest = &text[c + 1..];
                if let Some(end) = rest.find('`') {
                    spans.push(Span::styled(
                        rest[..end].to_string(),
                        Style::default().fg(Color::Yellow),
                    ));
                    text = &rest[end + 1..];
                } else {
                    spans.push(Span::raw(text[c..].to_string()));
                    break;
                }
            }
            (None, None) => {
                spans.push(Span::raw(text.to_string()));
                break;
            }
            (Some(_), Some(_)) => unreachable!(),
        }
    }
    spans
}

pub fn render_markdown_lines(markdown: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut in_code_block = false;
    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            lines.push(Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(Color::DarkGray),
            )));
            continue;
        }
        if in_code_block {
            lines.push(Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(Color::Yellow),
            )));
            continue;
        }
        if let Some(h1) = trimmed.strip_prefix("# ") {
            lines.push(Line::from(vec![Span::styled(
                h1.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            )]));
        } else if let Some(h2) = trimmed.strip_prefix("## ") {
            lines.push(Line::from(vec![Span::styled(
                h2.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            )]));
        } else if let Some(h3) = trimmed.strip_prefix("### ") {
            lines.push(Line::from(vec![Span::styled(
                h3.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            )]));
        } else if let Some(quote) = trimmed.strip_prefix("> ") {
            lines.push(Line::from(vec![Span::styled(
                quote.to_string(),
                Style::default().fg(Color::DarkGray),
            )]));
        } else if let Some(bullet) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            let mut spans = vec![Span::raw("• ")];
            spans.extend(parse_inline_spans(bullet));
            lines.push(Line::from(spans));
        } else if trimmed.is_empty() {
            lines.push(Line::raw(""));
        } else {
            lines.push(Line::from(parse_inline_spans(line)));
        }
    }
    if lines.is_empty() && !markdown.is_empty() {
        lines.push(Line::raw(markdown.to_string()));
    }
    lines
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

pub fn render_tree_view(f: &mut Frame, area: Rect, state: &AppState) {
    let mut lines = Vec::new();
    let selected = state.tree.selected_index();
    let mut row_idx = 0usize;

    if !state.threads.is_empty() {
        for thread in &state.threads {
            let is_active = thread.id == state.active_thread_id;
            let (glyph, glyph_color) =
                thread_status_glyph(state.thread_statuses.get(&thread.id).map(String::as_str));
            let active_marker = if is_active { "▶ " } else { "  " };
            if selected == Some(row_idx) {
                let style = Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED);
                lines.push(Line::from(vec![Span::styled(
                    format!("{active_marker}{glyph} {}", thread.title),
                    style,
                )]));
            } else {
                let title_style = if is_active {
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().add_modifier(Modifier::BOLD)
                };
                lines.push(Line::from(vec![
                    Span::styled(active_marker, title_style),
                    Span::styled(format!("{glyph} "), Style::default().fg(glyph_color)),
                    Span::styled(thread.title.clone(), title_style),
                ]));
            }
            row_idx += 1;
            for anchor in &thread.anchors {
                let anchor_style = if selected == Some(row_idx) {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else {
                    Style::default()
                };
                lines.push(Line::from(vec![Span::styled(
                    format!("  ⚓ {}", anchor.label),
                    anchor_style,
                )]));
                row_idx += 1;
            }
        }
    } else {
        for node in &state.tree.nodes {
            let style = if selected == Some(row_idx) {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default()
            };
            match node {
                TreeNode::Thread(t) => {
                    lines.push(Line::from(vec![Span::styled(
                        format!("📁 {}", t.title),
                        style.add_modifier(Modifier::BOLD),
                    )]));
                }
                TreeNode::Anchor(a) => {
                    lines.push(Line::from(vec![Span::styled(
                        format!("  ⚓ {}", a.label),
                        style,
                    )]));
                }
            }
            row_idx += 1;
        }
    }

    let tree_focused = state.active_focus == PaneFocus::Tree
        || state.focused_pane == PaneFocus::Tree
        || state.focus == PaneFocus::Tree;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style(tree_focused))
        .title(" Tree ");
    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

/// Renders the application state into the terminal frame.
pub fn render(f: &mut Frame, state: &AppState) {
    render_app(f, state);
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
    let (thread_id, max_scroll) = render_app_inner(f, state);
    state
        .transcript_viewport_mut(&thread_id)
        .set_max_scroll(max_scroll as usize);
}

fn render_app_inner(f: &mut Frame, state: &AppState) -> (String, u16) {
    let mut layout = compute_layout(f.area(), state.is_blackboard_collapsed());
    let line_count = state.input_buffer.split('\n').count().max(1) as u16;
    let desired_height = (line_count + 2)
        .clamp(3, 10)
        .min(f.area().height.saturating_sub(3).max(3));
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

    render_tree_view(f, layout.tree_area, state);

    let transcript_lines = render_markdown_lines(&state.active_transcript_md);
    let transcript_inner_width = layout.transcript_area.width.saturating_sub(2);
    let transcript_inner_height = layout.transcript_area.height.saturating_sub(2);
    let visual_lines = wrapped_line_count(&transcript_lines, transcript_inner_width);
    let max_scroll_u16 = visual_lines
        .saturating_sub(transcript_inner_height as usize)
        .min(u16::MAX as usize) as u16;
    let vp = state.transcript_viewport(state.active_thread_id());
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
        .title_bottom(
            " Enter: Send | !cmd: Shell | Ctrl+N: Branch | /wb: Whiteboard | Tab: Focus | Esc: Cancel | Ctrl+Q: Quit ",
        )
        .borders(Borders::ALL)
        .border_style(border_style(composer_focused));

    let inner_height = layout.composer_area.height.saturating_sub(2);
    let scroll_y = line_count.saturating_sub(inner_height);
    let composer_widget = Paragraph::new(state.input_buffer.as_str())
        .block(composer_block)
        .scroll((scroll_y, 0));
    f.render_widget(composer_widget, layout.composer_area);

    if composer_focused && state.pending_permission.is_none() {
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
    }

    (state.active_thread_id().to_string(), max_scroll_u16)
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
        assert!(screen.contains("Enter: Send"));
    }
}
