use ratatui::style::Style;

use super::string_style;

pub(super) fn scan_rust_attribute(line: &str, start: usize) -> usize {
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

pub(super) fn scan_c_directive_end(line: &str, start: usize) -> usize {
    let mut j = start + 1;
    while j < line.len() && line.as_bytes()[j].is_ascii_whitespace() {
        j += 1;
    }
    while j < line.len() && is_ident_cont(line[j..].chars().next().unwrap()) {
        j += 1;
    }
    j
}

pub(super) fn scan_toml_section(line: &str, start: usize) -> Option<usize> {
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

pub(super) fn scan_decorator(line: &str, start: usize) -> Option<usize> {
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

pub(super) fn scan_shell_var(line: &str, start: usize) -> Option<usize> {
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

pub(super) fn scan_rust_prefixed_string(line: &str, start: usize) -> Option<usize> {
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

pub(super) fn scan_python_prefixed_string(line: &str, start: usize) -> Option<(usize, Style)> {
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

pub(super) fn scan_quoted(line: &str, start: usize, quote: char, allow_escape: bool) -> usize {
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

pub(super) fn scan_sql_single_quoted(line: &str, start: usize) -> usize {
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

pub(super) fn scan_triple_quoted(
    line: &str,
    start: usize,
    delim: &str,
    allow_escape: bool,
) -> usize {
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

pub(super) fn scan_nix_indented_string(line: &str, start: usize) -> usize {
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

pub(super) fn scan_rust_char_literal(line: &str, start: usize) -> Option<usize> {
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

pub(super) fn scan_rust_lifetime_end(line: &str, start: usize) -> usize {
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

pub(super) fn is_json_key_suffix(suffix: &str) -> bool {
    suffix.trim_start().starts_with(':')
}

pub(super) fn is_toml_key_context(line: &str, start: usize, end: usize) -> bool {
    let before = line[..start].trim();
    let valid_before = before.is_empty() || before.ends_with('{') || before.ends_with(',');
    if !valid_before {
        return false;
    }
    let after = line[end..].trim_start();
    after.starts_with('=') && !after.starts_with("==")
}

pub(super) fn scan_toml_bare_key(line: &str, start: usize) -> Option<usize> {
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

pub(super) fn is_yaml_key_context(line: &str, start: usize, end: usize) -> bool {
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

pub(super) fn scan_yaml_bare_key(line: &str, start: usize) -> Option<usize> {
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

pub(super) fn is_number_start(line: &str, start: usize) -> bool {
    if start == 0 {
        return true;
    }
    let prev = line[..start].chars().next_back().unwrap_or(' ');
    !is_ident_cont(prev)
}

pub(super) fn scan_number(bytes: &[u8], start: usize) -> usize {
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

pub(super) fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

fn is_ident_cont(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

pub(super) fn scan_ident(line: &str, start: usize) -> usize {
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
