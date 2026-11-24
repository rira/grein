use crate::engine_port::PtyCell;
use crate::markdown::{char_display_width, LineMeta};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionPane {
    Transcript,
    Terminal,
    WorktreeDiff,
}

/// Inclusive cell-coordinate selection within a single TUI pane.
///
/// Both `anchor` and `focus` are `(row, col)` indices where `row` is the
/// unwrapped-scroll-relative content row and `col` is the 0-based display
/// column inside the pane's inner area. Selections cover `[start..=end]`
/// inclusively so `Drag`, `DoubleClick` (via [`word_bounds_at`]), and
/// `TripleClick` share identical endpoint semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneSelection {
    pub pane: SelectionPane,
    pub anchor: (usize, usize),
    pub focus: (usize, usize),
    pub dragging: bool,
}

impl PaneSelection {
    pub fn ordered(&self) -> ((usize, usize), (usize, usize)) {
        normalize_bounds(self.anchor, self.focus)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PaneRects {
    pub tree_inner: Option<Rect>,
    pub right_inner: Option<Rect>,
    pub composer_inner: Option<Rect>,
}

fn normalize_bounds(start: (usize, usize), end: (usize, usize)) -> ((usize, usize), (usize, usize)) {
    if start <= end {
        (start, end)
    } else {
        (end, start)
    }
}

fn line_non_trailing_space_limit(line: &Line<'_>) -> usize {
    let mut last_non_space: Option<usize> = None;
    let mut ordinal = 0usize;
    for span in &line.spans {
        for ch in span.content.chars() {
            if ch != ' ' {
                last_non_space = Some(ordinal + 1);
            }
            ordinal += 1;
        }
    }
    last_non_space.unwrap_or(0)
}

fn extract_single_line_cols(
    line: &Line<'_>,
    start_col: usize,
    end_col: usize,
    char_limit: Option<usize>,
) -> String {
    if start_col > end_col {
        return String::new();
    }
    let max_chars = char_limit.unwrap_or(usize::MAX);
    let mut out = String::new();
    let mut cur_col = 0usize;
    let mut ordinal = 0usize;
    let mut last_included = false;

    for span in &line.spans {
        for ch in span.content.chars() {
            if ordinal >= max_chars {
                return out;
            }
            ordinal += 1;

            let cw = char_display_width(ch);
            if cw == 0 {
                if !ch.is_control() && last_included {
                    out.push(ch);
                }
                continue;
            }

            let ch_start = cur_col;
            let ch_end = cur_col + cw - 1;
            cur_col += cw;

            if ch_start > end_col {
                return out;
            }

            if ch_end >= start_col {
                out.push(ch);
                last_included = true;
            } else {
                last_included = false;
            }
        }
    }

    out
}

pub fn extract_lines_selection(
    lines: &[Line<'static>],
    meta: &[LineMeta],
    start: (usize, usize),
    end: (usize, usize),
) -> String {
    let (start, end) = normalize_bounds(start, end);
    if lines.is_empty() || start.0 >= lines.len() {
        return String::new();
    }

    let first_row = start.0;
    let last_row = end.0.min(lines.len() - 1);
    let mut out = String::new();
    let mut logical_line_has_content = false;

    for (row, line) in lines
        .iter()
        .enumerate()
        .take(last_row + 1)
        .skip(first_row)
    {
        let row_meta = meta.get(row).copied().unwrap_or_default();
        if row > first_row {
            if row_meta.soft_wrapped {
                if row_meta.joined_by_space && logical_line_has_content {
                    out.push(' ');
                }
            } else {
                out.push('\n');
                logical_line_has_content = false;
            }
        }

        let row_start_col = if row == first_row { start.1 } else { 0 };
        let row_end_col = if row == end.0 { end.1 } else { usize::MAX };
        let is_hard_ended = !meta.get(row + 1).is_some_and(|m| m.soft_wrapped);
        let char_limit = is_hard_ended.then(|| line_non_trailing_space_limit(line));

        let slice = extract_single_line_cols(line, row_start_col, row_end_col, char_limit);
        if !slice.is_empty() {
            out.push_str(&slice);
            logical_line_has_content = true;
        }
    }

    out
}

fn is_blank_pty_cell(cell: &PtyCell) -> bool {
    !cell.is_wide_continuation && (cell.contents.is_empty() || cell.contents.chars().all(|c| c == ' '))
}

fn pty_row_effective_len(row: &[PtyCell], is_wrapped: bool) -> usize {
    if is_wrapped {
        return row.len();
    }
    let Some(last_content) = row
        .iter()
        .rposition(|cell| !cell.is_wide_continuation && !is_blank_pty_cell(cell))
    else {
        return 0;
    };
    let mut len = last_content + 1;
    while row.get(len).is_some_and(|cell| cell.is_wide_continuation) {
        len += 1;
    }
    len
}

fn extract_single_pty_row(
    row: &[PtyCell],
    is_wrapped: bool,
    raw_start: usize,
    raw_end: usize,
    out: &mut String,
) {
    if row.is_empty() || raw_start >= row.len() || raw_start > raw_end {
        return;
    }

    let mut col_start = raw_start;
    while col_start > 0 && row[col_start].is_wide_continuation {
        col_start -= 1;
    }

    let effective_len = pty_row_effective_len(row, is_wrapped);
    if col_start >= effective_len {
        return;
    }

    let mut col_end = raw_end.min(effective_len - 1);
    while col_end + 1 < row.len() && row[col_end + 1].is_wide_continuation {
        col_end += 1;
    }

    if col_start > col_end {
        return;
    }

    for cell in &row[col_start..=col_end] {
        if cell.is_wide_continuation {
            continue;
        }
        if cell.contents.is_empty() {
            out.push(' ');
        } else {
            out.push_str(&cell.contents);
        }
    }
}

pub fn extract_pty_selection(
    cells: &[Vec<PtyCell>],
    row_wrapped: &[bool],
    start: (usize, usize),
    end: (usize, usize),
) -> String {
    let (start, end) = normalize_bounds(start, end);
    if cells.is_empty() || start.0 >= cells.len() {
        return String::new();
    }

    let first_row = start.0;
    let last_row = end.0.min(cells.len() - 1);
    let mut out = String::new();

    for (r, row) in cells
        .iter()
        .enumerate()
        .take(last_row + 1)
        .skip(first_row)
    {
        let is_wrapped = row_wrapped.get(r).copied().unwrap_or(false);
        let raw_start = if r == first_row { start.1 } else { 0 };
        let raw_end = if r == end.0 {
            end.1
        } else {
            row.len().saturating_sub(1)
        };

        extract_single_pty_row(row, is_wrapped, raw_start, raw_end, &mut out);

        if r < last_row && !is_wrapped {
            out.push('\n');
        }
    }

    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WordRunClass {
    Word,
    Whitespace,
    Symbol,
}

impl WordRunClass {
    fn of(ch: char) -> Self {
        if ch.is_alphanumeric() || ch == '_' {
            Self::Word
        } else if ch.is_whitespace() {
            Self::Whitespace
        } else {
            Self::Symbol
        }
    }
}

pub fn word_bounds_at(line_text: &str, col: usize) -> Option<(usize, usize)> {
    let mut cur_col = 0usize;
    let mut active_run: Option<(WordRunClass, usize, usize)> = None;

    for ch in line_text.chars() {
        let cw = char_display_width(ch);
        if cw == 0 {
            continue;
        }
        let ch_start = cur_col;
        let ch_end = cur_col + cw - 1;
        cur_col += cw;

        let class = WordRunClass::of(ch);
        match active_run {
            Some((run_class, run_start, _)) if run_class == class => {
                active_run = Some((run_class, run_start, ch_end));
            }
            Some((_, run_start, run_end)) => {
                if col >= run_start && col <= run_end {
                    return Some((run_start, run_end));
                }
                active_run = Some((class, ch_start, ch_end));
            }
            None => {
                active_run = Some((class, ch_start, ch_end));
            }
        }
    }

    active_run.and_then(|(_, run_start, run_end)| (col >= run_start && col <= run_end).then_some((run_start, run_end)))
}

pub fn apply_selection_highlight(
    buf: &mut Buffer,
    area: Rect,
    scroll_y: usize,
    sel: &PaneSelection,
    bg: Color,
    fg: Color,
) {
    let visible = area.intersection(*buf.area());
    if visible.width == 0 || visible.height == 0 {
        return;
    }

    let (start, end) = sel.ordered();
    let highlight_style = Style::default()
        .bg(bg)
        .fg(fg)
        .remove_modifier(Modifier::REVERSED);

    for y in visible.top()..visible.bottom() {
        let row = scroll_y + usize::from(y - area.y);
        for x in visible.left()..visible.right() {
            let col = usize::from(x - area.x);
            if (row, col) >= start && (row, col) <= end {
                buf[(x, y)].set_style(highlight_style);
            }
        }
    }
}
