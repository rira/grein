use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::ui::focus::PaneFocus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionItem {
    pub id: String,
    pub description: String,
}

impl DecisionItem {
    pub fn new(id: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlackboardState {
    collapsed: bool,
    inspector_title: String,
    inspector_detail: String,
    decisions: Vec<DecisionItem>,
}

impl BlackboardState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_collapsed(&self) -> bool {
        self.collapsed
    }

    pub fn set_collapsed(&mut self, collapsed: bool) {
        self.collapsed = collapsed;
    }

    pub fn toggle_collapsed(&mut self) {
        self.collapsed = !self.collapsed;
    }

    pub fn inspector_title(&self) -> &str {
        &self.inspector_title
    }

    pub fn set_inspector_title(&mut self, title: impl Into<String>) {
        self.inspector_title = title.into();
    }

    pub fn inspector_detail(&self) -> &str {
        &self.inspector_detail
    }

    pub fn set_inspector_detail(&mut self, detail: impl Into<String>) {
        self.inspector_detail = detail.into();
    }

    pub fn decisions(&self) -> &[DecisionItem] {
        &self.decisions
    }

    pub fn add_decision(&mut self, decision: DecisionItem) {
        self.decisions.push(decision);
    }

    pub fn clear_decisions(&mut self) {
        self.decisions.clear();
    }
}

#[derive(Debug, Clone)]
pub struct BlackboardView<'a> {
    state: &'a BlackboardState,
    focus: PaneFocus,
}

impl<'a> BlackboardView<'a> {
    pub fn new(state: &'a BlackboardState) -> Self {
        Self {
            state,
            focus: PaneFocus::default(),
        }
    }

    pub fn with_focus(mut self, focus: PaneFocus) -> Self {
        self.focus = focus;
        self
    }
}

impl<'a> Widget for BlackboardView<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        (&self).render(area, buf);
    }
}

impl<'a> Widget for &BlackboardView<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let border_style = if self.focus == PaneFocus::Blackboard {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        if self.state.is_collapsed() {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title("Blackboard [Collapsed]");
            let inner = block.inner(area);
            block.render(area, buf);
            if inner.height > 0 && inner.width >= 22 {
                let p = Paragraph::new("Blackboard [Collapsed]");
                p.render(inner, buf);
            }
            return;
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title("Blackboard");
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(4), Constraint::Min(1)])
            .split(inner);

        let mut inspector_lines = Vec::new();
        if !self.state.inspector_title.is_empty() {
            inspector_lines.push(Line::from(Span::styled(
                self.state.inspector_title.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            )));
        }
        if !self.state.inspector_detail.is_empty() {
            inspector_lines.push(Line::from(self.state.inspector_detail.clone()));
        }

        let inspector = Paragraph::new(inspector_lines)
            .block(Block::default().borders(Borders::BOTTOM).title("Inspector"));
        inspector.render(chunks[0], buf);

        let mut decision_lines = Vec::new();
        for decision in &self.state.decisions {
            decision_lines.push(Line::from(format!(
                "{}: {}",
                decision.id, decision.description
            )));
        }

        let decisions_widget =
            Paragraph::new(decision_lines).block(Block::default().title("Decision Registry"));
        decisions_widget.render(chunks[1], buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn test_blackboard_view_renders_inspector_and_decision_registry() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("failed to initialize test terminal");

        let mut state = BlackboardState::default();
        state.set_inspector_title("Global Inspector");
        state.set_inspector_detail("Active agent: autonomous-coder");
        state.add_decision(DecisionItem::new(
            "DEC-001",
            "Adopt Universal Two-Tier Architecture",
        ));
        state.add_decision(DecisionItem::new("DEC-002", "Enforce Zero-Null Policy"));

        let view = BlackboardView::new(&state).with_focus(PaneFocus::Blackboard);

        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, 80, 24);
                frame.render_widget(view, area);
            })
            .expect("failed to render blackboard view");

        let buffer = terminal.backend().buffer();
        let content = buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(
            content.contains("Global Inspector"),
            "Expected buffer to render inspector title"
        );
        assert!(
            content.contains("Active agent: autonomous-coder"),
            "Expected buffer to render inspector details"
        );
        assert!(
            content.contains("DEC-001"),
            "Expected buffer to render decision ID"
        );
        assert!(
            content.contains("Adopt Universal Two-Tier Architecture"),
            "Expected buffer to render decision description"
        );
        assert!(
            content.contains("DEC-002"),
            "Expected buffer to render second decision ID"
        );
    }

    #[test]
    fn test_blackboard_view_collapsed_state_renders_indicator() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("failed to initialize test terminal");

        let mut state = BlackboardState::default();
        state.set_collapsed(true);
        state.set_inspector_title("Global Inspector");

        let view = BlackboardView::new(&state).with_focus(PaneFocus::Blackboard);

        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, 80, 24);
                frame.render_widget(view, area);
            })
            .expect("failed to render collapsed blackboard view");

        let buffer = terminal.backend().buffer();
        let content = buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(
            content.contains("Blackboard [Collapsed]")
                || content.contains("Blackboard (Collapsed)"),
            "Expected collapsed indicator when blackboard is collapsed"
        );
    }
}
