use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lang {
    Rust,
    JsTs,
    Python,
    Shell,
    Json,
    Toml,
    Yaml,
    Nix,
    Sql,
    Go,
    CFamily,
    Diff,
    Plain,
}

fn normalize_lang(lang: &str) -> Lang {
    let norm = lang
        .trim()
        .split(|c: char| c.is_whitespace() || c == ',')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match norm.as_str() {
        "rust" | "rs" => Lang::Rust,
        "ts" | "tsx" | "js" | "jsx" | "typescript" | "javascript" => Lang::JsTs,
        "py" | "python" => Lang::Python,
        "sh" | "bash" | "zsh" | "shell" => Lang::Shell,
        "json" | "jsonc" => Lang::Json,
        "toml" => Lang::Toml,
        "yaml" | "yml" => Lang::Yaml,
        "nix" => Lang::Nix,
        "sql" => Lang::Sql,
        "go" => Lang::Go,
        "c" | "cpp" | "cc" | "cxx" | "c++" | "h" | "hpp" => Lang::CFamily,
        "diff" | "patch" => Lang::Diff,
        _ => Lang::Plain,
    }
}

fn kw_style() -> Style {
    Style::default()
        .fg(Color::Magenta)
        .add_modifier(Modifier::BOLD)
}

fn type_style() -> Style {
    Style::default().fg(Color::Cyan)
}

fn string_style() -> Style {
    Style::default().fg(Color::Green)
}

fn number_style() -> Style {
    Style::default().fg(Color::LightYellow)
}

fn comment_style() -> Style {
    Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::ITALIC)
}

fn attr_style() -> Style {
    Style::default().fg(Color::LightBlue)
}

fn plain_style() -> Style {
    Style::default().fg(Color::Reset)
}

fn push_span(spans: &mut Vec<Span<'static>>, text: &str, style: Style) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = spans.last_mut() {
        if last.style == style {
            last.content.to_mut().push_str(text);
            return;
        }
    }
    spans.push(Span::styled(text.to_string(), style));
}

pub fn safe_fence_for(content: &str) -> String {
    let mut max_run = 0usize;
    let mut cur_run = 0usize;
    for b in content.bytes() {
        if b == b'`' {
            cur_run += 1;
            if cur_run > max_run {
                max_run = cur_run;
            }
        } else {
            cur_run = 0;
        }
    }
    "`".repeat(max_run.max(2) + 1)
}

pub fn highlight_code_lines(lang: &str, code: &str) -> Vec<Line<'static>> {
    let mut in_block_comment = false;
    code.lines()
        .map(|line| highlight_code_line(lang, line, &mut in_block_comment))
        .collect()
}

