use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::TranscriptBlock;

pub(super) fn trim_trailing_newlines(text: &str) -> &str {
    text.trim_end_matches(['\r', '\n'])
}

pub(super) fn starts_with_code_fence(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with("```") || trimmed.starts_with("~~~")
}

pub(super) fn trimmed_line_count(text: &str) -> usize {
    let clean = trim_trailing_newlines(text);
    if clean.is_empty() {
        0
    } else {
        clean.lines().count()
    }
}

pub(super) fn shell_output_line_count(stdout: &str, stderr: &str) -> usize {
    trimmed_line_count(stdout) + trimmed_line_count(stderr)
}

pub(super) fn clean_summary(summary: Option<&str>) -> Option<&str> {
    summary.map(str::trim).filter(|s| !s.is_empty())
}

fn format_line_count_label(count: usize) -> String {
    if count == 1 {
        "1 line".to_string()
    } else {
        format!("{count} lines")
    }
}

pub(super) fn render_block(block: &TranscriptBlock, width: usize, out: &mut Vec<Line<'static>>) {
    match block {
        TranscriptBlock::UserTurn { content } => {
            let user_style = Style::default().fg(Color::Green);
            let clean = trim_trailing_newlines(content);
            let mut iter = clean.split('\n');
            let first = iter.next().unwrap_or("").trim_end_matches('\r');
            let mut first_spans =
                vec![Span::styled("❯ ", user_style.add_modifier(Modifier::BOLD))];
            if !first.is_empty() {
                first_spans.push(Span::styled(first.to_string(), user_style));
            }
            out.push(Line::from(first_spans));
            for cont in iter {
                let cont = cont.trim_end_matches('\r');
                let mut spans = vec![Span::styled("  ", user_style)];
                if !cont.is_empty() {
                    spans.push(Span::styled(cont.to_string(), user_style));
                }
                out.push(Line::from(spans));
            }
        }
        TranscriptBlock::AssistantMarkdown { content } => {
            let accent_style = Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD);
            let body = crate::markdown::render_markdown_lines_with_width(content, width);
            if content.trim().is_empty() || body.is_empty() {
                out.push(Line::from(Span::styled("✦", accent_style)));
                return;
            }
            if starts_with_code_fence(content) {
                out.push(Line::from(Span::styled("✦", accent_style)));
                out.extend(body);
            } else {
                let mut iter = body.into_iter();
                if let Some(mut first) = iter.next() {
                    let mut spans = Vec::with_capacity(first.spans.len() + 1);
                    spans.push(Span::styled("✦ ", accent_style));
                    spans.extend(first.spans);
                    first.spans = spans;
                    if width > 0 {
                        out.extend(crate::markdown::wrap_styled_lines(vec![first], width));
                    } else {
                        out.push(first);
                    }
                }
                out.extend(iter);
            }
        }
        TranscriptBlock::ToolCall {
            tool_name,
            summary,
            output,
            completed,
            is_error,
            collapsed,
            ..
        } => render_tool_call(
            tool_name,
            summary.as_deref(),
            output,
            *completed,
            *is_error,
            *collapsed,
            out,
        ),
        TranscriptBlock::ShellExecution {
            command,
            stdout,
            stderr,
            exit_code,
            collapsed,
        } => render_shell_execution(command, stdout, stderr, *exit_code, *collapsed, out),
        TranscriptBlock::JoinNote { text } => {
            let style = Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC);
            for (idx, line) in trim_trailing_newlines(text).split('\n').enumerate() {
                let prefix = if idx == 0 { "↳ " } else { "  " };
                out.push(Line::from(Span::styled(
                    format!("{prefix}{}", line.trim_end_matches('\r')),
                    style,
                )));
            }
        }
        TranscriptBlock::ErrorNotice { message } => {
            let style = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);
            for (idx, line) in trim_trailing_newlines(message).split('\n').enumerate() {
                let prefix = if idx == 0 { "✖ Error: " } else { "         " };
                out.push(Line::from(Span::styled(
                    format!("{prefix}{}", line.trim_end_matches('\r')),
                    style,
                )));
            }
        }
    }
}

