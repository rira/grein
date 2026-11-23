use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::inline::{parse_inline_spans, parse_inline_spans_with_base};
use super::wrap::{spans_display_width, wrap_spans_to_width};
use super::{
    is_horizontal_rule, parse_opening_fence, strip_blockquote_prefix, strip_heading_prefix,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Alignment {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone)]
pub(super) struct TableData {
    alignments: Vec<Alignment>,
    header: Vec<String>,
    rows: Vec<Vec<String>>,
}

fn contains_unescaped_pipe(line: &str) -> bool {
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.peek() == Some(&'|') {
            chars.next();
            continue;
        }
        if ch == '|' {
            return true;
        }
    }
    false
}

fn split_table_row(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut chars = trimmed.chars().peekable();
    let mut idx = 0usize;
    let mut ended_with_unescaped_pipe = false;

    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.peek() == Some(&'|') {
            chars.next();
            cur.push('|');
            ended_with_unescaped_pipe = false;
        } else if ch == '|' {
            if idx > 0 {
                cells.push(cur.trim().replace('\t', " "));
            }
            cur.clear();
            ended_with_unescaped_pipe = true;
        } else {
            cur.push(ch);
            ended_with_unescaped_pipe = false;
        }
        idx += 1;
    }

    if !ended_with_unescaped_pipe || !cur.is_empty() {
        cells.push(cur.trim().replace('\t', " "));
    }

    cells
}

fn parse_delimiter_row(line: &str) -> Option<Vec<Alignment>> {
    if !contains_unescaped_pipe(line) {
        return None;
    }
    let cells = split_table_row(line);
    if cells.is_empty() {
        return None;
    }
    let mut alignments = Vec::with_capacity(cells.len());
    for cell in cells {
        let c = cell.trim();
        if c.is_empty() {
            return None;
        }
        let left = c.starts_with(':');
        let right = c.ends_with(':');
        let mid = c.strip_prefix(':').unwrap_or(c);
        let mid = mid.strip_suffix(':').unwrap_or(mid);
        if mid.is_empty() || !mid.chars().all(|ch| ch == '-') {
            return None;
        }
        let align = match (left, right) {
            (true, true) => Alignment::Center,
            (false, true) => Alignment::Right,
            _ => Alignment::Left,
        };
        alignments.push(align);
    }
    Some(alignments)
}

fn is_table_terminating_line(trimmed: &str) -> bool {
    trimmed.is_empty()
        || parse_opening_fence(trimmed).is_some()
        || strip_heading_prefix(trimmed).is_some()
        || strip_blockquote_prefix(trimmed).is_some()
        || is_horizontal_rule(trimmed)
        || !contains_unescaped_pipe(trimmed)
}

pub(super) fn try_parse_table(raw_lines: &[&str], start: usize) -> Option<(TableData, usize)> {
    let header_line = raw_lines.get(start)?;
    let delim_line = raw_lines.get(start + 1)?;

    let header_trimmed = header_line.trim();
    if parse_opening_fence(header_trimmed).is_some()
        || strip_blockquote_prefix(header_trimmed).is_some()
        || !contains_unescaped_pipe(header_trimmed)
    {
        return None;
    }

    let mut alignments = parse_delimiter_row(delim_line)?;
    let mut header = split_table_row(header_trimmed);
    if header.is_empty() {
        return None;
    }

    let mut rows = Vec::new();
    let mut idx = start + 2;
    while idx < raw_lines.len() {
        let row_trimmed = raw_lines[idx].trim();
        if is_table_terminating_line(row_trimmed) {
            break;
        }
        rows.push(split_table_row(row_trimmed));
        idx += 1;
    }

    let ncols = header
        .len()
        .max(alignments.len())
        .max(rows.iter().map(Vec::len).max().unwrap_or(0))
        .max(1);

    header.resize(ncols, String::new());
    alignments.resize(ncols, Alignment::Left);
    for row in &mut rows {
        row.resize(ncols, String::new());
    }

    Some((
        TableData {
            alignments,
            header,
            rows,
        },
        idx,
    ))
}

