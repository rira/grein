use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Alignment {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone)]
struct TableData {
    alignments: Vec<Alignment>,
    header: Vec<String>,
    rows: Vec<Vec<String>>,
}

#[derive(Debug, Clone)]
enum CellSegment {
    Word(Vec<Span<'static>>, usize),
    Space(Vec<Span<'static>>, usize),
}

pub fn render_markdown_lines(markdown: &str) -> Vec<Line<'static>> {
    render_markdown_internal(markdown, None)
}

pub fn render_markdown_lines_with_width(markdown: &str, max_width: usize) -> Vec<Line<'static>> {
    render_markdown_internal(markdown, Some(max_width))
}

pub fn parse_inline_spans(text: &str) -> Vec<Span<'static>> {
    parse_inline_spans_with_base(text, Style::default())
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

fn parse_opening_fence(line: &str) -> Option<(char, usize, String)> {
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

pub fn render_ansi_text(text: &str) -> Vec<Line<'static>> {
    let mut style = Style::default();
    text.lines()
        .map(|line| render_ansi_line(line, &mut style))
        .collect()
}

fn render_ansi_line(line: &str, style: &mut Style) -> Line<'static> {
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

fn strip_heading_prefix(trimmed: &str) -> Option<&str> {
    let hash_count = trimmed.bytes().take_while(|&b| b == b'#').count();
    if (1..=6).contains(&hash_count) {
        let rest = &trimmed[hash_count..];
        if let Some(content) = rest.strip_prefix(' ') {
            return Some(content.trim_start_matches(' '));
        }
    }
    None
}

fn strip_blockquote_prefix(trimmed: &str) -> Option<&str> {
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

fn is_horizontal_rule(trimmed: &str) -> bool {
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

fn try_parse_table(raw_lines: &[&str], start: usize) -> Option<(TableData, usize)> {
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

fn render_table(table: &TableData, max_width: Option<usize>) -> Vec<Line<'static>> {
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
        .map(|row| {
            row.iter()
                .map(|cell| parse_inline_spans(cell))
                .collect()
        })
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

fn str_display_width(s: &str) -> usize {
    if s.is_ascii() {
        s.bytes().filter(|b| !b.is_ascii_control()).count()
    } else {
        Span::raw(s).width()
    }
}

fn spans_display_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| str_display_width(s.content.as_ref())).sum()
}

fn push_merged_span(target: &mut Vec<Span<'static>>, text: &str, style: Style) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = target.last_mut() {
        if last.style == style {
            last.content.to_mut().push_str(text);
            return;
        }
    }
    target.push(Span::styled(text.to_string(), style));
}

fn push_merged_char(target: &mut Vec<Span<'static>>, ch: char, style: Style) {
    if let Some(last) = target.last_mut() {
        if last.style == style {
            last.content.to_mut().push(ch);
            return;
        }
    }
    target.push(Span::styled(ch.to_string(), style));
}

fn tokenize_spans(spans: Vec<Span<'static>>, normalize_ws: bool) -> Vec<CellSegment> {
    let mut segments: Vec<CellSegment> = Vec::new();
    for span in spans {
        let style = span.style;
        let text = span.content.as_ref();
        let mut cur_start = 0usize;
        let mut cur_is_ws: Option<bool> = None;

        for (byte_idx, ch) in text.char_indices() {
            let is_ws = ch.is_whitespace();
            match cur_is_ws {
                None => {
                    cur_is_ws = Some(is_ws);
                    cur_start = byte_idx;
                }
                Some(prev_ws) if prev_ws != is_ws => {
                    append_token_slice(
                        &mut segments,
                        &text[cur_start..byte_idx],
                        prev_ws,
                        style,
                        normalize_ws,
                    );
                    cur_is_ws = Some(is_ws);
                    cur_start = byte_idx;
                }
                Some(_) => {}
            }
        }
        if let Some(prev_ws) = cur_is_ws {
            append_token_slice(
                &mut segments,
                &text[cur_start..],
                prev_ws,
                style,
                normalize_ws,
            );
        }
    }
    segments
}

fn append_token_slice(
    segments: &mut Vec<CellSegment>,
    slice: &str,
    is_ws: bool,
    style: Style,
    normalize_ws: bool,
) {
    if slice.is_empty() {
        return;
    }
    if is_ws {
        let text = if normalize_ws {
            slice.replace(['\t', '\n', '\r'], " ")
        } else {
            slice.to_string()
        };
        let w = str_display_width(&text);
        if let Some(CellSegment::Space(spans, total_w)) = segments.last_mut() {
            push_merged_span(spans, &text, style);
            *total_w += w;
        } else {
            segments.push(CellSegment::Space(vec![Span::styled(text, style)], w));
        }
    } else {
        let w = str_display_width(slice);
        if let Some(CellSegment::Word(spans, total_w)) = segments.last_mut() {
            push_merged_span(spans, slice, style);
            *total_w += w;
        } else {
            segments.push(CellSegment::Word(
                vec![Span::styled(slice.to_string(), style)],
                w,
            ));
        }
    }
}

fn push_spans_char_wrapped(
    out: &mut Vec<Vec<Span<'static>>>,
    cur_line: &mut Vec<Span<'static>>,
    cur_w: &mut usize,
    spans: Vec<Span<'static>>,
    max_width: usize,
) {
    for sp in spans {
        for ch in sp.content.chars() {
            let cw = char_display_width(ch);
            if *cur_w > 0 && *cur_w + cw > max_width {
                out.push(std::mem::take(cur_line));
                *cur_w = 0;
            }
            push_merged_char(cur_line, ch, sp.style);
            *cur_w += cw;
        }
    }
}

