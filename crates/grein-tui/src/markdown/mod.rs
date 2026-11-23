mod ansi;
mod inline;
mod table;
mod wrap;

pub use ansi::render_ansi_text;
pub use inline::parse_inline_spans;
pub use wrap::{wrap_styled_line, wrap_styled_lines};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use ansi::render_ansi_line;
use inline::parse_inline_spans_with_base;
use table::{render_table, try_parse_table};

pub fn render_markdown_lines(markdown: &str) -> Vec<Line<'static>> {
    render_markdown_internal(markdown, None)
}

pub fn render_markdown_lines_with_width(markdown: &str, max_width: usize) -> Vec<Line<'static>> {
    render_markdown_internal(markdown, Some(max_width))
}

#[derive(Debug, Clone)]
struct ActiveFence {
    fence_char: char,
    fence_len: usize,
    lang: String,
    in_block_comment: bool,
    ansi_style: Style,
}

fn flush_pending_accent(
    out: &mut Vec<Line<'static>>,
    pending_assistant_accent: &mut bool,
    accent_style: Style,
) {
    if std::mem::take(pending_assistant_accent) {
        out.push(Line::from(Span::styled("✦", accent_style)));
    }
}

fn push_with_pending_accent(
    out: &mut Vec<Line<'static>>,
    mut line: Line<'static>,
    pending_assistant_accent: &mut bool,
    accent_style: Style,
) {
    if std::mem::take(pending_assistant_accent) {
        let mut spans = Vec::with_capacity(line.spans.len() + 1);
        spans.push(Span::styled("✦ ", accent_style));
        spans.extend(line.spans);
        line.spans = spans;
    }
    out.push(line);
}

