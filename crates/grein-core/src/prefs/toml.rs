use super::{SectionPatch, SettingsPatch, SettingsSnapshot, SETTINGS_VERSION, THEME_NAMESPACE};
use crate::error::CoreError;
use std::collections::BTreeMap;

pub(super) fn format_toml_snapshot(snapshot: &SettingsSnapshot) -> String {
    let mut out = format!("# grein configuration\nversion = {}\n", snapshot.version);
    for (section, entries) in &snapshot.preferences {
        if entries.is_empty() {
            continue;
        }
        out.push('\n');
        format_toml_section(&mut out, section, entries);
    }
    out
}

pub(super) fn format_toml_single_section(
    section: &str,
    entries: &BTreeMap<String, String>,
) -> String {
    let mut out = String::new();
    format_toml_section(&mut out, section, entries);
    out
}

pub(super) fn parse_toml_settings(raw: &str) -> Result<SettingsPatch, CoreError> {
    let doc = parse_toml_document(raw)?;
    let mut preferences = BTreeMap::new();
    for (key, val) in doc.root_entries {
        if val.is_some() {
            return Err(CoreError::SyncError(format!(
                "unexpected top-level key '{key}' outside any [section]"
            )));
        }
        preferences.insert(key, SectionPatch::ResetToDefault);
    }
    for (section, entries) in doc.sections {
        let patch = if entries.is_empty() {
            SectionPatch::ResetToDefault
        } else {
            SectionPatch::Entries(entries)
        };
        preferences.insert(section, patch);
    }
    Ok(SettingsPatch {
        version: doc.version.unwrap_or(SETTINGS_VERSION),
        preferences,
    })
}

pub(super) fn parse_toml_theme(raw: &str) -> Result<SettingsPatch, CoreError> {
    let mut doc = parse_toml_document(raw)?;
    let version = doc.version.unwrap_or(SETTINGS_VERSION);
    let mut preferences = BTreeMap::new();

    if let Some(entries) = doc.sections.remove(THEME_NAMESPACE) {
        ensure_no_root_scalars(&doc.root_entries)?;
        let patch = if entries.is_empty() {
            SectionPatch::ResetToDefault
        } else {
            SectionPatch::Entries(entries)
        };
        preferences.insert(THEME_NAMESPACE.to_string(), patch);
        return Ok(SettingsPatch {
            version,
            preferences,
        });
    }

    if matches!(doc.root_entries.get(THEME_NAMESPACE), Some(None)) {
        ensure_no_root_scalars(&doc.root_entries)?;
        preferences.insert(THEME_NAMESPACE.to_string(), SectionPatch::ResetToDefault);
        return Ok(SettingsPatch {
            version,
            preferences,
        });
    }

    if !doc.sections.is_empty() {
        return Err(CoreError::SyncError(
            "settings TOML does not contain a [theme] section".to_string(),
        ));
    }

    if doc.root_entries.is_empty() {
        return Err(CoreError::SyncError(
            "theme TOML contains neither a [theme] section nor theme keys".to_string(),
        ));
    }

    preferences.insert(
        THEME_NAMESPACE.to_string(),
        SectionPatch::Entries(doc.root_entries),
    );
    Ok(SettingsPatch {
        version,
        preferences,
    })
}

