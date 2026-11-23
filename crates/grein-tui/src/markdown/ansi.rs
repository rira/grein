use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::wrap::push_merged_span;

pub fn render_ansi_text(text: &str) -> Vec<Line<'static>> {
    let mut style = Style::default();
    text.lines()
        .map(|line| render_ansi_line(line, &mut style))
        .collect()
}

pub(super) fn render_ansi_line(line: &str, style: &mut Style) -> Line<'static> {
    if line.trim_end_matches('\r') == "[stderr]" {
        *style = Style::default();
        return Line::from(Span::styled(
            "[stderr]".to_string(),
            Style::default().fg(Color::Red),
        ));
    }
    let spans = parse_ansi_spans(line, style);
    if spans.is_empty() {
        Line::raw("")
    } else {
        Line::from(spans)
    }
}

fn parse_ansi_spans(line: &str, style: &mut Style) -> Vec<Span<'static>> {
    let bytes = line.as_bytes();
    let mut spans = Vec::new();
    let mut plain_start = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        match bytes[i] {
            0x1B => {
                push_merged_span(&mut spans, &line[plain_start..i], *style);
                i = consume_escape_sequence(line, bytes, i, style);
                plain_start = i;
            }
            b'\r' => {
                push_merged_span(&mut spans, &line[plain_start..i], *style);
                i += 1;
                plain_start = i;
            }
            _ => {
                i += 1;
            }
        }
    }

    push_merged_span(&mut spans, &line[plain_start..], *style);
    spans
}

fn consume_escape_sequence(line: &str, bytes: &[u8], esc_idx: usize, style: &mut Style) -> usize {
    let Some(&next) = bytes.get(esc_idx + 1) else {
        return bytes.len();
    };

    match next {
        b'[' => {
            let mut j = esc_idx + 2;
            while j < bytes.len() && (0x20..=0x3F).contains(&bytes[j]) {
                j += 1;
            }
            if j >= bytes.len() {
                return bytes.len();
            }
            let final_byte = bytes[j];
            if (0x40..=0x7E).contains(&final_byte) {
                if final_byte == b'm' {
                    let params = &line[esc_idx + 2..j];
                    if params
                        .bytes()
                        .all(|b| b.is_ascii_digit() || b == b';' || b == b':')
                    {
                        apply_sgr_params(params, style);
                    }
                }
                j + 1
            } else {
                j
            }
        }
        b']' => {
            let mut j = esc_idx + 2;
            while j < bytes.len() {
                if bytes[j] == 0x07 {
                    return j + 1;
                }
                if bytes[j] == 0x1B && bytes.get(j + 1) == Some(&b'\\') {
                    return j + 2;
                }
                j += 1;
            }
            bytes.len()
        }
        b'(' | b')' | b'*' | b'+' => {
            if esc_idx + 2 < bytes.len() && bytes[esc_idx + 2].is_ascii() {
                esc_idx + 3
            } else if esc_idx + 2 < bytes.len() {
                esc_idx + 2
            } else {
                bytes.len()
            }
        }
        b if (0x20..=0x7E).contains(&b) => esc_idx + 2,
        _ => esc_idx + 1,
    }
}

fn parse_sgr_u32(tok: &str) -> Option<u32> {
    if tok.is_empty() {
        Some(0)
    } else {
        tok.parse::<u32>().ok()
    }
}

fn parse_sgr_u8(tok: &str) -> Option<u8> {
    tok.parse::<u32>().ok().and_then(|v| u8::try_from(v).ok())
}

fn set_fg_or_bg(style: &mut Style, color: Color, is_fg: bool) {
    if is_fg {
        *style = style.fg(color);
    } else {
        *style = style.bg(color);
    }
}

fn ansi_standard_color(idx: u8) -> Color {
    match idx {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        _ => Color::Gray,
    }
}

fn ansi_bright_color(idx: u8) -> Color {
    match idx {
        0 => Color::DarkGray,
        1 => Color::LightRed,
        2 => Color::LightGreen,
        3 => Color::LightYellow,
        4 => Color::LightBlue,
        5 => Color::LightMagenta,
        6 => Color::LightCyan,
        _ => Color::White,
    }
}