fn render_markdown_internal(markdown: &str, max_width: Option<usize>) -> Vec<Line<'static>> {
    let raw_lines: Vec<&str> = markdown.lines().collect();
    let mut out = Vec::new();
    let mut active_fence: Option<ActiveFence> = None;
    let mut in_user_block = false;
    let mut pending_assistant_accent = false;
    let accent_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let mut idx = 0usize;

    while idx < raw_lines.len() {
        let line = raw_lines[idx];
        let trimmed = line.trim();

        if let Some(ref mut fence) = active_fence {
            if is_closing_fence_for(trimmed, fence.fence_char, fence.fence_len) {
                if !fence.lang.eq_ignore_ascii_case("ansi") {
                    out.push(Line::from(Span::styled(
                        "╰───".to_string(),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
                active_fence = None;
            } else if fence.lang.eq_ignore_ascii_case("ansi") {
                out.push(render_ansi_line(line, &mut fence.ansi_style));
            } else {
                out.push(crate::syntax::highlight_code_line(
                    &fence.lang,
                    line,
                    &mut fence.in_block_comment,
                ));
            }
            idx += 1;
            continue;
        }

        if let Some((fence_char, fence_len, lang)) = parse_opening_fence(line) {
            in_user_block = false;
            flush_pending_accent(&mut out, &mut pending_assistant_accent, accent_style);
            if !lang.eq_ignore_ascii_case("ansi") {
                let header = if lang.is_empty() {
                    "╭───".to_string()
                } else {
                    format!("╭─ {lang} ─")
                };
                out.push(Line::from(Span::styled(
                    header,
                    Style::default().fg(Color::DarkGray),
                )));
            }
            active_fence = Some(ActiveFence {
                fence_char,
                fence_len,
                lang,
                in_block_comment: false,
                ansi_style: Style::default(),
            });
            idx += 1;
            continue;
        }

        if let Some(user_prompt) = trimmed
            .strip_prefix("# User: ")
            .or_else(|| (trimmed == "# User:").then_some(""))
        {
            flush_pending_accent(&mut out, &mut pending_assistant_accent, accent_style);
            in_user_block = true;
            let user_style = Style::default().fg(Color::Green);
            let mut spans = vec![Span::styled("❯ ", user_style.add_modifier(Modifier::BOLD))];
            if !user_prompt.is_empty() {
                spans.extend(parse_inline_spans_with_base(user_prompt, user_style));
            }
            out.push(Line::from(spans));
            idx += 1;
            continue;
        }

        if trimmed == "## Assistant" || trimmed == "## Agent (agy)" {
            in_user_block = false;
            flush_pending_accent(&mut out, &mut pending_assistant_accent, accent_style);
            pending_assistant_accent = true;
            idx += 1;
            continue;
        }

        if in_user_block {
            if trimmed.is_empty() || trimmed.starts_with('#') {
                in_user_block = false;
            } else {
                let user_style = Style::default().fg(Color::Green);
                let mut spans = vec![Span::styled("  ", user_style)];
                spans.extend(parse_inline_spans_with_base(line, user_style));
                out.push(Line::from(spans));
                idx += 1;
                continue;
            }
        }

        if pending_assistant_accent && trimmed.is_empty() {
            idx += 1;
            continue;
        }

        if idx + 1 < raw_lines.len() {
            if let Some((table, next_idx)) = try_parse_table(&raw_lines, idx) {
                flush_pending_accent(&mut out, &mut pending_assistant_accent, accent_style);
                out.extend(render_table(&table, max_width));
                idx = next_idx;
                continue;
            }
        }

        if let Some((cmd, exit_code)) = parse_shell_header_line(trimmed) {
            let exit_color = if exit_code == 0 {
                Color::Green
            } else {
                Color::Red
            };
            let rendered_line = Line::from(vec![
                Span::styled(
                    format!("$ {cmd} "),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("(exit {exit_code})"),
                    Style::default().fg(exit_color),
                ),
            ]);
            push_with_pending_accent(
                &mut out,
                rendered_line,
                &mut pending_assistant_accent,
                accent_style,
            );
        } else if let Some(heading_text) = strip_heading_prefix(trimmed) {
            let heading_style = Style::default().add_modifier(Modifier::BOLD);
            let spans = parse_inline_spans_with_base(heading_text, heading_style);
            let rendered_line = Line::from(if spans.is_empty() {
                vec![Span::styled(String::new(), heading_style)]
            } else {
                spans
            });
            push_with_pending_accent(
                &mut out,
                rendered_line,
                &mut pending_assistant_accent,
                accent_style,
            );
        } else if is_horizontal_rule(trimmed) {
            flush_pending_accent(&mut out, &mut pending_assistant_accent, accent_style);
            let rule_len = match max_width {
                Some(w) => w.clamp(3, 60),
                None => 32,
            };
            out.push(Line::from(Span::styled(
                "─".repeat(rule_len),
                Style::default().fg(Color::DarkGray),
            )));
        } else if let Some(quote) = strip_blockquote_prefix(trimmed) {
            let quote_base = Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC);
            let mut spans = vec![Span::styled("│ ", Style::default().fg(Color::DarkGray))];
            spans.extend(parse_inline_spans_with_base(quote, quote_base));
            push_with_pending_accent(
                &mut out,
                Line::from(spans),
                &mut pending_assistant_accent,
                accent_style,
            );
        } else if let Some((indent, bullet)) = split_list_bullet(line) {
            let mut spans = vec![Span::raw(format!("{indent}• "))];
            spans.extend(parse_inline_spans(bullet));
            push_with_pending_accent(
                &mut out,
                Line::from(spans),
                &mut pending_assistant_accent,
                accent_style,
            );
        } else if trimmed.is_empty() {
            out.push(Line::raw(""));
        } else {
            push_with_pending_accent(
                &mut out,
                Line::from(parse_inline_spans(line)),
                &mut pending_assistant_accent,
                accent_style,
            );
        }

        idx += 1;
    }

    flush_pending_accent(&mut out, &mut pending_assistant_accent, accent_style);

    if out.is_empty() && !markdown.is_empty() {
        out.push(Line::raw(markdown.to_string()));
    }
    out
}

pub(super) fn parse_opening_fence(line: &str) -> Option<(char, usize, String)> {
    let trimmed_start = line.trim_start();
    let first = trimmed_start.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let fence_len = trimmed_start.bytes().take_while(|&b| b == first as u8).count();
    if fence_len < 3 {
        return None;
    }
    let info = trimmed_start[fence_len..].trim();
    if first == '`' && info.contains('`') {
        return None;
    }
    let lang = info.split_whitespace().next().unwrap_or("").to_string();
    Some((first, fence_len, lang))
}

fn is_closing_fence_for(trimmed: &str, fence_char: char, min_len: usize) -> bool {
    let count = trimmed.bytes().take_while(|&b| b == fence_char as u8).count();
    count >= min_len && trimmed[count..].trim().is_empty()
}

fn parse_shell_header_line(trimmed: &str) -> Option<(&str, i32)> {
    let rest = trimmed.strip_prefix("$ ")?;
    let inner = rest.strip_suffix(')')?;
    let (cmd, code_str) = inner.rsplit_once(" (exit ")?;
    if cmd.trim().is_empty() || code_str.starts_with('+') {
        return None;
    }
    let exit_code = code_str.parse::<i32>().ok()?;
    Some((cmd, exit_code))
}

pub(super) fn strip_heading_prefix(trimmed: &str) -> Option<&str> {
    let hash_count = trimmed.bytes().take_while(|&b| b == b'#').count();
    if (1..=6).contains(&hash_count) {
        let rest = &trimmed[hash_count..];
        if let Some(content) = rest.strip_prefix(' ') {
            return Some(content.trim_start_matches(' '));
        }
    }
    None
}

pub(super) fn strip_blockquote_prefix(trimmed: &str) -> Option<&str> {
    if trimmed == ">" {
        Some("")
    } else {
        trimmed.strip_prefix("> ")
    }
}

fn split_list_bullet(line: &str) -> Option<(String, &str)> {
    let indent_len = line.len() - line.trim_start().len();
    let indent_raw = &line[..indent_len];
    let rest = &line[indent_len..];
    let bullet = rest.strip_prefix("- ").or_else(|| rest.strip_prefix("* "))?;
    let indent = indent_raw.replace('\t', "  ");
    Some((indent, bullet))
}

pub(super) fn is_horizontal_rule(trimmed: &str) -> bool {
    let mut marker: Option<char> = None;
    let mut count = 0usize;
    for ch in trimmed.chars() {
        if ch == ' ' || ch == '\t' {
            continue;
        }
        if !matches!(ch, '-' | '*' | '_') {
            return false;
        }
        match marker {
            None => {
                marker = Some(ch);
                count = 1;
            }
            Some(m) if m == ch => count += 1,
            Some(_) => return false,
        }
    }
    count >= 3
}
