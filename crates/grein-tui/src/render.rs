use crate::app::AppState;
use crate::layout::compute_layout;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph};
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

/// Renders the application state into the terminal frame.
pub fn render(f: &mut Frame, state: &AppState) {
    render_app(f, state);
}

/// Renders all active workspace panes, auto-growing the Composer pane for multi-line input.
pub fn render_app(f: &mut Frame, state: &AppState) {
    let mut layout = compute_layout(f.area(), state.blackboard_collapsed);
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
    let tree_focused = focused_debug.contains("Tree") || focused_debug.contains("Nav");
    let transcript_focused = focused_debug.contains("Transcript");
    let blackboard_focused = focused_debug.contains("Blackboard");
    let composer_focused = focused_debug.contains("Composer") || focused_debug.contains("Input");

    let tree_block = Block::default()
        .title(" Tree ")
        .borders(Borders::ALL)
        .border_style(border_style(tree_focused));
    let tree_widget = Paragraph::new("").block(tree_block);
    f.render_widget(tree_widget, layout.tree_area);

    let transcript_block = Block::default()
        .title(" Transcript ")
        .borders(Borders::ALL)
        .border_style(border_style(transcript_focused));
    let transcript_widget = Paragraph::new("").block(transcript_block);
    f.render_widget(transcript_widget, layout.transcript_area);

    if let Some(bb_area) = layout.blackboard_area {
        let bb_block = Block::default()
            .title(" Blackboard ")
            .borders(Borders::ALL)
            .border_style(border_style(blackboard_focused));
        let bb_widget = Paragraph::new("").block(bb_block);
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
}
