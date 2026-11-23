use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

const DEFAULT_SCROLLBACK_LEN: usize = 1000;

/// Terminal foreground or background color stored in a snapshot cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PtyColor {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// Single rendered cell within a [`PtyScreenSnapshot`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PtyCell {
    pub contents: String,
    pub fg: PtyColor,
    pub bg: PtyColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    pub is_wide_continuation: bool,
}

/// Immutable point-in-time snapshot of the VT100 terminal screen and process state.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PtyScreenSnapshot {
    pub rows: u16,
    pub cols: u16,
    pub cells: Vec<Vec<PtyCell>>,
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub hide_cursor: bool,
    pub application_cursor: bool,
    pub bracketed_paste: bool,
    pub alternate_screen: bool,
    pub exited: bool,
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub scrollback_offset: usize,
    #[serde(default)]
    pub cwd: Option<String>,
}

impl PtyScreenSnapshot {
    /// Renders the screen grid as plain UTF-8 lines, skipping wide-character
    /// continuation cells and trimming trailing spaces on each row.
    pub fn plain_text(&self) -> String {
        self.cells
            .iter()
            .map(|row| {
                let mut line = String::with_capacity(row.len());
                for cell in row {
                    if cell.is_wide_continuation {
                        continue;
                    }
                    if cell.contents.is_empty() {
                        line.push(' ');
                    } else {
                        line.push_str(&cell.contents);
                    }
                }
                line.trim_end_matches(' ').to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub(super) struct PtyState {
    pub(super) parser: vt100::Parser,
    pub(super) scrollback: VecDeque<Vec<PtyCell>>,
    pub(super) scrollback_offset: usize,
    scroll_top: u16,
    scroll_bottom: u16,
    pub(super) current_cwd: Option<String>,
    pub(super) last_os_cwd: Option<String>,
    pub(super) exited: bool,
    pub(super) exit_code: Option<i32>,
}

impl PtyState {
    pub(super) fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: vt100::Parser::new(rows, cols, 0),
            scrollback: VecDeque::new(),
            scrollback_offset: 0,
            scroll_top: 0,
            scroll_bottom: rows.saturating_sub(1),
            current_cwd: None,
            last_os_cwd: None,
            exited: false,
            exit_code: None,
        }
    }

    pub(super) fn scroll_region_active(&self) -> bool {
        let (rows, _) = self.parser.screen().size();
        self.scroll_top != 0 || self.scroll_bottom != rows.saturating_sub(1)
    }

    pub(super) fn push_scrollback_row(&mut self, row: Vec<PtyCell>) {
        if self.scrollback.len() >= DEFAULT_SCROLLBACK_LEN {
            self.scrollback.pop_front();
        }
        self.scrollback.push_back(row);
        if self.scrollback_offset > 0 {
            self.scrollback_offset = self.scrollback.len().min(self.scrollback_offset + 1);
        }
    }

    pub(super) fn set_scroll_region(&mut self, top: u16, bottom: u16) {
        let (rows, _) = self.parser.screen().size();
        let max_row = rows.saturating_sub(1);
        let top = top.saturating_sub(1);
        let bottom = if bottom == 0 {
            max_row
        } else {
            (bottom - 1).min(max_row)
        };
        if top < bottom {
            self.scroll_top = top;
            self.scroll_bottom = bottom;
        } else {
            self.scroll_top = 0;
            self.scroll_bottom = max_row;
        }
    }

    pub(super) fn resize(&mut self, rows: u16, cols: u16) {
        let (old_rows, _) = self.parser.screen().size();
        if self.scroll_bottom == old_rows.saturating_sub(1) {
            self.scroll_bottom = rows.saturating_sub(1);
        }
        if self.scroll_bottom >= rows {
            self.scroll_bottom = rows.saturating_sub(1);
        }
        if self.scroll_bottom < self.scroll_top {
            self.scroll_top = 0;
        }
        self.parser.set_size(rows, cols);
        self.clamp_scrollback_offset(self.scrollback_offset);
    }

    pub(super) fn reset(&mut self) {
        let (rows, _) = self.parser.screen().size();
        self.scrollback.clear();
        self.scrollback_offset = 0;
        self.scroll_top = 0;
        self.scroll_bottom = rows.saturating_sub(1);
    }

    pub(super) fn clamp_scrollback_offset(&mut self, target: usize) -> usize {
        if self.parser.screen().alternate_screen() {
            self.scrollback_offset = 0;
        } else {
            self.scrollback_offset = target.min(self.scrollback.len());
        }
        self.scrollback_offset
    }
}

pub(super) fn capture_screen_row(screen: &vt100::Screen, row: u16, cols: u16) -> Vec<PtyCell> {
    let mut row_cells = Vec::with_capacity(usize::from(cols));
    for c in 0..cols {
        let cell = match screen.cell(row, c) {
            Some(vt_cell) => PtyCell {
                contents: vt_cell.contents(),
                fg: map_vt100_color(vt_cell.fgcolor()),
                bg: map_vt100_color(vt_cell.bgcolor()),
                bold: vt_cell.bold(),
                italic: vt_cell.italic(),
                underline: vt_cell.underline(),
                inverse: vt_cell.inverse(),
                is_wide_continuation: vt_cell.is_wide_continuation(),
            },
            None => PtyCell::default(),
        };
        row_cells.push(cell);
    }
    row_cells
}

pub(super) fn fit_row_to_cols(row: &[PtyCell], cols: u16) -> Vec<PtyCell> {
    let target_len = usize::from(cols);
    if row.len() == target_len {
        return row.to_vec();
    }
    let mut fitted = Vec::with_capacity(target_len);
    if row.len() < target_len {
        fitted.extend_from_slice(row);
        fitted.resize_with(target_len, PtyCell::default);
    } else {
        fitted.extend_from_slice(&row[..target_len]);
        if target_len > 0
            && row
                .get(target_len)
                .is_some_and(|next| next.is_wide_continuation)
        {
            fitted[target_len - 1].contents.clear();
        }
    }
    fitted
}

pub(super) fn map_vt100_color(color: vt100::Color) -> PtyColor {
    match color {
        vt100::Color::Default => PtyColor::Default,
        vt100::Color::Idx(idx) => PtyColor::Indexed(idx),
        vt100::Color::Rgb(r, g, b) => PtyColor::Rgb(r, g, b),
    }
}
