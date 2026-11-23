mod keywords;
mod scanners;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use keywords::classify_ident;
use scanners::{
    is_ident_start, is_json_key_suffix, is_number_start, is_toml_key_context, is_yaml_key_context,
    scan_c_directive_end, scan_decorator, scan_ident, scan_nix_indented_string, scan_number,
    scan_python_prefixed_string, scan_quoted, scan_rust_attribute, scan_rust_char_literal,
    scan_rust_lifetime_end, scan_rust_prefixed_string, scan_shell_var, scan_sql_single_quoted,
    scan_toml_bare_key, scan_toml_section, scan_triple_quoted, scan_yaml_bare_key,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Lang {
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

pub(super) fn kw_style() -> Style {
    Style::default()
        .fg(Color::Magenta)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn type_style() -> Style {
    Style::default().fg(Color::Cyan)
}

pub(super) fn string_style() -> Style {
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

pub(super) fn plain_style() -> Style {
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