pub fn highlight_code_line(
    lang: &str,
    line: &str,
    in_block_comment: &mut bool,
) -> Line<'static> {
    let kind = normalize_lang(lang);

    if kind == Lang::Diff {
        return highlight_diff_line(line);
    }

    if kind == Lang::Plain {
        return Line::from(Span::styled(line.to_string(), plain_style()));
    }

    if line.is_empty() {
        let style = if *in_block_comment && supports_block_comments(kind) {
            comment_style()
        } else {
            plain_style()
        };
        return Line::from(Span::styled(String::new(), style));
    }

    let mut spans = Vec::new();
    let mut i = 0usize;

    if *in_block_comment && supports_block_comments(kind) {
        if let Some(end_rel) = line.find("*/") {
            let end = end_rel + 2;
            push_span(&mut spans, &line[..end], comment_style());
            *in_block_comment = false;
            i = end;
        } else {
            push_span(&mut spans, line, comment_style());
            return Line::from(spans);
        }
    }

    let bytes = line.as_bytes();
    while i < line.len() {
        if supports_block_comments(kind) && line[i..].starts_with("/*") {
            if let Some(close_rel) = line[i + 2..].find("*/") {
                let end = i + 2 + close_rel + 2;
                push_span(&mut spans, &line[i..end], comment_style());
                i = end;
            } else {
                push_span(&mut spans, &line[i..], comment_style());
                *in_block_comment = true;
                break;
            }
            continue;
        }

        if is_line_comment_start(kind, line, i) {
            push_span(&mut spans, &line[i..], comment_style());
            break;
        }

        let ch = line[i..].chars().next().unwrap();

        if kind == Lang::Rust && (line[i..].starts_with("#![") || line[i..].starts_with("#[")) {
            let end = scan_rust_attribute(line, i);
            push_span(&mut spans, &line[i..end], attr_style());
            i = end;
            continue;
        }

        if kind == Lang::CFamily && ch == '#' && line[..i].trim().is_empty() {
            let end = scan_c_directive_end(line, i);
            push_span(&mut spans, &line[i..end], attr_style());
            i = end;
            if line[..end].trim_end().ends_with("include") {
                let ws_len = line[i..].len() - line[i..].trim_start().len();
                if ws_len > 0 {
                    push_span(&mut spans, &line[i..i + ws_len], plain_style());
                    i += ws_len;
                }
                if line[i..].starts_with('<') {
                    if let Some(rel_gt) = line[i + 1..].find('>') {
                        let inc_end = i + 1 + rel_gt + 1;
                        push_span(&mut spans, &line[i..inc_end], string_style());
                        i = inc_end;
                    }
                }
            }
            continue;
        }

        if kind == Lang::Toml && ch == '[' && line[..i].trim().is_empty() {
            if let Some(end) = scan_toml_section(line, i) {
                push_span(&mut spans, &line[i..end], attr_style());
                i = end;
                continue;
            }
        }

        if matches!(kind, Lang::Python | Lang::JsTs) && ch == '@' {
            if let Some(end) = scan_decorator(line, i) {
                push_span(&mut spans, &line[i..end], attr_style());
                i = end;
                continue;
            }
        }

        if kind == Lang::Shell && ch == '$' {
            if let Some(end) = scan_shell_var(line, i) {
                push_span(&mut spans, &line[i..end], type_style());
                i = end;
                continue;
            }
        }

        if kind == Lang::Rust {
            if let Some(end) = scan_rust_prefixed_string(line, i) {
                push_span(&mut spans, &line[i..end], string_style());
                i = end;
                continue;
            }
        }

        if kind == Lang::Python {
            if let Some((end, style)) = scan_python_prefixed_string(line, i) {
                push_span(&mut spans, &line[i..end], style);
                i = end;
                continue;
            }
        }

        if ch == '"' {
            let end = if matches!(kind, Lang::Python | Lang::Toml) && line[i..].starts_with("\"\"\"")
            {
                scan_triple_quoted(line, i, "\"\"\"", kind != Lang::Toml)
            } else {
                scan_quoted(line, i, '"', true)
            };
            let style = if (kind == Lang::Json && is_json_key_suffix(&line[end..]))
                || (kind == Lang::Toml && is_toml_key_context(line, i, end))
                || (kind == Lang::Yaml && is_yaml_key_context(line, i, end))
            {
                attr_style()
            } else {
                string_style()
            };
            push_span(&mut spans, &line[i..end], style);
            i = end;
            continue;
        }

        if ch == '\'' {
            match kind {
                Lang::Rust => {
                    if let Some(end) = scan_rust_char_literal(line, i) {
                        push_span(&mut spans, &line[i..end], string_style());
                        i = end;
                        continue;
                    }
                    let end = scan_rust_lifetime_end(line, i);
                    push_span(&mut spans, &line[i..end], plain_style());
                    i = end;
                    continue;
                }
                Lang::Nix if line[i..].starts_with("''") => {
                    let end = scan_nix_indented_string(line, i);
                    push_span(&mut spans, &line[i..end], string_style());
                    i = end;
                    continue;
                }
                Lang::JsTs | Lang::Python | Lang::Shell | Lang::Go | Lang::CFamily | Lang::Yaml => {
                    let end = if kind == Lang::Python && line[i..].starts_with("'''") {
                        scan_triple_quoted(line, i, "'''", true)
                    } else {
                        scan_quoted(line, i, '\'', true)
                    };
                    let style = if kind == Lang::Yaml && is_yaml_key_context(line, i, end) {
                        attr_style()
                    } else {
                        string_style()
                    };
                    push_span(&mut spans, &line[i..end], style);
                    i = end;
                    continue;
                }
                Lang::Toml | Lang::Sql => {
                    let end = if kind == Lang::Toml && line[i..].starts_with("'''") {
                        scan_triple_quoted(line, i, "'''", false)
                    } else if kind == Lang::Sql {
                        scan_sql_single_quoted(line, i)
                    } else {
                        scan_quoted(line, i, '\'', false)
                    };
                    let style = if kind == Lang::Toml && is_toml_key_context(line, i, end) {
                        attr_style()
                    } else {
                        string_style()
                    };
                    push_span(&mut spans, &line[i..end], style);
                    i = end;
                    continue;
                }
                Lang::Json | Lang::Nix | Lang::Diff | Lang::Plain => {}
            }
        }

        if ch == '`' && matches!(kind, Lang::JsTs | Lang::Go | Lang::Shell) {
            let end = scan_quoted(line, i, '`', kind != Lang::Go);
            push_span(&mut spans, &line[i..end], string_style());
            i = end;
            continue;
        }

        if ch.is_ascii_digit() && is_number_start(line, i) {
            let end = scan_number(bytes, i);
            push_span(&mut spans, &line[i..end], number_style());
            i = end;
            continue;
        }

        if is_ident_start(ch) {
            if kind == Lang::Toml {
                if let Some(end) = scan_toml_bare_key(line, i) {
                    push_span(&mut spans, &line[i..end], attr_style());
                    i = end;
                    continue;
                }
            }
            if kind == Lang::Yaml {
                if let Some(end) = scan_yaml_bare_key(line, i) {
                    push_span(&mut spans, &line[i..end], attr_style());
                    i = end;
                    continue;
                }
            }

            let end = scan_ident(line, i);
            let word = &line[i..end];

            if kind == Lang::Rust && bytes.get(end) == Some(&b'!') && bytes.get(end + 1) != Some(&b'=')
            {
                push_span(&mut spans, &line[i..end + 1], attr_style());
                i = end + 1;
                continue;
            }

            let style = classify_ident(kind, word);
            push_span(&mut spans, word, style);
            i = end;
            continue;
        }

        let next_i = i + ch.len_utf8();
        push_span(&mut spans, &line[i..next_i], plain_style());
        i = next_i;
    }

    if spans.is_empty() {
        Line::from(Span::styled(String::new(), plain_style()))
    } else {
        Line::from(spans)
    }
}