fn is_bare_toml_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn escape_toml_string(val: &str) -> String {
    let mut out = String::with_capacity(val.len());
    for ch in val.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                use std::fmt::Write;
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

fn format_toml_key(key: &str) -> String {
    if is_bare_toml_key(key) {
        key.to_string()
    } else {
        format!("\"{}\"", escape_toml_string(key))
    }
}

fn format_toml_section(out: &mut String, section: &str, entries: &BTreeMap<String, String>) {
    out.push('[');
    out.push_str(&format_toml_key(section));
    out.push_str("]\n");
    for (key, value) in entries {
        out.push_str(&format_toml_key(key));
        out.push_str(" = \"");
        out.push_str(&escape_toml_string(value));
        out.push_str("\"\n");
    }
}

fn ensure_no_root_scalars(
    root_entries: &BTreeMap<String, Option<String>>,
) -> Result<(), CoreError> {
    for (key, val) in root_entries {
        if val.is_some() {
            return Err(CoreError::SyncError(format!(
                "unexpected top-level key '{key}' outside any [section]"
            )));
        }
    }
    Ok(())
}

struct ParsedToml {
    version: Option<u32>,
    root_entries: BTreeMap<String, Option<String>>,
    sections: BTreeMap<String, BTreeMap<String, Option<String>>>,
}

enum TomlVal {
    Null,
    Int(String),
    Scalar(String),
}

impl TomlVal {
    fn into_option_string(self) -> Option<String> {
        match self {
            TomlVal::Null => None,
            TomlVal::Int(s) | TomlVal::Scalar(s) => Some(s),
        }
    }
}

fn parse_toml_document(raw: &str) -> Result<ParsedToml, CoreError> {
    let mut doc = ParsedToml {
        version: None,
        root_entries: BTreeMap::new(),
        sections: BTreeMap::new(),
    };
    let mut current_section: Option<String> = None;

    for (idx, line) in raw.lines().enumerate() {
        let line_no = idx + 1;
        let pos = skip_ws(line, 0);
        if pos >= line.len() || line.as_bytes()[pos] == b'#' {
            continue;
        }

        if line.as_bytes()[pos] == b'[' {
            let section = parse_section_header(line, pos, line_no)?;
            if doc.root_entries.contains_key(&section) || doc.sections.contains_key(&section) {
                return Err(CoreError::SyncError(format!(
                    "line {line_no}: duplicate section '[{section}]'"
                )));
            }
            doc.sections.insert(section.clone(), BTreeMap::new());
            current_section = Some(section);
            continue;
        }

        let (key, next_pos) = parse_toml_key(line, pos, line_no)?;
        let eq_pos = skip_ws(line, next_pos);
        if line.as_bytes().get(eq_pos) != Some(&b'=') {
            return Err(CoreError::SyncError(format!(
                "line {line_no}: expected '=' after key '{key}'"
            )));
        }
        let val = parse_toml_value(line, eq_pos + 1, line_no)?;

        if let Some(sec_name) = current_section.as_deref() {
            let sec_map = doc.sections.get_mut(sec_name).ok_or_else(|| {
                CoreError::SyncError(format!("line {line_no}: missing active section"))
            })?;
            if sec_map.contains_key(&key) {
                return Err(CoreError::SyncError(format!(
                    "line {line_no}: duplicate key '{key}' in section '[{sec_name}]'"
                )));
            }
            sec_map.insert(key, val.into_option_string());
        } else if key == "version" {
            if doc.version.is_some() {
                return Err(CoreError::SyncError(format!(
                    "line {line_no}: duplicate 'version' key"
                )));
            }
            match val {
                TomlVal::Int(int_str) => {
                    let v = int_str.parse::<u32>().map_err(|_| {
                        CoreError::SyncError(format!(
                            "line {line_no}: invalid settings 'version': expected unsigned integer"
                        ))
                    })?;
                    doc.version = Some(v);
                }
                _ => {
                    return Err(CoreError::SyncError(format!(
                        "line {line_no}: invalid settings 'version': expected unsigned integer"
                    )));
                }
            }
        } else {
            if doc.root_entries.contains_key(&key) || doc.sections.contains_key(&key) {
                return Err(CoreError::SyncError(format!(
                    "line {line_no}: duplicate top-level key '{key}'"
                )));
            }
            doc.root_entries.insert(key, val.into_option_string());
        }
    }

    Ok(doc)
}

fn skip_ws(s: &str, mut pos: usize) -> usize {
    let bytes = s.as_bytes();
    while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t') {
        pos += 1;
    }
    pos
}

fn expect_eol_or_comment(s: &str, pos: usize, line_no: usize) -> Result<(), CoreError> {
    let pos = skip_ws(s, pos);
    if pos >= s.len() || s.as_bytes()[pos] == b'#' {
        Ok(())
    } else {
        Err(CoreError::SyncError(format!(
            "line {line_no}: unexpected trailing content '{}'",
            &s[pos..]
        )))
    }
}