pub(super) fn render_table(table: &TableData, max_width: Option<usize>) -> Vec<Line<'static>> {
    let ncols = table.header.len();
    let header_style = Style::default().add_modifier(Modifier::BOLD);
    let border_style = Style::default().fg(Color::DarkGray);

    let parsed_header: Vec<Vec<Span<'static>>> = table
        .header
        .iter()
        .map(|cell| parse_inline_spans_with_base(cell, header_style))
        .collect();

    let parsed_rows: Vec<Vec<Vec<Span<'static>>>> = table
        .rows
        .iter()
        .map(|row| row.iter().map(|cell| parse_inline_spans(cell)).collect())
        .collect();

    let mut natural_widths = vec![1usize; ncols];
    for (c, cell) in parsed_header.iter().enumerate() {
        natural_widths[c] = natural_widths[c].max(spans_display_width(cell));
    }
    for row in &parsed_rows {
        for (c, cell) in row.iter().enumerate() {
            natural_widths[c] = natural_widths[c].max(spans_display_width(cell));
        }
    }

    let col_widths = allocate_column_widths(&natural_widths, max_width);

    let wrapped_header: Vec<Vec<Vec<Span<'static>>>> = parsed_header
        .into_iter()
        .enumerate()
        .map(|(c, spans)| wrap_spans_to_width(spans, col_widths[c]))
        .collect();

    let wrapped_rows: Vec<Vec<Vec<Vec<Span<'static>>>>> = parsed_rows
        .into_iter()
        .map(|row| {
            row.into_iter()
                .enumerate()
                .map(|(c, spans)| wrap_spans_to_width(spans, col_widths[c]))
                .collect()
        })
        .collect();

    let header_height = wrapped_header.iter().map(Vec::len).max().unwrap_or(1);
    let body_heights: Vec<usize> = wrapped_rows
        .iter()
        .map(|row| row.iter().map(Vec::len).max().unwrap_or(1))
        .collect();
    let has_wrapped_rows = header_height > 1 || body_heights.iter().any(|&h| h > 1);

    let mut lines = Vec::new();
    lines.push(build_border_line(&col_widths, '┌', '┬', '┐', border_style));

    for sub in 0..header_height {
        lines.push(build_table_content_line(
            &wrapped_header,
            sub,
            &col_widths,
            &table.alignments,
            border_style,
        ));
    }

    lines.push(build_border_line(&col_widths, '├', '┼', '┤', border_style));

    for (r_idx, row) in wrapped_rows.iter().enumerate() {
        let height = body_heights[r_idx];
        for sub in 0..height {
            lines.push(build_table_content_line(
                row,
                sub,
                &col_widths,
                &table.alignments,
                border_style,
            ));
        }
        if has_wrapped_rows && r_idx + 1 < wrapped_rows.len() {
            lines.push(build_border_line(&col_widths, '├', '┼', '┤', border_style));
        }
    }

    lines.push(build_border_line(&col_widths, '└', '┴', '┘', border_style));
    lines
}

fn allocate_column_widths(natural_widths: &[usize], max_width: Option<usize>) -> Vec<usize> {
    let ncols = natural_widths.len();
    if ncols == 0 {
        return Vec::new();
    }
    let Some(max_w) = max_width.filter(|&w| w > 0) else {
        return natural_widths.to_vec();
    };

    let chrome = 3 * ncols + 1;
    let natural_total: usize = natural_widths.iter().sum::<usize>() + chrome;
    if natural_total <= max_w {
        return natural_widths.to_vec();
    }

    let budget = max_w.saturating_sub(chrome);
    if budget < ncols {
        return vec![1usize; ncols];
    }

    let mut order: Vec<usize> = (0..ncols).collect();
    order.sort_by_key(|&c| natural_widths[c]);

    let mut widths = vec![1usize; ncols];
    let mut remaining_budget = budget;
    let mut remaining_cols = ncols;

    for (pos, &col_idx) in order.iter().enumerate() {
        let fair = remaining_budget / remaining_cols;
        if natural_widths[col_idx] <= fair {
            widths[col_idx] = natural_widths[col_idx];
            remaining_budget -= widths[col_idx];
            remaining_cols -= 1;
        } else {
            let rem_slice = &order[pos..];
            let base = remaining_budget / remaining_cols;
            let extra = remaining_budget % remaining_cols;
            for (k, &rem_idx) in rem_slice.iter().enumerate() {
                let bump = usize::from(k >= remaining_cols - extra);
                widths[rem_idx] = (base + bump).max(1);
            }
            break;
        }
    }

    widths
}

fn build_border_line(
    col_widths: &[usize],
    left: char,
    mid: char,
    right: char,
    style: Style,
) -> Line<'static> {
    let mut s = String::new();
    s.push(left);
    for (idx, &w) in col_widths.iter().enumerate() {
        if idx > 0 {
            s.push(mid);
        }
        for _ in 0..(w + 2) {
            s.push('─');
        }
    }
    s.push(right);
    Line::from(Span::styled(s, style))
}

fn build_table_content_line(
    wrapped_row: &[Vec<Vec<Span<'static>>>],
    subline: usize,
    col_widths: &[usize],
    alignments: &[Alignment],
    border_style: Style,
) -> Line<'static> {
    let mut spans = Vec::new();
    spans.push(Span::styled("│", border_style));

    for (c, &col_w) in col_widths.iter().enumerate() {
        let cell_spans = wrapped_row[c].get(subline).map(Vec::as_slice).unwrap_or(&[]);
        let cell_w = spans_display_width(cell_spans);
        let pad_total = col_w.saturating_sub(cell_w);
        let (pad_left, pad_right) = match alignments[c] {
            Alignment::Left => (0, pad_total),
            Alignment::Right => (pad_total, 0),
            Alignment::Center => (pad_total / 2, pad_total - pad_total / 2),
        };

        spans.push(Span::raw(" ".repeat(1 + pad_left)));
        spans.extend(cell_spans.iter().cloned());
        spans.push(Span::raw(" ".repeat(pad_right + 1)));
        spans.push(Span::styled("│", border_style));
    }

    Line::from(spans)
}
