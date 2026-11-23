use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use super::wrap::{push_merged_char, push_merged_span};

pub fn parse_inline_spans(text: &str) -> Vec<Span<'static>> {
    parse_inline_spans_with_base(text, Style::default())
}

pub(super) fn parse_inline_spans_with_base(text: &str, base: Style) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    parse_inline_into(text, base, &mut out);
    out
}

fn parse_inline_into(text: &str, base: Style, out: &mut Vec<Span<'static>>) {
    let mut plain_start = 0usize;
    let mut i = 0usize;

    while i < text.len() {
        let ch = text[i..].chars().next().unwrap();
        let next_i = i + ch.len_utf8();

        match ch {
            '\\' => {
                if let Some(escaped) = text[next_i..].chars().next() {
                    if matches!(escaped, '*' | '_' | '`' | '\\' | '|') {
                        push_merged_span(out, &text[plain_start..i], base);
                        push_merged_char(out, escaped, base);
                        i = next_i + escaped.len_utf8();
                        plain_start = i;
                        continue;
                    }
                }
                i = next_i;
            }
            '`' => {
                if let Some((open_len, close_start, next_pos)) =
                    find_closing_backtick_run(text, i)
                {
                    let raw = &text[i + open_len..close_start];
                    let code = if raw.len() >= 2
                        && raw.starts_with(' ')
                        && raw.ends_with(' ')
                        && raw.bytes().any(|b| b != b' ')
                    {
                        &raw[1..raw.len() - 1]
                    } else {
                        raw
                    };
                    if !code.is_empty() {
                        push_merged_span(out, &text[plain_start..i], base);
                        push_merged_span(out, code, base.fg(Color::Yellow));
                        i = next_pos;
                        plain_start = i;
                        continue;
                    }
                }
                let run = text[i..].bytes().take_while(|&b| b == b'`').count();
                i += run;
            }
            '*' => {
                if text[i..].starts_with("***") {
                    let rest = &text[i + 3..];
                    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
                        if let Some(rel_end) = find_closing_star_run(rest, 3) {
                            push_merged_span(out, &text[plain_start..i], base);
                            parse_inline_into(
                                &rest[..rel_end],
                                base.add_modifier(Modifier::BOLD | Modifier::ITALIC),
                                out,
                            );
                            i = i + 3 + rel_end + 3;
                            plain_start = i;
                            continue;
                        }
                    }
                }

                if text[i..].starts_with("**") {
                    let rest = &text[i + 2..];
                    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
                        if let Some(rel_end) = find_closing_star_run(rest, 2) {
                            push_merged_span(out, &text[plain_start..i], base);
                            parse_inline_into(
                                &rest[..rel_end],
                                base.add_modifier(Modifier::BOLD),
                                out,
                            );
                            i = i + 2 + rel_end + 2;
                            plain_start = i;
                            continue;
                        }
                    }
                    let star_run = text[i..].bytes().take_while(|&b| b == b'*').count();
                    i += star_run;
                    continue;
                }

                let rest = &text[next_i..];
                if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
                    if let Some(rel_end) = find_closing_single_star(rest) {
                        push_merged_span(out, &text[plain_start..i], base);
                        parse_inline_into(
                            &rest[..rel_end],
                            base.add_modifier(Modifier::ITALIC),
                            out,
                        );
                        i = next_i + rel_end + 1;
                        plain_start = i;
                        continue;
                    }
                }
                i = next_i;
            }
            '_' => {
                let prev_char = text[..i].chars().next_back();
                let valid_prev = prev_char.is_none_or(|c| !c.is_alphanumeric() && c != '_');
                let next_char = text[next_i..].chars().next();
                let valid_next = next_char.is_some_and(|c| !c.is_whitespace() && c != '_');

                if valid_prev && valid_next {
                    let rest = &text[next_i..];
                    if let Some(rel_end) = find_closing_underscore(rest) {
                        push_merged_span(out, &text[plain_start..i], base);
                        parse_inline_into(
                            &rest[..rel_end],
                            base.add_modifier(Modifier::ITALIC),
                            out,
                        );
                        i = next_i + rel_end + 1;
                        plain_start = i;
                        continue;
                    }
                }
                i = next_i;
            }
            _ => {
                i = next_i;
            }
        }
    }

    push_merged_span(out, &text[plain_start..], base);
}

