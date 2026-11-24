use std::collections::VecDeque;

pub use grein_core::port::dto::{
    PtyCell, PtyColor, PtyMouseEncoding, PtyMouseMode, PtyScreenSnapshot,
};

const DEFAULT_SCROLLBACK_LEN: usize = 1000;

pub(super) struct PtyState {
    pub(super) parser: vt100::Parser,
    pub(super) scrollback: VecDeque<(Vec<PtyCell>, bool)>,
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

    pub(super) fn push_scrollback_row(&mut self, row: Vec<PtyCell>, wrapped: bool) {
        if self.scrollback.len() >= DEFAULT_SCROLLBACK_LEN {
            self.scrollback.pop_front();
        }
        self.scrollback.push_back((row, wrapped));
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

    pub(super) fn snapshot(&self) -> PtyScreenSnapshot {
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        let (cursor_row, cursor_col) = screen.cursor_position();
        let alternate_screen = screen.alternate_screen();
        let scrollback_offset = if alternate_screen {
            0
        } else {
            self.scrollback_offset.min(self.scrollback.len())
        };

        let row_count = usize::from(rows);
        let from_scrollback = scrollback_offset.min(row_count);
        let sb_start = self.scrollback.len().saturating_sub(scrollback_offset);

        let mut cells = Vec::with_capacity(row_count);
        let mut row_wrapped = Vec::with_capacity(row_count);
        for idx in 0..from_scrollback {
            if let Some((sb_row, wrapped)) = self.scrollback.get(sb_start + idx) {
                cells.push(fit_row_to_cols(sb_row, cols));
                row_wrapped.push(*wrapped);
            }
        }
        let live_rows = row_count.saturating_sub(from_scrollback);
        for r in 0..rows {
            if usize::from(r) >= live_rows {
                break;
            }
            cells.push(capture_screen_row(screen, r, cols));
            row_wrapped.push(screen.row_wrapped(r));
        }

        PtyScreenSnapshot {
            rows,
            cols,
            cells,
            cursor_row,
            cursor_col,
            hide_cursor: screen.hide_cursor(),
            application_cursor: screen.application_cursor(),
            bracketed_paste: screen.bracketed_paste(),
            alternate_screen,
            exited: self.exited,
            exit_code: self.exit_code,
            scrollback_offset,
            cwd: self.current_cwd.clone(),
            row_wrapped,
            mouse_protocol_mode: map_vt100_mouse_mode(screen.mouse_protocol_mode()),
            mouse_protocol_encoding: map_vt100_mouse_encoding(screen.mouse_protocol_encoding()),
        }
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

pub(super) fn map_vt100_mouse_mode(mode: vt100::MouseProtocolMode) -> PtyMouseMode {
    match mode {
        vt100::MouseProtocolMode::None => PtyMouseMode::None,
        vt100::MouseProtocolMode::Press => PtyMouseMode::Press,
        vt100::MouseProtocolMode::PressRelease => PtyMouseMode::PressRelease,
        vt100::MouseProtocolMode::ButtonMotion => PtyMouseMode::ButtonMotion,
        vt100::MouseProtocolMode::AnyMotion => PtyMouseMode::AnyMotion,
    }
}

pub(super) fn map_vt100_mouse_encoding(encoding: vt100::MouseProtocolEncoding) -> PtyMouseEncoding {
    match encoding {
        vt100::MouseProtocolEncoding::Default => PtyMouseEncoding::Default,
        vt100::MouseProtocolEncoding::Utf8 => PtyMouseEncoding::Utf8,
        vt100::MouseProtocolEncoding::Sgr => PtyMouseEncoding::Sgr,
    }
}
