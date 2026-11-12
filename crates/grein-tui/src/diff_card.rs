use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Widget;
use std::sync::Arc;

use crate::pager::{DefaultPagerRunner, PagerRunner};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffCardAction {
    OpenedPager,
    Ignored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Addition,
    Deletion,
    Header,
    Context,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffPreviewLine {
    content: String,
    kind: DiffLineKind,
}

impl DiffPreviewLine {
    pub fn new(content: impl Into<String>, kind: DiffLineKind) -> Self {
        Self {
            content: content.into(),
            kind,
        }
    }

    pub fn is_addition(&self) -> bool {
        self.kind == DiffLineKind::Addition
    }

    pub fn is_deletion(&self) -> bool {
        self.kind == DiffLineKind::Deletion
    }

    pub fn is_header(&self) -> bool {
        self.kind == DiffLineKind::Header
    }

    pub fn is_context(&self) -> bool {
        self.kind == DiffLineKind::Context
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn kind(&self) -> DiffLineKind {
        self.kind
    }
}

pub struct DiffCard {
    diff_content: String,
    reply_anchor: Option<String>,
    max_preview_lines: usize,
    pager_runner: Arc<dyn PagerRunner>,
}

impl DiffCard {
    pub fn builder() -> DiffCardBuilder {
        DiffCardBuilder::new()
    }

    pub fn diff_content(&self) -> &str {
        &self.diff_content
    }

    pub fn reply_anchor(&self) -> Option<&str> {
        self.reply_anchor.as_deref()
    }

    pub fn max_preview_lines(&self) -> usize {
        self.max_preview_lines
    }

    pub fn preview_lines(&self) -> Vec<DiffPreviewLine> {
        self.diff_content
            .lines()
            .take(self.max_preview_lines)
            .map(|line| {
                let kind = if line.starts_with('+') {
                    DiffLineKind::Addition
                } else if line.starts_with('-') {
                    DiffLineKind::Deletion
                } else if line.starts_with('@') {
                    DiffLineKind::Header
                } else {
                    DiffLineKind::Context
                };
                DiffPreviewLine::new(line, kind)
            })
            .collect()
    }

    pub fn render_badge(&self, buffer: &mut Buffer, area: Rect) {
        if let Some(ref anchor) = self.reply_anchor {
            let badge_text = format!("[Replying to: {}]", anchor);
            buffer.set_string(area.x, area.y, badge_text, Style::default());
        }
    }

    pub fn handle_key<F>(&self, key_event: KeyEvent, suspend_terminal: F) -> DiffCardAction
    where
        F: FnOnce(),
    {
        if key_event.code == KeyCode::Char('o') && key_event.modifiers == KeyModifiers::NONE {
            suspend_terminal();
            let _ = self.pager_runner.run_pager(&self.diff_content);
            DiffCardAction::OpenedPager
        } else {
            DiffCardAction::Ignored
        }
    }
}

impl Widget for &DiffCard {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let mut y = area.y;
        if self.reply_anchor.is_some() && y < area.bottom() {
            self.render_badge(buf, Rect::new(area.x, y, area.width, 1));
            y += 1;
        }

        for line in self.preview_lines() {
            if y >= area.bottom() {
                break;
            }
            let style = match line.kind() {
                DiffLineKind::Addition => Style::default().fg(Color::Green),
                DiffLineKind::Deletion => Style::default().fg(Color::Red),
                DiffLineKind::Header => Style::default().fg(Color::Cyan),
                DiffLineKind::Context => Style::default().fg(Color::DarkGray),
            };
            buf.set_string(area.x, y, line.content(), style);
            y += 1;
        }
    }
}

impl Widget for DiffCard {
    fn render(self, area: Rect, buf: &mut Buffer) {
        (&self).render(area, buf);
    }
}

pub struct DiffCardBuilder {
    diff_content: String,
    reply_anchor: Option<String>,
    max_preview_lines: usize,
    pager_runner: Option<Arc<dyn PagerRunner>>,
}

impl Default for DiffCardBuilder {
    fn default() -> Self {
        Self {
            diff_content: String::new(),
            reply_anchor: None,
            max_preview_lines: 50,
            pager_runner: None,
        }
    }
}

impl DiffCardBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn diff_content(mut self, diff_content: impl Into<String>) -> Self {
        self.diff_content = diff_content.into();
        self
    }

    pub fn reply_anchor(mut self, reply_anchor: impl Into<String>) -> Self {
        self.reply_anchor = Some(reply_anchor.into());
        self
    }

    pub fn max_preview_lines(mut self, max_preview_lines: usize) -> Self {
        self.max_preview_lines = max_preview_lines;
        self
    }

    pub fn pager_runner<P: PagerRunner + 'static>(mut self, pager_runner: P) -> Self {
        self.pager_runner = Some(Arc::new(pager_runner));
        self
    }

    pub fn build(self) -> DiffCard {
        let pager = self
            .pager_runner
            .unwrap_or_else(|| Arc::new(DefaultPagerRunner::new()));

        DiffCard {
            diff_content: self.diff_content,
            reply_anchor: self.reply_anchor,
            max_preview_lines: self.max_preview_lines,
            pager_runner: pager,
        }
    }
}