fn highlight_diff_line(line: &str) -> Line<'static> {
    let style = if line.starts_with("+++")
        || line.starts_with("---")
        || line.starts_with("diff --git")
        || line.starts_with("index ")
    {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else if line.starts_with("@@") {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else if line.starts_with('+') {
        Style::default().fg(Color::Green)
    } else if line.starts_with('-') {
        Style::default().fg(Color::Red)
    } else {
        plain_style()
    };
    Line::from(Span::styled(line.to_string(), style))
}

fn supports_block_comments(kind: Lang) -> bool {
    matches!(
        kind,
        Lang::Rust | Lang::JsTs | Lang::Json | Lang::Nix | Lang::Sql | Lang::Go | Lang::CFamily
    )
}

fn is_line_comment_start(kind: Lang, line: &str, i: usize) -> bool {
    let rest = &line[i..];
    match kind {
        Lang::Rust | Lang::JsTs | Lang::Json | Lang::Go | Lang::CFamily => rest.starts_with("//"),
        Lang::Python | Lang::Toml | Lang::Nix => rest.starts_with('#'),
        Lang::Shell | Lang::Yaml => {
            if !rest.starts_with('#') {
                return false;
            }
            if i == 0 {
                return true;
            }
            let prev = line[..i].chars().next_back().unwrap_or(' ');
            prev.is_whitespace()
                || (kind == Lang::Shell && matches!(prev, ';' | '|' | '&' | '(' | ')'))
        }
        Lang::Sql => rest.starts_with("--"),
        Lang::Diff | Lang::Plain => false,
    }
}

fn scan_rust_attribute(line: &str, start: usize) -> usize {
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    for (rel, ch) in line[start..].char_indices() {
        if in_str {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_str = false;
            }
            continue;
        }
        match ch {
            '"' => in_str = true,
            '[' => depth += 1,
            ']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return start + rel + 1;
                }
            }
            _ => {}
        }
    }
    line.len()
}

fn scan_c_directive_end(line: &str, start: usize) -> usize {
    let mut j = start + 1;
    while j < line.len() && line.as_bytes()[j].is_ascii_whitespace() {
        j += 1;
    }
    while j < line.len() && is_ident_cont(line[j..].chars().next().unwrap()) {
        j += 1;
    }
    j
}

fn scan_toml_section(line: &str, start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    for (rel, ch) in line[start..].char_indices() {
        if in_str {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_str = false;
            }
            continue;
        }
        match ch {
            '"' => in_str = true,
            '[' => depth += 1,
            ']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(start + rel + 1);
                }
            }
            '#' if depth == 0 => return None,
            _ => {}
        }
    }
    None
}

fn scan_decorator(line: &str, start: usize) -> Option<usize> {
    let after = &line[start + 1..];
    let first = after.chars().next()?;
    if !is_ident_start(first) {
        return None;
    }
    let mut j = start + 1;
    while j < line.len() {
        let ch = line[j..].chars().next().unwrap();
        if is_ident_cont(ch) || ch == '.' {
            j += ch.len_utf8();
        } else {
            break;
        }
    }
    Some(j)
}