fn parse_unicode_escape(
    s: &str,
    pos: usize,
    digits: usize,
    line_no: usize,
) -> Result<(char, usize), CoreError> {
    let hex = s.get(pos..pos + digits).ok_or_else(|| {
        CoreError::SyncError(format!(
            "line {line_no}: incomplete unicode escape sequence"
        ))
    })?;
    let code = u32::from_str_radix(hex, 16).map_err(|_| {
        CoreError::SyncError(format!("line {line_no}: invalid unicode escape '\\u{hex}'"))
    })?;
    let ch = char::from_u32(code).ok_or_else(|| {
        CoreError::SyncError(format!(
            "line {line_no}: invalid unicode scalar value 0x{code:X}"
        ))
    })?;
    Ok((ch, pos + digits))
}

fn parse_double_quoted(
    s: &str,
    start: usize,
    line_no: usize,
) -> Result<(String, usize), CoreError> {
    let mut pos = start + 1;
    let mut out = String::new();
    while pos < s.len() {
        let ch = s[pos..].chars().next().ok_or_else(|| {
            CoreError::SyncError(format!("line {line_no}: unterminated double-quoted string"))
        })?;
        match ch {
            '"' => return Ok((out, pos + 1)),
            '\\' => {
                pos += 1;
                let esc = s[pos..].chars().next().ok_or_else(|| {
                    CoreError::SyncError(format!(
                        "line {line_no}: unterminated escape sequence in double-quoted string"
                    ))
                })?;
                pos += esc.len_utf8();
                match esc {
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    'b' => out.push('\u{0008}'),
                    'f' => out.push('\u{000C}'),
                    'u' => {
                        let (uc, next) = parse_unicode_escape(s, pos, 4, line_no)?;
                        out.push(uc);
                        pos = next;
                    }
                    'U' => {
                        let (uc, next) = parse_unicode_escape(s, pos, 8, line_no)?;
                        out.push(uc);
                        pos = next;
                    }
                    other => {
                        return Err(CoreError::SyncError(format!(
                            "line {line_no}: invalid TOML escape '\\{other}'"
                        )));
                    }
                }
            }
            c if c.is_control() && c != '\t' => {
                return Err(CoreError::SyncError(format!(
                    "line {line_no}: unescaped control character in string"
                )));
            }
            c => {
                out.push(c);
                pos += c.len_utf8();
            }
        }
    }
    Err(CoreError::SyncError(format!(
        "line {line_no}: unterminated double-quoted string"
    )))
}

fn parse_single_quoted(
    s: &str,
    start: usize,
    line_no: usize,
) -> Result<(String, usize), CoreError> {
    let mut pos = start + 1;
    while pos < s.len() {
        let ch = s[pos..].chars().next().ok_or_else(|| {
            CoreError::SyncError(format!("line {line_no}: unterminated single-quoted string"))
        })?;
        if ch == '\'' {
            return Ok((s[start + 1..pos].to_string(), pos + 1));
        }
        if ch.is_control() && ch != '\t' {
            return Err(CoreError::SyncError(format!(
                "line {line_no}: unescaped control character in literal string"
            )));
        }
        pos += ch.len_utf8();
    }
    Err(CoreError::SyncError(format!(
        "line {line_no}: unterminated single-quoted string"
    )))
}

fn is_bare_toml_parse_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')
}

fn parse_toml_key(s: &str, start: usize, line_no: usize) -> Result<(String, usize), CoreError> {
    let (key, next_pos) = match s.as_bytes().get(start) {
        Some(b'"') => parse_double_quoted(s, start, line_no)?,
        Some(b'\'') => parse_single_quoted(s, start, line_no)?,
        Some(_) => {
            let mut pos = start;
            let bytes = s.as_bytes();
            while pos < bytes.len() && is_bare_toml_parse_byte(bytes[pos]) {
                pos += 1;
            }
            if pos == start {
                return Err(CoreError::SyncError(format!(
                    "line {line_no}: expected key before '{}'",
                    &s[start..]
                )));
            }
            (s[start..pos].to_string(), pos)
        }
        None => {
            return Err(CoreError::SyncError(format!(
                "line {line_no}: expected key"
            )));
        }
    };
    if key.trim().is_empty() {
        return Err(CoreError::SyncError(format!(
            "line {line_no}: key cannot be empty"
        )));
    }
    Ok((key, next_pos))
}