fn wrap_spans_to_width(spans: Vec<Span<'static>>, col_width: usize) -> Vec<Vec<Span<'static>>> {
    let max_w = col_width.max(1);
    let segments = tokenize_spans(spans, true);
    let mut lines: Vec<Vec<Span<'static>>> = Vec::new();
    let mut cur_line: Vec<Span<'static>> = Vec::new();
    let mut cur_w = 0usize;
    let mut pending_space: Option<(Vec<Span<'static>>, usize)> = None;

    for seg in segments {
        match seg {
            CellSegment::Space(sp_spans, sp_w) => {
                if cur_w > 0 {
                    match &mut pending_space {
                        Some((existing, existing_w)) => {
                            for sp in sp_spans {
                                push_merged_span(existing, sp.content.as_ref(), sp.style);
                            }
                            *existing_w += sp_w;
                        }
                        None => pending_space = Some((sp_spans, sp_w)),
                    }
                }
            }
            CellSegment::Word(word_spans, word_w) => {
                let space_w = if cur_w > 0 {
                    pending_space.as_ref().map_or(0, |(_, w)| *w)
                } else {
                    0
                };

                if cur_w > 0 && cur_w + space_w + word_w <= max_w {
                    if let Some((sp_spans, _)) = pending_space.take() {
                        for sp in sp_spans {
                            push_merged_span(&mut cur_line, sp.content.as_ref(), sp.style);
                        }
                    }
                    for sp in word_spans {
                        push_merged_span(&mut cur_line, sp.content.as_ref(), sp.style);
                    }
                    cur_w += space_w + word_w;
                } else {
                    if cur_w > 0 {
                        lines.push(std::mem::take(&mut cur_line));
                        cur_w = 0;
                    }
                    pending_space = None;

                    if word_w <= max_w {
                        for sp in word_spans {
                            push_merged_span(&mut cur_line, sp.content.as_ref(), sp.style);
                        }
                        cur_w = word_w;
                    } else {
                        push_spans_char_wrapped(
                            &mut lines,
                            &mut cur_line,
                            &mut cur_w,
                            word_spans,
                            max_w,
                        );
                    }
                }
            }
        }
    }

    if !cur_line.is_empty() || lines.is_empty() {
        lines.push(cur_line);
    }

    lines
}

pub fn wrap_styled_lines(lines: Vec<Line<'static>>, max_width: usize) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .flat_map(|line| wrap_styled_line(line, max_width))
        .collect()
}

pub fn wrap_styled_line(line: Line<'static>, max_width: usize) -> Vec<Line<'static>> {
    if max_width == 0 {
        return vec![line];
    }

    let line_style = line.style;
    let line_alignment = line.alignment;
    let segments = tokenize_spans(line.spans, false);
    let mut out: Vec<Vec<Span<'static>>> = Vec::new();
    let mut cur_line: Vec<Span<'static>> = Vec::new();
    let mut cur_w = 0usize;
    let mut has_word_on_line = false;
    let mut pending_ws: Option<(Vec<Span<'static>>, usize)> = None;

    for seg in segments {
        match seg {
            CellSegment::Space(sp_spans, sp_w) => {
                if has_word_on_line {
                    match &mut pending_ws {
                        Some((existing, existing_w)) => {
                            for sp in sp_spans {
                                push_merged_span(existing, sp.content.as_ref(), sp.style);
                            }
                            *existing_w += sp_w;
                        }
                        None => pending_ws = Some((sp_spans, sp_w)),
                    }
                } else {
                    push_spans_char_wrapped(&mut out, &mut cur_line, &mut cur_w, sp_spans, max_width);
                }
            }
            CellSegment::Word(word_spans, word_w) => {
                let ws_w = if has_word_on_line {
                    pending_ws.as_ref().map_or(0, |(_, w)| *w)
                } else {
                    0
                };

                if cur_w + ws_w + word_w <= max_width {
                    if let Some((sp_spans, _)) = pending_ws.take() {
                        for sp in sp_spans {
                            push_merged_span(&mut cur_line, sp.content.as_ref(), sp.style);
                        }
                    }
                    for sp in word_spans {
                        push_merged_span(&mut cur_line, sp.content.as_ref(), sp.style);
                    }
                    cur_w += ws_w + word_w;
                } else {
                    pending_ws = None;
                    if cur_w > 0 && (has_word_on_line || word_w <= max_width) {
                        out.push(std::mem::take(&mut cur_line));
                        cur_w = 0;
                    }

                    if word_w <= max_width {
                        for sp in word_spans {
                            push_merged_span(&mut cur_line, sp.content.as_ref(), sp.style);
                        }
                        cur_w = word_w;
                    } else {
                        push_spans_char_wrapped(
                            &mut out,
                            &mut cur_line,
                            &mut cur_w,
                            word_spans,
                            max_width,
                        );
                    }
                }
                has_word_on_line = true;
            }
        }
    }

    if let Some((sp_spans, sp_w)) = pending_ws.take() {
        if cur_w + sp_w <= max_width {
            for sp in sp_spans {
                push_merged_span(&mut cur_line, sp.content.as_ref(), sp.style);
            }
        }
    }

    if !cur_line.is_empty() || out.is_empty() {
        out.push(cur_line);
    }

    out.into_iter()
        .map(|spans| {
            let mut l = Line::from(spans).style(line_style);
            l.alignment = line_alignment;
            l
        })
        .collect()
}

fn parse_inline_spans_with_base(text: &str, base: Style) -> Vec<Span<'static>> {
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