fn scan_shell_var(line: &str, start: usize) -> Option<usize> {
    let rest = &line[start + 1..];
    if rest.starts_with('{') {
        if let Some(rel) = rest.find('}') {
            return Some(start + 1 + rel + 1);
        }
        return Some(line.len());
    }
    let first = rest.chars().next()?;
    if is_ident_start(first) || first.is_ascii_digit() {
        let mut j = start + 1;
        while j < line.len() {
            let ch = line[j..].chars().next().unwrap();
            if is_ident_cont(ch) {
                j += ch.len_utf8();
            } else {
                break;
            }
        }
        Some(j)
    } else if matches!(first, '?' | '@' | '*' | '#' | '$' | '!' | '-') {
        Some(start + 2)
    } else {
        None
    }
}

fn scan_rust_prefixed_string(line: &str, start: usize) -> Option<usize> {
    if start > 0 && is_ident_cont(line[..start].chars().next_back().unwrap_or(' ')) {
        return None;
    }
    let rest = &line[start..];
    if rest.starts_with("b\"") {
        return Some(scan_quoted(line, start + 1, '"', true));
    }
    if rest.starts_with("b'") {
        return scan_rust_char_literal(line, start + 1);
    }
    let prefix_len = if rest.starts_with("br") {
        2
    } else if rest.starts_with('r') {
        1
    } else {
        return None;
    };
    let after_prefix = &rest[prefix_len..];
    let hashes = after_prefix.bytes().take_while(|&b| b == b'#').count();
    if !after_prefix[hashes..].starts_with('"') {
        return None;
    }
    let body_start = start + prefix_len + hashes + 1;
    let closing = format!("\"{}", "#".repeat(hashes));
    if let Some(rel_end) = line[body_start..].find(&closing) {
        Some(body_start + rel_end + closing.len())
    } else {
        Some(line.len())
    }
}

fn scan_python_prefixed_string(line: &str, start: usize) -> Option<(usize, Style)> {
    if start > 0 && is_ident_cont(line[..start].chars().next_back().unwrap_or(' ')) {
        return None;
    }
    let bytes = line.as_bytes();
    let mut prefix_len = 0usize;
    while prefix_len < 2 && start + prefix_len < bytes.len() {
        let b = bytes[start + prefix_len].to_ascii_lowercase();
        if matches!(b, b'f' | b'r' | b'b' | b'u') {
            prefix_len += 1;
        } else {
            break;
        }
    }
    if prefix_len == 0 || start + prefix_len >= line.len() {
        return None;
    }
    let q_idx = start + prefix_len;
    let q = bytes[q_idx];
    if q != b'"' && q != b'\'' {
        return None;
    }
    let end = if q == b'"' && line[q_idx..].starts_with("\"\"\"") {
        scan_triple_quoted(line, q_idx, "\"\"\"", true)
    } else if q == b'\'' && line[q_idx..].starts_with("'''") {
        scan_triple_quoted(line, q_idx, "'''", true)
    } else {
        scan_quoted(line, q_idx, q as char, true)
    };
    Some((end, string_style()))
}

fn scan_quoted(line: &str, start: usize, quote: char, allow_escape: bool) -> usize {
    let mut j = start + quote.len_utf8();
    while j < line.len() {
        let ch = line[j..].chars().next().unwrap();
        j += ch.len_utf8();
        if allow_escape && ch == '\\' {
            if j < line.len() {
                let esc = line[j..].chars().next().unwrap();
                j += esc.len_utf8();
            }
        } else if ch == quote {
            break;
        }
    }
    j
}

fn scan_sql_single_quoted(line: &str, start: usize) -> usize {
    let mut j = start + 1;
    while j < line.len() {
        let ch = line[j..].chars().next().unwrap();
        j += ch.len_utf8();
        if ch == '\'' {
            if line[j..].starts_with('\'') {
                j += 1;
            } else {
                break;
            }
        }
    }
    j
}

fn scan_triple_quoted(line: &str, start: usize, delim: &str, allow_escape: bool) -> usize {
    let mut j = start + delim.len();
    while j < line.len() {
        if line[j..].starts_with(delim) {
            return j + delim.len();
        }
        let ch = line[j..].chars().next().unwrap();
        j += ch.len_utf8();
        if allow_escape && ch == '\\' && j < line.len() {
            let esc = line[j..].chars().next().unwrap();
            j += esc.len_utf8();
        }
    }
    line.len()
}

fn scan_nix_indented_string(line: &str, start: usize) -> usize {
    let mut j = start + 2;
    while j < line.len() {
        if line[j..].starts_with("'''") {
            j += 3;
            continue;
        }
        if line[j..].starts_with("''") {
            return j + 2;
        }
        let ch = line[j..].chars().next().unwrap();
        j += ch.len_utf8();
    }
    line.len()
}

