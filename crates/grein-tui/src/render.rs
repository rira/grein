use crate::app::{AppState, PaneFocus};
use crate::layout::compute_layout;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

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
    for line in markdown.lines() {
        let trimmed = line.trim();
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

pub fn render_tree_view(f: &mut Frame, area: Rect, state: &AppState) {
    let mut lines = Vec::new();
    let selected = state.tree.selected_index();
    let mut row_idx = 0usize;
    for thread in &state.threads {
        let style = if selected == Some(row_idx) {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        };
        lines.push(Line::from(vec![Span::styled(
            format!("📁 {}", thread.title),
            style,
        )]));
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
    let border_style = if state.active_focus == PaneFocus::Tree {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title("Tree Navigator");
    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

pub fn render_app(f: &mut Frame, state: &AppState) {
    let layout = compute_layout(f.area(), state.blackboard_collapsed);

    render_tree_view(f, layout.tree_area, state);

    let transcript_border_style = if state.active_focus == PaneFocus::Transcript {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let transcript_lines = render_markdown_lines(&state.active_transcript_md);
    let transcript_block = Block::default()
        .borders(Borders::ALL)
        .border_style(transcript_border_style)
        .title("Transcript");
    let transcript_widget = Paragraph::new(transcript_lines).block(transcript_block);
    f.render_widget(transcript_widget, layout.transcript_area);

    if let Some(bb_area) = layout.blackboard_area {
        let bb_border_style = if state.active_focus == PaneFocus::Blackboard {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let bb_block = Block::default()
            .borders(Borders::ALL)
            .border_style(bb_border_style)
            .title("Blackboard");
        let bb_widget = Paragraph::new(state.blackboard_notes.as_str()).block(bb_block);
        f.render_widget(bb_widget, bb_area);
    }

    let composer_border_style = if state.active_focus == PaneFocus::Composer {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let composer_block = Block::default()
        .borders(Borders::ALL)
        .border_style(composer_border_style)
        .title("Composer");
    let composer_widget = Paragraph::new(state.input_buffer.as_str()).block(composer_block);
    f.render_widget(composer_widget, layout.composer_area);
}