fn apply_extended_color(rest: &[&str], style: &mut Style, is_fg: bool) -> usize {
    let Some(mode) = rest.first().and_then(|t| parse_sgr_u32(t)) else {
        return 0;
    };
    match mode {
        5 => {
            if let Some(idx) = rest.get(1).and_then(|t| parse_sgr_u8(t)) {
                set_fg_or_bg(style, Color::Indexed(idx), is_fg);
            }
            rest.len().min(2)
        }
        2 => {
            if let (Some(r), Some(g), Some(b)) = (
                rest.get(1).and_then(|t| parse_sgr_u8(t)),
                rest.get(2).and_then(|t| parse_sgr_u8(t)),
                rest.get(3).and_then(|t| parse_sgr_u8(t)),
            ) {
                set_fg_or_bg(style, Color::Rgb(r, g, b), is_fg);
            }
            rest.len().min(4)
        }
        _ => 0,
    }
}

fn apply_colon_sgr(tok: &str, style: &mut Style) {
    let parts: Vec<&str> = tok.split(':').collect();
    let Some(head) = parts.first().and_then(|t| parse_sgr_u32(t)) else {
        return;
    };
    match head {
        4 => match parts.get(1).and_then(|t| parse_sgr_u32(t)) {
            Some(0) => *style = style.remove_modifier(Modifier::UNDERLINED),
            Some(1..=5) => *style = style.add_modifier(Modifier::UNDERLINED),
            _ => {}
        },
        38 | 48 => {
            let is_fg = head == 38;
            match parts.get(1).and_then(|t| parse_sgr_u32(t)) {
                Some(5) => {
                    if let Some(idx) = parts.get(2).and_then(|t| parse_sgr_u8(t)) {
                        set_fg_or_bg(style, Color::Indexed(idx), is_fg);
                    }
                }
                Some(2) => {
                    let rgb_start = if parts.len() >= 6 { 3 } else { 2 };
                    if let (Some(r), Some(g), Some(b)) = (
                        parts.get(rgb_start).and_then(|t| parse_sgr_u8(t)),
                        parts.get(rgb_start + 1).and_then(|t| parse_sgr_u8(t)),
                        parts.get(rgb_start + 2).and_then(|t| parse_sgr_u8(t)),
                    ) {
                        set_fg_or_bg(style, Color::Rgb(r, g, b), is_fg);
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
}

fn apply_sgr_params(params: &str, style: &mut Style) {
    if params.is_empty() {
        *style = Style::default();
        return;
    }

    let tokens: Vec<&str> = params.split(';').collect();
    let mut idx = 0usize;
    while idx < tokens.len() {
        let tok = tokens[idx];
        if tok.contains(':') {
            apply_colon_sgr(tok, style);
            idx += 1;
            continue;
        }
        match parse_sgr_u32(tok) {
            Some(0) => *style = Style::default(),
            Some(1) => *style = style.add_modifier(Modifier::BOLD),
            Some(2) => *style = style.add_modifier(Modifier::DIM),
            Some(3) => *style = style.add_modifier(Modifier::ITALIC),
            Some(4) => *style = style.add_modifier(Modifier::UNDERLINED),
            Some(7) => *style = style.add_modifier(Modifier::REVERSED),
            Some(22) => *style = style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            Some(23) => *style = style.remove_modifier(Modifier::ITALIC),
            Some(24) => *style = style.remove_modifier(Modifier::UNDERLINED),
            Some(27) => *style = style.remove_modifier(Modifier::REVERSED),
            Some(c @ 30..=37) => {
                *style = style.fg(ansi_standard_color((c - 30) as u8));
            }
            Some(39) => style.fg = None,
            Some(c @ 90..=97) => {
                *style = style.fg(ansi_bright_color((c - 90) as u8));
            }
            Some(c @ 40..=47) => {
                *style = style.bg(ansi_standard_color((c - 40) as u8));
            }
            Some(49) => style.bg = None,
            Some(c @ 100..=107) => {
                *style = style.bg(ansi_bright_color((c - 100) as u8));
            }
            Some(38) => {
                idx += apply_extended_color(&tokens[idx + 1..], style, true);
            }
            Some(48) => {
                idx += apply_extended_color(&tokens[idx + 1..], style, false);
            }
            _ => {}
        }
        idx += 1;
    }
}
