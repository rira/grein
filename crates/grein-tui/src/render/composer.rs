use crate::theme::ThemePalette;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

fn char_display_width(ch: char) -> usize {
    if ch.is_ascii() {
        if ch.is_control() {
            0
        } else {
            1
        }
    } else {
        let mut buf = [0u8; 4];
        Span::raw(&*ch.encode_utf8(&mut buf)).width()
    }
}

pub(super) fn clamp_char_boundary(text: &str, pos: usize) -> usize {
    let mut idx = pos.min(text.len());
    while idx > 0 && !text.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

pub(crate) struct ComposerVisualRow {
    pub stops: Vec<(usize, usize)>,
}

pub(crate) struct ComposerVisualLayout {
    pub lines: Vec<Line<'static>>,
    pub rows: Vec<ComposerVisualRow>,
    pub cursor_col: u16,
    pub cursor_row: u16,
}

pub(super) struct ComposerTextLayout {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) cursor_col: u16,
    pub(super) cursor_row: u16,
}

pub(crate) fn layout_composer_visual_rows(
    input: &str,
    cursor_pos: usize,
    max_width: Option<usize>,
) -> ComposerVisualLayout {
    if max_width == Some(0) {
        return ComposerVisualLayout {
            lines: vec![Line::raw("")],
            rows: vec![ComposerVisualRow { stops: vec![(0, 0)] }],
            cursor_col: 0,
            cursor_row: 0,
        };
    }

    let cursor_byte = clamp_char_boundary(input, cursor_pos);
    let mut out_lines: Vec<Line<'static>> = Vec::new();
    let mut out_rows: Vec<ComposerVisualRow> = Vec::new();
    let mut cursor_col = 0usize;
    let mut cursor_row = 0usize;
    let mut cursor_set = false;
    let mut line_start = 0usize;

    for logical_line in input.split('\n') {
        let mut cur_line = String::new();
        let mut cur_stops: Vec<(usize, usize)> = Vec::new();
        let mut cur_w = 0usize;
        let mut idx = 0usize;
        let bytes = logical_line.as_bytes();

        while idx < bytes.len() {
            let ch = logical_line[idx..].chars().next().unwrap();
            if ch.is_whitespace() {
                let byte_idx = line_start + idx;
                let cw = if ch == '\t' { 1 } else { char_display_width(ch) };
                if max_width.is_some_and(|w| cur_w > 0 && cur_w + cw > w) {
                    out_lines.push(Line::from(std::mem::take(&mut cur_line)));
                    out_rows.push(ComposerVisualRow {
                        stops: std::mem::take(&mut cur_stops),
                    });
                    cur_w = 0;
                }
                cur_stops.push((byte_idx, cur_w));
                if byte_idx == cursor_byte && !cursor_set {
                    cursor_col = cur_w;
                    cursor_row = out_rows.len();
                    cursor_set = true;
                }
                cur_line.push(if ch == '\t' { ' ' } else { ch });
                cur_w += cw;
                idx += ch.len_utf8();
            } else {
                let word_start = idx;
                let mut word_w = 0usize;
                while idx < bytes.len() {
                    let wch = logical_line[idx..].chars().next().unwrap();
                    if wch.is_whitespace() {
                        break;
                    }
                    word_w += char_display_width(wch);
                    idx += wch.len_utf8();
                }
                let word = &logical_line[word_start..idx];
                if max_width.is_some_and(|w| cur_w > 0 && cur_w + word_w > w && word_w <= w) {
                    out_lines.push(Line::from(std::mem::take(&mut cur_line)));
                    out_rows.push(ComposerVisualRow {
                        stops: std::mem::take(&mut cur_stops),
                    });
                    cur_w = 0;
                }
                for (rel_idx, wch) in word.char_indices() {
                    let byte_idx = line_start + word_start + rel_idx;
                    let cw = char_display_width(wch);
                    if max_width.is_some_and(|w| cur_w > 0 && cur_w + cw > w) {
                        out_lines.push(Line::from(std::mem::take(&mut cur_line)));
                        out_rows.push(ComposerVisualRow {
                            stops: std::mem::take(&mut cur_stops),
                        });
                        cur_w = 0;
                    }
                    cur_stops.push((byte_idx, cur_w));
                    if byte_idx == cursor_byte && !cursor_set {
                        cursor_col = cur_w;
                        cursor_row = out_rows.len();
                        cursor_set = true;
                    }
                    cur_line.push(wch);
                    cur_w += cw;
                }
            }
        }

        let line_end = line_start + logical_line.len();
        if line_end == cursor_byte && !cursor_set {
            if max_width.is_some_and(|w| cur_w >= w) {
                out_lines.push(Line::from(std::mem::take(&mut cur_line)));
                out_rows.push(ComposerVisualRow {
                    stops: std::mem::take(&mut cur_stops),
                });
                cur_w = 0;
            }
            cursor_col = cur_w;
            cursor_row = out_rows.len();
            cursor_set = true;
        }
        cur_stops.push((line_end, cur_w));
        out_lines.push(Line::from(cur_line));
        out_rows.push(ComposerVisualRow { stops: cur_stops });
        line_start = line_end + 1;
    }

    if out_rows.is_empty() {
        out_lines.push(Line::raw(""));
        out_rows.push(ComposerVisualRow { stops: vec![(0, 0)] });
    }

    ComposerVisualLayout {
        lines: out_lines,
        rows: out_rows,
        cursor_col: cursor_col.min(u16::MAX as usize) as u16,
        cursor_row: cursor_row.min(u16::MAX as usize) as u16,
    }
}