fn scan_rust_char_literal(line: &str, start: usize) -> Option<usize> {
    let after = &line[start + 1..];
    if after.starts_with('\\') {
        let mut j = 1usize;
        if let Some(esc) = after[j..].chars().next() {
            j += esc.len_utf8();
            for _ in 0..12 {
                if j >= after.len() {
                    break;
                }
                let ch = after[j..].chars().next().unwrap();
                if ch == '\'' {
                    return Some(start + 1 + j + 1);
                }
                j += ch.len_utf8();
            }
        }
        return None;
    }
    let ch = after.chars().next()?;
    if ch != '\'' && after[ch.len_utf8()..].starts_with('\'') {
        return Some(start + 1 + ch.len_utf8() + 1);
    }
    None
}

fn scan_rust_lifetime_end(line: &str, start: usize) -> usize {
    let mut j = start + 1;
    while j < line.len() {
        let ch = line[j..].chars().next().unwrap();
        if is_ident_cont(ch) {
            j += ch.len_utf8();
        } else {
            break;
        }
    }
    j
}

fn is_json_key_suffix(suffix: &str) -> bool {
    suffix.trim_start().starts_with(':')
}

fn is_toml_key_context(line: &str, start: usize, end: usize) -> bool {
    let before = line[..start].trim();
    let valid_before = before.is_empty() || before.ends_with('{') || before.ends_with(',');
    if !valid_before {
        return false;
    }
    let after = line[end..].trim_start();
    after.starts_with('=') && !after.starts_with("==")
}

fn scan_toml_bare_key(line: &str, start: usize) -> Option<usize> {
    let before = line[..start].trim();
    if !(before.is_empty() || before.ends_with('{') || before.ends_with(',')) {
        return None;
    }
    let mut j = start;
    while j < line.len() {
        let b = line.as_bytes()[j];
        if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.') {
            j += 1;
        } else {
            break;
        }
    }
    if j == start {
        return None;
    }
    let after = line[j..].trim_start();
    if after.starts_with('=') && !after.starts_with("==") {
        Some(j)
    } else {
        None
    }
}

fn is_yaml_key_context(line: &str, start: usize, end: usize) -> bool {
    let before = line[..start].trim();
    if !(before.is_empty() || before == "-") {
        return false;
    }
    let after = &line[end..];
    after.starts_with(':')
        && after[1..]
            .chars()
            .next()
            .is_none_or(char::is_whitespace)
}

fn scan_yaml_bare_key(line: &str, start: usize) -> Option<usize> {
    let before = line[..start].trim();
    if !(before.is_empty() || before == "-") {
        return None;
    }
    let mut j = start;
    while j < line.len() {
        let b = line.as_bytes()[j];
        if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.') {
            j += 1;
        } else {
            break;
        }
    }
    if j == start {
        return None;
    }
    let after = &line[j..];
    if after.starts_with(':')
        && after[1..]
            .chars()
            .next()
            .is_none_or(char::is_whitespace)
    {
        Some(j)
    } else {
        None
    }
}

fn is_number_start(line: &str, start: usize) -> bool {
    if start == 0 {
        return true;
    }
    let prev = line[..start].chars().next_back().unwrap_or(' ');
    !is_ident_cont(prev)
}

fn scan_number(bytes: &[u8], start: usize) -> usize {
    let mut j = start;
    if bytes.get(j) == Some(&b'0')
        && matches!(
            bytes.get(j + 1).copied(),
            Some(b'x' | b'X' | b'b' | b'B' | b'o' | b'O')
        )
    {
        j += 2;
        while j < bytes.len() && (bytes[j].is_ascii_hexdigit() || bytes[j] == b'_') {
            j += 1;
        }
        return j;
    }

    while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b'_') {
        j += 1;
    }
    if bytes.get(j) == Some(&b'.') && bytes.get(j + 1).is_some_and(u8::is_ascii_digit) {
        j += 1;
        while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b'_') {
            j += 1;
        }
    }
    if matches!(bytes.get(j), Some(b'e' | b'E')) {
        let mut k = j + 1;
        if matches!(bytes.get(k), Some(b'+' | b'-')) {
            k += 1;
        }
        if bytes.get(k).is_some_and(u8::is_ascii_digit) {
            j = k;
            while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b'_') {
                j += 1;
            }
        }
    }
    j
}

fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