fn parse_section_header(s: &str, start: usize, line_no: usize) -> Result<String, CoreError> {
    if s.as_bytes().get(start + 1) == Some(&b'[') {
        return Err(CoreError::SyncError(format!(
            "line {line_no}: TOML array tables '[[...]]' are not supported"
        )));
    }
    let pos = skip_ws(s, start + 1);
    if pos >= s.len() || s.as_bytes()[pos] == b']' {
        return Err(CoreError::SyncError(format!(
            "line {line_no}: section name cannot be empty"
        )));
    }
    let (section, next_pos) = parse_toml_key(s, pos, line_no)?;
    let close_pos = skip_ws(s, next_pos);
    if s.as_bytes().get(close_pos) != Some(&b']') {
        return Err(CoreError::SyncError(format!(
            "line {line_no}: invalid or unterminated section header"
        )));
    }
    expect_eol_or_comment(s, close_pos + 1, line_no)?;
    Ok(section)
}

fn parse_toml_value(s: &str, start: usize, line_no: usize) -> Result<TomlVal, CoreError> {
    let pos = skip_ws(s, start);
    if pos >= s.len() || s.as_bytes()[pos] == b'#' {
        return Err(CoreError::SyncError(format!(
            "line {line_no}: missing value after '='"
        )));
    }
    match s.as_bytes()[pos] {
        b'"' => {
            let (val, next) = parse_double_quoted(s, pos, line_no)?;
            expect_eol_or_comment(s, next, line_no)?;
            Ok(TomlVal::Scalar(val))
        }
        b'\'' => {
            let (val, next) = parse_single_quoted(s, pos, line_no)?;
            expect_eol_or_comment(s, next, line_no)?;
            Ok(TomlVal::Scalar(val))
        }
        _ => {
            let raw_end = s[pos..].find('#').map(|i| pos + i).unwrap_or(s.len());
            let token = s[pos..raw_end].trim();
            if token.is_empty() {
                return Err(CoreError::SyncError(format!(
                    "line {line_no}: missing value after '='"
                )));
            }
            if token == "null" {
                Ok(TomlVal::Null)
            } else if token == "true" || token == "false" {
                Ok(TomlVal::Scalar(token.to_string()))
            } else if is_valid_toml_int(token) {
                Ok(TomlVal::Int(token.replace('_', "")))
            } else if is_valid_toml_float(token) {
                Ok(TomlVal::Scalar(token.replace('_', "")))
            } else {
                Err(CoreError::SyncError(format!(
                    "line {line_no}: invalid TOML value '{token}'"
                )))
            }
        }
    }
}

fn is_valid_digit_run(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let bytes = s.as_bytes();
    if bytes[0] == b'_' || bytes[bytes.len() - 1] == b'_' {
        return false;
    }
    let mut prev_underscore = false;
    for &b in bytes {
        if b == b'_' {
            if prev_underscore {
                return false;
            }
            prev_underscore = true;
        } else if b.is_ascii_digit() {
            prev_underscore = false;
        } else {
            return false;
        }
    }
    true
}

fn is_valid_toml_int(token: &str) -> bool {
    let body = token.strip_prefix(['+', '-']).unwrap_or(token);
    is_valid_digit_run(body)
}

fn is_valid_toml_float(token: &str) -> bool {
    let body = token.strip_prefix(['+', '-']).unwrap_or(token);
    let (mantissa, exp_opt) = match body.find(['e', 'E']) {
        Some(idx) => (&body[..idx], Some(&body[idx + 1..])),
        None => (body, None),
    };
    if let Some(exp) = exp_opt {
        let exp_digits = exp.strip_prefix(['+', '-']).unwrap_or(exp);
        if !is_valid_digit_run(exp_digits) {
            return false;
        }
    }
    match mantissa.split_once('.') {
        Some((int_part, frac_part)) => {
            is_valid_digit_run(int_part) && is_valid_digit_run(frac_part)
        }
        None => exp_opt.is_some() && is_valid_digit_run(mantissa),
    }
}