pub(super) fn layout_composer_text(
    input: &str,
    cursor_pos: usize,
    inner_width: usize,
) -> ComposerTextLayout {
    let layout = layout_composer_visual_rows(input, cursor_pos, Some(inner_width));
    ComposerTextLayout {
        lines: layout.lines,
        cursor_col: layout.cursor_col,
        cursor_row: layout.cursor_row,
    }
}

fn clip_to_display_width(text: &str, max_width: usize) -> String {
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        if ch == '\n' || ch == '\r' {
            break;
        }
        let cw = char_display_width(ch);
        if used + cw > max_width {
            break;
        }
        out.push(ch);
        used += cw;
    }
    out
}

pub(super) fn render_composer_ghost_suffix(
    f: &mut Frame,
    inner_area: Rect,
    cursor_col: u16,
    visible_row: u16,
    ghost: &str,
    theme: &ThemePalette,
) {
    if inner_area.width == 0
        || inner_area.height == 0
        || cursor_col >= inner_area.width
        || visible_row >= inner_area.height
    {
        return;
    }
    let remaining = (inner_area.width - cursor_col) as usize;
    let clipped = clip_to_display_width(ghost, remaining);
    if clipped.is_empty() {
        return;
    }
    let mut style = Style::default()
        .fg(theme.ghost_text)
        .add_modifier(Modifier::ITALIC);
    if theme.panel_bg != Color::Reset {
        style = style.bg(theme.panel_bg);
    }
    let x = inner_area.x + cursor_col;
    let y = inner_area.y + visible_row;
    f.buffer_mut().set_stringn(x, y, &clipped, remaining, style);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemePreset;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn test_layout_composer_zero_and_single_width_with_cjk_emoji_and_tabs() {
        let zero = layout_composer_visual_rows("abc\t中😀", 3, Some(0));
        assert_eq!(zero.lines.len(), 1);
        assert_eq!(zero.cursor_col, 0);
        assert_eq!(zero.cursor_row, 0);

        let one = layout_composer_visual_rows("a\tb中😀c", 0, Some(1));
        assert!(one.lines.len() >= 5);
        assert_eq!(one.cursor_col, 0);

        let end_cursor = layout_composer_visual_rows("ab", 2, Some(2));
        assert_eq!(end_cursor.cursor_row, 1);
        assert_eq!(end_cursor.cursor_col, 0);

        assert_eq!(clip_to_display_width("a中b\nc", 3), "a中");
        assert_eq!(clip_to_display_width("😀x", 1), "");

        let backend = TestBackend::new(10, 4);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = ThemePalette::default();
        terminal
            .draw(|f| {
                render_composer_ghost_suffix(f, Rect::new(0, 0, 5, 2), 2, 0, "hello world", &theme);
                render_composer_ghost_suffix(f, Rect::new(0, 0, 0, 0), 0, 0, "hello", &theme);
                render_composer_ghost_suffix(f, Rect::new(0, 0, 5, 2), 5, 0, "hello", &theme);
            })
            .unwrap();
        let cell = &terminal.backend().buffer()[(2, 0)];
        assert_eq!(cell.symbol(), "h");
        assert_eq!(cell.fg, theme.ghost_text);
        assert_eq!(cell.bg, theme.panel_bg);
        assert!(cell.modifier.contains(Modifier::ITALIC));

        let ansi = ThemePalette::from_preset(ThemePreset::Ansi);
        terminal
            .draw(|f| {
                render_composer_ghost_suffix(f, Rect::new(0, 0, 5, 2), 1, 0, "ok", &ansi);
            })
            .unwrap();
        let ansi_cell = &terminal.backend().buffer()[(1, 0)];
        assert_eq!(ansi_cell.fg, ansi.ghost_text);
        assert_eq!(ansi_cell.bg, Color::Reset);
        assert!(ansi_cell.modifier.contains(Modifier::ITALIC));
    }
}