fn find_closing_backtick_run(text: &str, start: usize) -> Option<(usize, usize, usize)> {
    let open_len = text[start..].bytes().take_while(|&b| b == b'`').count();
    if open_len == 0 {
        return None;
    }
    let mut j = start + open_len;
    while j < text.len() {
        if text.as_bytes()[j] == b'`' {
            let close_len = text[j..].bytes().take_while(|&b| b == b'`').count();
            if close_len == open_len {
                return Some((open_len, j, j + close_len));
            }
            j += close_len;
        } else {
            let ch = text[j..].chars().next().unwrap();
            j += ch.len_utf8();
        }
    }
    None
}

fn skip_backtick_span(text: &str, tick_pos: usize) -> Option<usize> {
    find_closing_backtick_run(text, tick_pos).map(|(_, _, next_pos)| next_pos)
}

fn skip_escaped_or_backtick(text: &str, i: usize, ch: char) -> Option<usize> {
    match ch {
        '\\' => {
            let next_i = i + 1;
            if let Some(escaped) = text[next_i..].chars().next() {
                if matches!(escaped, '*' | '_' | '`' | '\\' | '|') {
                    return Some(next_i + escaped.len_utf8());
                }
            }
            Some(next_i)
        }
        '`' => {
            let run = text[i..].bytes().take_while(|&b| b == b'`').count();
            Some(skip_backtick_span(text, i).unwrap_or(i + run))
        }
        _ => None,
    }
}

fn find_closing_star_run(rest: &str, count: usize) -> Option<usize> {
    let mut i = 0usize;
    while i < rest.len() {
        let ch = rest[i..].chars().next().unwrap();
        if let Some(next_i) = skip_escaped_or_backtick(rest, i, ch) {
            i = next_i;
            continue;
        }
        if ch == '*' {
            let run = rest[i..].bytes().take_while(|&b| b == b'*').count();
            if run >= count && i > 0 {
                let before_run = rest[..i].chars().next_back();
                if before_run.is_some_and(|c| !c.is_whitespace()) {
                    return Some(i + (run - count));
                }
            }
            i += run;
        } else {
            i += ch.len_utf8();
        }
    }
    None
}

fn find_closing_single_star(rest: &str) -> Option<usize> {
    let mut i = 0usize;
    while i < rest.len() {
        let ch = rest[i..].chars().next().unwrap();
        if let Some(next_i) = skip_escaped_or_backtick(rest, i, ch) {
            i = next_i;
            continue;
        }
        if ch == '*' {
            let run = rest[i..].bytes().take_while(|&b| b == b'*').count();
            if run == 1 && i > 0 {
                let before = rest[..i].chars().next_back();
                if before.is_some_and(|c| !c.is_whitespace()) {
                    return Some(i);
                }
            } else if run >= 2
                && rest[i + 2..]
                    .chars()
                    .next()
                    .is_some_and(|c| !c.is_whitespace())
            {
                if let Some(inner_close) = find_closing_star_run(&rest[i + 2..], 2) {
                    i = i + 2 + inner_close + 2;
                    continue;
                }
            }
            i += run;
        } else {
            i += ch.len_utf8();
        }
    }
    None
}

fn find_closing_underscore(rest: &str) -> Option<usize> {
    let mut i = 0usize;
    while i < rest.len() {
        let ch = rest[i..].chars().next().unwrap();
        if let Some(next_i) = skip_escaped_or_backtick(rest, i, ch) {
            i = next_i;
            continue;
        }
        if ch == '_' {
            if i > 0 {
                let before = rest[..i].chars().next_back();
                let after = rest[i + 1..].chars().next();
                let valid_before = before.is_some_and(|c| !c.is_whitespace() && c != '_');
                let valid_after = after.is_none_or(|c| !c.is_alphanumeric() && c != '_');
                if valid_before && valid_after {
                    return Some(i);
                }
            }
            i += 1;
        } else {
            i += ch.len_utf8();
        }
    }
    None
}