fn is_ident_cont(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn scan_ident(line: &str, start: usize) -> usize {
    let mut j = start;
    while j < line.len() {
        let ch = line[j..].chars().next().unwrap();
        if is_ident_cont(ch) {
            j += ch.len_utf8();
        } else {
            break;
        }
    }
    j
}

fn is_pascal_case_type(word: &str) -> bool {
    let bytes = word.as_bytes();
    bytes.len() >= 2
        && bytes[0].is_ascii_uppercase()
        && bytes.iter().any(u8::is_ascii_lowercase)
        && !bytes.contains(&b'_')
}

fn classify_ident(kind: Lang, word: &str) -> Style {
    match kind {
        Lang::Rust => {
            if matches!(
                word,
                "as" | "async"
                    | "await"
                    | "break"
                    | "const"
                    | "continue"
                    | "crate"
                    | "dyn"
                    | "else"
                    | "enum"
                    | "extern"
                    | "false"
                    | "fn"
                    | "for"
                    | "if"
                    | "impl"
                    | "in"
                    | "let"
                    | "loop"
                    | "match"
                    | "mod"
                    | "move"
                    | "mut"
                    | "pub"
                    | "ref"
                    | "return"
                    | "self"
                    | "static"
                    | "struct"
                    | "super"
                    | "trait"
                    | "true"
                    | "type"
                    | "unsafe"
                    | "use"
                    | "where"
                    | "while"
                    | "try"
                    | "macro_rules"
                    | "union"
            ) {
                kw_style()
            } else if matches!(
                word,
                "bool"
                    | "char"
                    | "str"
                    | "i8"
                    | "i16"
                    | "i32"
                    | "i64"
                    | "i128"
                    | "isize"
                    | "u8"
                    | "u16"
                    | "u32"
                    | "u64"
                    | "u128"
                    | "usize"
                    | "f32"
                    | "f64"
                    | "Self"
                    | "String"
                    | "Vec"
                    | "Option"
                    | "Result"
                    | "Some"
                    | "None"
                    | "Ok"
                    | "Err"
                    | "Box"
                    | "Rc"
                    | "Arc"
                    | "Cell"
                    | "RefCell"
                    | "Mutex"
                    | "HashMap"
                    | "HashSet"
                    | "BTreeMap"
                    | "BTreeSet"
                    | "Path"
                    | "PathBuf"
                    | "Cow"
                    | "Pin"
                    | "Future"
            ) || is_pascal_case_type(word)
            {
                type_style()
            } else {
                plain_style()
            }
        }
        Lang::JsTs => {
            if matches!(
                word,
                "abstract"
                    | "as"
                    | "async"
                    | "await"
                    | "break"
                    | "case"
                    | "catch"
                    | "class"
                    | "const"
                    | "continue"
                    | "debugger"
                    | "declare"
                    | "default"
                    | "delete"
                    | "do"
                    | "else"
                    | "enum"
                    | "export"
                    | "extends"
                    | "false"
                    | "finally"
                    | "for"
                    | "from"
                    | "function"
                    | "get"
                    | "if"
                    | "implements"
                    | "import"
                    | "in"
                    | "instanceof"
                    | "interface"
                    | "is"
                    | "keyof"
                    | "let"
                    | "module"
                    | "namespace"
                    | "new"
                    | "null"
                    | "of"
                    | "override"
                    | "package"
                    | "private"
                    | "protected"
                    | "public"
                    | "readonly"
                    | "return"
                    | "satisfies"
                    | "set"
                    | "static"
                    | "super"
                    | "switch"
                    | "this"
                    | "throw"
                    | "true"
                    | "try"
                    | "type"
                    | "typeof"
                    | "undefined"
                    | "var"
                    | "void"
                    | "while"
                    | "with"
                    | "yield"
            ) {
                kw_style()
            } else if matches!(
                word,
                "any"
                    | "bigint"
                    | "boolean"
                    | "never"
                    | "number"
                    | "object"
                    | "string"
                    | "symbol"
                    | "unknown"
                    | "Array"
                    | "Promise"
                    | "Record"
                    | "Partial"
                    | "Required"
                    | "Readonly"
                    | "Pick"
                    | "Omit"
                    | "Map"
                    | "Set"
                    | "Error"
                    | "Date"
                    | "RegExp"
            ) || is_pascal_case_type(word)
            {
                type_style()
            } else {
                plain_style()
            }
        }
        Lang::Python => {
            if matches!(
                word,
                "and"
                    | "as"
                    | "assert"
                    | "async"
                    | "await"
                    | "break"
                    | "class"
                    | "continue"
                    | "def"
                    | "del"
                    | "elif"
                    | "else"
                    | "except"
                    | "False"
                    | "finally"
                    | "for"
                    | "from"
                    | "global"
                    | "if"
                    | "import"
                    | "in"
                    | "is"
                    | "lambda"
                    | "None"
                    | "nonlocal"
                    | "not"
                    | "or"
                    | "pass"
                    | "raise"
                    | "return"
                    | "True"
                    | "try"
                    | "while"
                    | "with"
                    | "yield"
                    | "match"
                    | "case"
            ) {
                kw_style()
            } else if matches!(
                word,
                "int"
                    | "float"
                    | "str"
                    | "bool"
                    | "bytes"
                    | "bytearray"
                    | "list"
                    | "dict"
                    | "set"
                    | "frozenset"
                    | "tuple"
                    | "object"
                    | "type"
                    | "Any"
                    | "List"
                    | "Dict"
                    | "Set"
                    | "Tuple"
                    | "Optional"
                    | "Union"
                    | "Callable"
                    | "Sequence"
                    | "Mapping"
                    | "Iterable"
                    | "Iterator"
                    | "Final"
                    | "Literal"
                    | "Protocol"
                    | "Self"
            ) || is_pascal_case_type(word)
            {
                type_style()
            } else {
                plain_style()
            }
        }
        Lang::Shell => {
            if matches!(
                word,
                "if" | "then"
                    | "else"
                    | "elif"
                    | "fi"
                    | "for"
                    | "while"
                    | "until"
                    | "do"
                    | "done"
                    | "case"
                    | "esac"
                    | "in"
                    | "function"
                    | "select"
                    | "time"
                    | "return"
                    | "exit"
                    | "export"
                    | "local"
                    | "readonly"
                    | "declare"
                    | "typeset"
                    | "unset"
                    | "shift"
                    | "break"
                    | "continue"
                    | "source"
                    | "eval"
                    | "exec"
                    | "trap"
                    | "true"
                    | "false"
            ) {
                kw_style()
            } else {
                plain_style()
            }
        }
        Lang::Json | Lang::Toml | Lang::Yaml => {
            if matches!(word, "true" | "false" | "null" | "nan" | "inf") {
                kw_style()
            } else {
                plain_style()
            }
        }
        Lang::Nix => {
            if matches!(
                word,
                "let" | "in"
                    | "with"
                    | "inherit"
                    | "rec"
                    | "if"
                    | "then"
                    | "else"
                    | "import"
                    | "true"
                    | "false"
                    | "null"
                    | "or"
                    | "assert"
            ) {
                kw_style()
            } else if matches!(word, "builtins" | "pkgs" | "lib" | "stdenv" | "mkDerivation") {
                type_style()
            } else {
                plain_style()
            }
        }
        Lang::Sql => {
            let upper = word.to_ascii_uppercase();
            if matches!(
                upper.as_str(),
                "SELECT"
                    | "FROM"
                    | "WHERE"
                    | "INSERT"
                    | "INTO"
                    | "VALUES"
                    | "UPDATE"
                    | "SET"
                    | "DELETE"
                    | "CREATE"
                    | "TABLE"
                    | "INDEX"
                    | "VIEW"
                    | "DROP"
                    | "ALTER"
                    | "ADD"
                    | "COLUMN"
                    | "JOIN"
                    | "INNER"
                    | "LEFT"
                    | "RIGHT"
                    | "FULL"
                    | "OUTER"
                    | "CROSS"
                    | "ON"
                    | "AS"
                    | "AND"
                    | "OR"
                    | "NOT"
                    | "NULL"
                    | "IS"
                    | "IN"
                    | "EXISTS"
                    | "BETWEEN"
                    | "LIKE"
                    | "ILIKE"
                    | "ORDER"
                    | "BY"
                    | "GROUP"
                    | "HAVING"
                    | "LIMIT"
                    | "OFFSET"
                    | "UNION"
                    | "ALL"
                    | "DISTINCT"
                    | "CASE"
                    | "WHEN"
                    | "THEN"
                    | "ELSE"
                    | "END"
                    | "ASC"
                    | "DESC"
                    | "PRIMARY"
                    | "KEY"
                    | "FOREIGN"
                    | "REFERENCES"
                    | "UNIQUE"
                    | "CHECK"
                    | "DEFAULT"
                    | "CONSTRAINT"
                    | "BEGIN"
                    | "COMMIT"
                    | "ROLLBACK"
                    | "TRANSACTION"
                    | "RETURNING"
                    | "WITH"
                    | "RECURSIVE"
                    | "TRUE"
                    | "FALSE"
                    | "CAST"
                    | "COALESCE"
                    | "COUNT"
                    | "SUM"
                    | "AVG"
                    | "MIN"
                    | "MAX"
            ) {
                kw_style()
            } else if matches!(
                upper.as_str(),
                "INT"
                    | "INTEGER"
                    | "BIGINT"
                    | "SMALLINT"
                    | "TINYINT"
                    | "SERIAL"
                    | "BIGSERIAL"
                    | "TEXT"
                    | "VARCHAR"
                    | "CHAR"
                    | "BOOLEAN"
                    | "BOOL"
                    | "DATE"
                    | "TIME"
                    | "TIMESTAMP"
                    | "TIMESTAMPTZ"
                    | "NUMERIC"
                    | "DECIMAL"
                    | "FLOAT"
                    | "REAL"
                    | "DOUBLE"
                    | "JSON"
                    | "JSONB"
                    | "UUID"
                    | "BYTEA"
                    | "BLOB"
            ) {
                type_style()
            } else {
                plain_style()
            }
        }
        Lang::Go => {
            if matches!(
                word,
                "break"
                    | "default"
                    | "func"
                    | "interface"
                    | "select"
                    | "case"
                    | "defer"
                    | "go"
                    | "map"
                    | "struct"
                    | "chan"
                    | "else"
                    | "goto"
                    | "package"
                    | "switch"
                    | "const"
                    | "fallthrough"
                    | "if"
                    | "range"
                    | "type"
                    | "continue"
                    | "for"
                    | "import"
                    | "return"
                    | "var"
                    | "true"
                    | "false"
                    | "nil"
                    | "iota"
            ) {
                kw_style()
            } else if matches!(
                word,
                "bool"
                    | "byte"
                    | "complex64"
                    | "complex128"
                    | "error"
                    | "float32"
                    | "float64"
                    | "int"
                    | "int8"
                    | "int16"
                    | "int32"
                    | "int64"
                    | "rune"
                    | "string"
                    | "uint"
                    | "uint8"
                    | "uint16"
                    | "uint32"
                    | "uint64"
                    | "uintptr"
                    | "any"
                    | "comparable"
            ) || is_pascal_case_type(word)
            {
                type_style()
            } else {
                plain_style()
            }
        }
        Lang::CFamily => {
            if matches!(
                word,
                "auto"
                    | "break"
                    | "case"
                    | "const"
                    | "continue"
                    | "default"
                    | "do"
                    | "else"
                    | "enum"
                    | "extern"
                    | "for"
                    | "goto"
                    | "if"
                    | "inline"
                    | "register"
                    | "restrict"
                    | "return"
                    | "sizeof"
                    | "static"
                    | "struct"
                    | "switch"
                    | "typedef"
                    | "union"
                    | "volatile"
                    | "while"
                    | "alignas"
                    | "alignof"
                    | "and"
                    | "asm"
                    | "catch"
                    | "class"
                    | "concept"
                    | "consteval"
                    | "constexpr"
                    | "constinit"
                    | "const_cast"
                    | "co_await"
                    | "co_return"
                    | "co_yield"
                    | "decltype"
                    | "delete"
                    | "dynamic_cast"
                    | "explicit"
                    | "export"
                    | "false"
                    | "friend"
                    | "mutable"
                    | "namespace"
                    | "new"
                    | "noexcept"
                    | "nullptr"
                    | "operator"
                    | "or"
                    | "override"
                    | "final"
                    | "private"
                    | "protected"
                    | "public"
                    | "reinterpret_cast"
                    | "requires"
                    | "static_assert"
                    | "static_cast"
                    | "template"
                    | "this"
                    | "thread_local"
                    | "throw"
                    | "true"
                    | "try"
                    | "typeid"
                    | "typename"
                    | "using"
                    | "virtual"
                    | "NULL"
            ) {
                kw_style()
            } else if matches!(
                word,
                "void"
                    | "bool"
                    | "char"
                    | "char8_t"
                    | "char16_t"
                    | "char32_t"
                    | "wchar_t"
                    | "short"
                    | "int"
                    | "long"
                    | "signed"
                    | "unsigned"
                    | "float"
                    | "double"
                    | "int8_t"
                    | "int16_t"
                    | "int32_t"
                    | "int64_t"
                    | "uint8_t"
                    | "uint16_t"
                    | "uint32_t"
                    | "uint64_t"
                    | "size_t"
                    | "ssize_t"
                    | "ptrdiff_t"
                    | "intptr_t"
                    | "uintptr_t"
                    | "std"
                    | "string"
                    | "string_view"
                    | "vector"
                    | "map"
                    | "unordered_map"
                    | "set"
                    | "optional"
                    | "variant"
                    | "unique_ptr"
                    | "shared_ptr"
                    | "span"
            ) || is_pascal_case_type(word)
            {
                type_style()
            } else {
                plain_style()
            }
        }
        Lang::Diff | Lang::Plain => plain_style(),
    }
}