fn render_tool_call(
    tool_name: &str,
    summary: Option<&str>,
    output: &str,
    completed: bool,
    is_error: bool,
    collapsed: bool,
    out: &mut Vec<Line<'static>>,
) {
    let summary = clean_summary(summary);
    if !completed {
        let mut spans = vec![Span::styled(
            format!("⚙ {tool_name}"),
            Style::default().fg(Color::Yellow),
        )];
        if let Some(sum) = summary {
            spans.push(Span::styled(
                format!(" ({sum})"),
                Style::default().fg(Color::DarkGray),
            ));
        }
        out.push(Line::from(spans));
        return;
    }

    let (status_glyph, status_color) = if is_error {
        ("✖", Color::Red)
    } else {
        ("✓", Color::Green)
    };
    let line_count = trimmed_line_count(output);

    if collapsed {
        let mut spans = vec![
            Span::styled("▶ ", Style::default().fg(Color::DarkGray)),
            Span::styled(status_glyph, Style::default().fg(status_color)),
            Span::raw(" "),
            Span::styled(
                tool_name.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ];
        if let Some(sum) = summary {
            spans.push(Span::styled(
                format!(" ({sum})"),
                Style::default().fg(Color::DarkGray),
            ));
        }
        if line_count > 0 {
            spans.push(Span::styled(
                format!(" · {}  [Ctrl+O]", format_line_count_label(line_count)),
                Style::default().fg(Color::DarkGray),
            ));
        }
        out.push(Line::from(spans));
        return;
    }

    let mut header_spans = vec![
        Span::styled("▼ ", Style::default().fg(Color::DarkGray)),
        Span::styled(status_glyph, Style::default().fg(status_color)),
        Span::raw(" "),
        Span::styled(
            tool_name.to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(sum) = summary {
        header_spans.push(Span::styled(
            format!(" ({sum})"),
            Style::default().fg(Color::DarkGray),
        ));
    }
    out.push(Line::from(header_spans));

    let clean_output = trim_trailing_newlines(output);
    if clean_output.is_empty() {
        return;
    }

    let gutter_style = Style::default().fg(Color::DarkGray);
    if looks_like_unified_diff(clean_output) {
        for body_line in clean_output.lines() {
            out.push(Line::from(vec![
                Span::styled("│ ", gutter_style),
                style_diff_line(body_line),
            ]));
        }
    } else {
        for ansi_line in crate::markdown::render_ansi_text(clean_output) {
            let mut spans = Vec::with_capacity(ansi_line.spans.len() + 1);
            spans.push(Span::styled("│ ", gutter_style));
            spans.extend(ansi_line.spans);
            out.push(Line::from(spans));
        }
    }
}

fn looks_like_unified_diff(output: &str) -> bool {
    let mut has_minus_hdr = false;
    let mut has_plus_hdr = false;
    for line in output.lines() {
        if line.starts_with("diff --git ") {
            return true;
        }
        if line.starts_with("@@ ") && line[3..].contains("@@") {
            return true;
        }
        if line.starts_with("--- ") {
            has_minus_hdr = true;
        } else if line.starts_with("+++ ") {
            has_plus_hdr = true;
        }
        if has_minus_hdr && has_plus_hdr {
            return true;
        }
    }
    false
}

fn style_diff_line(line: &str) -> Span<'static> {
    if line.starts_with("@@") {
        Span::styled(line.to_string(), Style::default().fg(Color::Cyan))
    } else if line.starts_with("diff --git ")
        || line.starts_with("index ")
        || line.starts_with("--- ")
        || line.starts_with("+++ ")
    {
        Span::styled(
            line.to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    } else if line.starts_with('+') {
        Span::styled(line.to_string(), Style::default().fg(Color::Green))
    } else if line.starts_with('-') {
        Span::styled(line.to_string(), Style::default().fg(Color::Red))
    } else {
        Span::raw(line.to_string())
    }
}

fn render_shell_execution(
    command: &str,
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
    collapsed: bool,
    out: &mut Vec<Line<'static>>,
) {
    let Some(code) = exit_code else {
        out.push(Line::from(Span::styled(
            format!("$ {command}"),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        return;
    };

    let exit_color = if code == 0 { Color::Green } else { Color::Red };
    out.push(Line::from(vec![
        Span::styled(
            format!("$ {command} "),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("(exit {code})"), Style::default().fg(exit_color)),
    ]));

    let total_lines = shell_output_line_count(stdout, stderr);
    if total_lines == 0 {
        return;
    }

    if collapsed {
        out.push(Line::from(vec![
            Span::styled("▶ ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}  [Ctrl+O]", format_line_count_label(total_lines)),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
        return;
    }

    let clean_stdout = trim_trailing_newlines(stdout);
    let clean_stderr = trim_trailing_newlines(stderr);
    let mut combined = String::new();
    if !clean_stdout.is_empty() {
        combined.push_str(clean_stdout);
    }
    if !clean_stderr.is_empty() {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str("[stderr]\n");
        combined.push_str(clean_stderr);
    }

    out.extend(crate::markdown::render_ansi_text(&combined));
}

fn parse_fence_open(line: &str) -> Option<(char, usize)> {
    let leading_spaces = line.bytes().take_while(|&b| b == b' ').count();
    if leading_spaces > 3 {
        return None;
    }
    let rest = &line[leading_spaces..];
    let fence_ch = rest.chars().next()?;
    if fence_ch != '`' && fence_ch != '~' {
        return None;
    }
    let count = rest.bytes().take_while(|&b| b == fence_ch as u8).count();
    if count < 3 {
        return None;
    }
    let info = &rest[count..];
    if fence_ch == '`' && info.contains('`') {
        return None;
    }
    Some((fence_ch, count))
}

fn is_fence_close(line: &str, fence_ch: char, min_len: usize) -> bool {
    let leading_spaces = line.bytes().take_while(|&b| b == b' ').count();
    if leading_spaces > 3 {
        return false;
    }
    let rest = &line[leading_spaces..];
    let count = rest.bytes().take_while(|&b| b == fence_ch as u8).count();
    count >= min_len && rest[count..].trim().is_empty()
}

pub(crate) fn extract_last_fenced_code_block(markdown: &str, last_code: &mut Option<String>) {
    let mut active_fence: Option<(char, usize, Vec<&str>)> = None;
    for line in markdown.lines() {
        match &mut active_fence {
            Some((fence_ch, fence_len, body_lines)) => {
                if is_fence_close(line, *fence_ch, *fence_len) {
                    *last_code = Some(body_lines.join("\n"));
                    active_fence = None;
                } else {
                    body_lines.push(line);
                }
            }
            None => {
                if let Some((fence_ch, fence_len)) = parse_fence_open(line) {
                    active_fence = Some((fence_ch, fence_len, Vec::new()));
                }
            }
        }
    }
}

pub(super) fn strip_ansi_sequences(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut plain_start = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        match bytes[i] {
            0x1B => {
                out.push_str(&input[plain_start..i]);
                i = skip_escape_sequence(bytes, i);
                plain_start = i;
            }
            b'\r' => {
                out.push_str(&input[plain_start..i]);
                i += 1;
                plain_start = i;
            }
            _ => i += 1,
        }
    }
    out.push_str(&input[plain_start..]);
    out
}

fn skip_escape_sequence(bytes: &[u8], esc_idx: usize) -> usize {
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
                bytes.len()
            } else if (0x40..=0x7E).contains(&bytes[j]) {
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
