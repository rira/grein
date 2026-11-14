use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneFocus {
    Sidebar,
    Transcript,
    Composer,
    Inspector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptMessage {
    pub id: String,
    pub role: MessageRole,
    pub content: String,
    pub timestamp: String,
}

impl TranscriptMessage {
    pub fn new(
        id: impl Into<String>,
        role: MessageRole,
        content: impl Into<String>,
        timestamp: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            role,
            content: content.into(),
            timestamp: timestamp.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptViewport {
    pub scroll_offset: usize,
    pub max_scroll: usize,
    pub auto_scroll: bool,
}

impl Default for TranscriptViewport {
    fn default() -> Self {
        Self {
            scroll_offset: 0,
            max_scroll: 0,
            auto_scroll: true,
        }
    }
}

impl TranscriptViewport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn scroll_up(&mut self, lines: usize) {
        self.auto_scroll = false;
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll_offset = (self.scroll_offset + lines).min(self.max_scroll);
        if self.scroll_offset >= self.max_scroll {
            self.auto_scroll = true;
        }
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = self.max_scroll;
        self.auto_scroll = true;
    }

    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = 0;
        self.auto_scroll = false;
    }

    pub fn update_max_scroll(&mut self, total_lines: usize, viewport_height: usize) {
        self.max_scroll = total_lines.saturating_sub(viewport_height);
        if self.auto_scroll {
            self.scroll_offset = self.max_scroll;
        } else {
            self.scroll_offset = self.scroll_offset.min(self.max_scroll);
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptPane {
    pub messages: Vec<TranscriptMessage>,
    pub viewport: TranscriptViewport,
    pub focus: PaneFocus,
}

impl TranscriptPane {
    pub fn new(focus: PaneFocus) -> Self {
        Self {
            messages: Vec::new(),
            viewport: TranscriptViewport::new(),
            focus,
        }
    }

    pub fn is_focused(&self) -> bool {
        self.focus == PaneFocus::Transcript
    }

    pub fn set_focus(&mut self, focus: PaneFocus) {
        self.focus = focus;
    }

    pub fn add_message(&mut self, message: TranscriptMessage) {
        self.messages.push(message);
    }

    pub fn border_style(&self) -> Style {
        if self.is_focused() {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        }
    }

    pub fn title_style(&self) -> Style {
        if self.is_focused() {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        }
    }
}

impl Widget for &TranscriptPane {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let border_style = self.border_style();
        let title_style = self.title_style();

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(Span::styled(" Transcript (Active Thread) ", title_style));

        let inner_area = block.inner(area);
        block.render(area, buf);

        if inner_area.height == 0 || inner_area.width == 0 {
            return;
        }

        let mut text_lines: Vec<Line> = Vec::new();
        for msg in &self.messages {
            let role_style = match msg.role {
                MessageRole::User => Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                MessageRole::Assistant => Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD),
                MessageRole::System => Style::default().fg(Color::Magenta).add_modifier(Modifier::ITALIC),
            };

            let role_label = match msg.role {
                MessageRole::User => "[User]",
                MessageRole::Assistant => "[Assistant]",
                MessageRole::System => "[System]",
            };

            text_lines.push(Line::from(vec![
                Span::styled(role_label, role_style),
                Span::raw(" "),
                Span::styled(&msg.timestamp, Style::default().fg(Color::DarkGray)),
            ]));
            text_lines.push(Line::from(Span::raw(&msg.content)));
            text_lines.push(Line::from(""));
        }

        let paragraph = Paragraph::new(text_lines)
            .scroll((self.viewport.scroll_offset as u16, 0));
        paragraph.render(inner_area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn test_transcript_active_focus_visual_styling() {
        let active_pane = TranscriptPane::new(PaneFocus::Transcript);
        assert!(active_pane.is_focused());

        let active_border_style = active_pane.border_style();
        assert_eq!(active_border_style.fg, Some(Color::Cyan));
        assert!(active_border_style.sub_modifier(Modifier::BOLD).is_empty() || active_border_style.add_modifier.contains(Modifier::BOLD));

        let active_title_style = active_pane.title_style();
        assert_eq!(active_title_style.fg, Some(Color::Yellow));
        assert!(active_title_style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn test_transcript_inactive_focus_visual_styling() {
        let inactive_pane = TranscriptPane::new(PaneFocus::Composer);
        assert!(!inactive_pane.is_focused());

        let inactive_border_style = inactive_pane.border_style();
        assert_eq!(inactive_border_style.fg, Some(Color::DarkGray));
        assert!(!inactive_border_style.add_modifier.contains(Modifier::BOLD));

        let inactive_title_style = inactive_pane.title_style();
        assert_eq!(inactive_title_style.fg, Some(Color::Gray));
        assert!(!inactive_title_style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn test_viewport_scroll_navigation_boundaries() {
        let mut viewport = TranscriptViewport::new();
        viewport.update_max_scroll(50, 10);
        assert_eq!(viewport.max_scroll, 40);
        assert_eq!(viewport.scroll_offset, 40);
        assert!(viewport.auto_scroll);

        viewport.scroll_up(15);
        assert_eq!(viewport.scroll_offset, 25);
        assert!(!viewport.auto_scroll);

        viewport.scroll_up(30);
        assert_eq!(viewport.scroll_offset, 0);
        assert!(!viewport.auto_scroll);

        viewport.scroll_down(20);
        assert_eq!(viewport.scroll_offset, 20);
        assert!(!viewport.auto_scroll);

        viewport.scroll_down(30);
        assert_eq!(viewport.scroll_offset, 40);
        assert!(viewport.auto_scroll);

        viewport.scroll_to_top();
        assert_eq!(viewport.scroll_offset, 0);
        assert!(!viewport.auto_scroll);

        viewport.scroll_to_bottom();
        assert_eq!(viewport.scroll_offset, 40);
        assert!(viewport.auto_scroll);
    }

    #[test]
    fn test_active_thread_transcript_rendered_buffer_with_active_focus() {
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();

        let mut pane = TranscriptPane::new(PaneFocus::Transcript);
        pane.add_message(TranscriptMessage::new(
            "msg-1",
            MessageRole::User,
            "Hello Eldi",
            "10:00:00",
        ));
        pane.add_message(TranscriptMessage::new(
            "msg-2",
            MessageRole::Assistant,
            "Autonomous engine ready.",
            "10:00:01",
        ));

        terminal.draw(|f| {
            f.render_widget(&pane, f.size());
        }).unwrap();

        let buffer = terminal.backend().buffer();

        let top_left_cell = buffer.get(0, 0);
        assert_eq!(top_left_cell.fg, Color::Cyan);

        let mut content_found = false;
        for y in 0..10 {
            let mut row_text = String::new();
            for x in 0..40 {
                row_text.push(buffer.get(x, y).symbol().chars().next().unwrap_or(' '));
            }
            if row_text.contains("Hello Eldi") {
                content_found = true;
                break;
            }
        }
        assert!(content_found, "Buffer should render the active thread transcript message content");
    }

    #[test]
    fn test_focus_switching_updates_visual_presentation() {
        let mut pane = TranscriptPane::new(PaneFocus::Sidebar);
        assert!(!pane.is_focused());
        assert_eq!(pane.border_style().fg, Some(Color::DarkGray));

        pane.set_focus(PaneFocus::Transcript);
        assert!(pane.is_focused());
        assert_eq!(pane.border_style().fg, Some(Color::Cyan));
    }
}