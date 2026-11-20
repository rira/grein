use std::fs;
use std::iter::Peekable;
use std::path::{Path, PathBuf};
use std::str::Chars;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AcpAgentKind {
    Agy,
    Claude,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpAgent {
    pub kind: AcpAgentKind,
    pub executable_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryError {
    IoError(String),
}

fn is_executable_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(path) {
            meta.is_file() && (meta.permissions().mode() & 0o111 != 0)
        } else {
            false
        }
    }
    #[cfg(not(unix))]
    {
        if let Ok(meta) = fs::metadata(path) {
            meta.is_file()
        } else {
            false
        }
    }
}

pub fn discover_acp_agents(search_path: Option<&str>) -> Vec<AcpAgent> {
    let dirs: Vec<PathBuf> = match search_path {
        Some(p) => std::env::split_paths(p).collect(),
        None => std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default(),
    };

    let candidates = [("agy", AcpAgentKind::Agy), ("claude", AcpAgentKind::Claude)];

    let mut discovered = Vec::new();

    for (name, kind) in candidates {
        for dir in &dirs {
            let candidate_path = dir.join(name);
            if is_executable_file(&candidate_path) {
                discovered.push(AcpAgent {
                    kind,
                    executable_path: candidate_path,
                });
                break;
            }
        }
    }

    discovered
}

/// A parsed TOML value (only the shapes relevant to agent declarations).
enum Value {
    Str(String),
    Array(Vec<String>),
    Other,
}

/// Raw agent declaration collected from `[[agents]]` or `[agent]`.
#[derive(Default)]
struct RawAgent {
    name: Option<String>,
    kind: Option<String>,
    command: Vec<String>,
    executable: Option<String>,
}

enum Section {
    Other,
    Agent,
}

fn strip_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (idx, ch) in line.char_indices() {
        match quote {
            Some(q) => {
                if escaped {
                    escaped = false;
                } else if ch == '\\' && q == '"' {
                    escaped = true;
                } else if ch == q {
                    quote = None;
                }
            }
            None => {
                if ch == '"' || ch == '\'' {
                    quote = Some(ch);
                } else if ch == '#' {
                    return &line[..idx];
                }
            }
        }
    }
    line
}

fn skip_ws(chars: &mut Peekable<Chars<'_>>) {
    while chars.peek().is_some_and(|c| c.is_whitespace()) {
        chars.next();
    }
}

fn parse_string(chars: &mut Peekable<Chars<'_>>) -> Result<String, String> {
    let quote = match chars.next() {
        Some(q @ ('"' | '\'')) => q,
        _ => return Err("expected quoted string".to_string()),
    };
    let mut out = String::new();
    loop {
        match chars.next() {
            None => return Err("unterminated string".to_string()),
            Some(c) if c == quote => return Ok(out),
            Some('\\') if quote == '"' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                _ => return Err("invalid escape sequence".to_string()),
            },
            Some(c) => out.push(c),
        }
    }
}

fn parse_value(raw: &str) -> Result<Value, String> {
    let mut chars = raw.trim().chars().peekable();
    match chars.peek() {
        None => Err("missing value".to_string()),
        Some('"') | Some('\'') => {
            let s = parse_string(&mut chars)?;
            skip_ws(&mut chars);
            if chars.peek().is_some() {
                return Err("unexpected trailing characters after string".to_string());
            }
            Ok(Value::Str(s))
        }
        Some('[') => {
            chars.next();
            let mut items = Vec::new();
            loop {
                skip_ws(&mut chars);
                match chars.peek() {
                    None => return Err("unterminated array".to_string()),
                    Some(']') => {
                        chars.next();
                        break;
                    }
                    Some(_) => {}
                }
                items.push(parse_string(&mut chars)?);
                skip_ws(&mut chars);
                match chars.next() {
                    Some(',') => {}
                    Some(']') => break,
                    _ => return Err("expected ',' or ']' in array".to_string()),
                }
            }
            skip_ws(&mut chars);
            if chars.peek().is_some() {
                return Err("unexpected trailing characters after array".to_string());
            }
            Ok(Value::Array(items))
        }
        Some(_) => Ok(Value::Other),
    }
}

fn parse_agents(contents: &str) -> Result<Vec<RawAgent>, String> {
    let mut agents: Vec<RawAgent> = Vec::new();
    let mut section = Section::Other;

    for (idx, original) in contents.lines().enumerate() {
        let lineno = idx + 1;
        let line = strip_comment(original).trim();
        if line.is_empty() {
            continue;
        }

        if line.starts_with('[') {
            let name = if let Some(rest) = line.strip_prefix("[[") {
                let inner = rest
                    .strip_suffix("]]")
                    .ok_or_else(|| format!("line {lineno}: malformed array-of-tables header"))?;
                (true, inner.trim())
            } else {
                let inner = line
                    .strip_prefix('[')
                    .and_then(|r| r.strip_suffix(']'))
                    .ok_or_else(|| format!("line {lineno}: malformed table header"))?;
                (false, inner.trim())
            };
            let (is_array, table) = name;
            if table.is_empty() || table.contains('[') || table.contains(']') {
                return Err(format!("line {lineno}: malformed table header"));
            }
            section = match (is_array, table) {
                (true, "agents") | (false, "agent") => {
                    agents.push(RawAgent::default());
                    Section::Agent
                }
                _ => Section::Other,
            };
            continue;
        }

        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("line {lineno}: expected key = value"))?;
        let key = key.trim();
        if key.is_empty() {
            return Err(format!("line {lineno}: missing key"));
        }
        let value = parse_value(value).map_err(|e| format!("line {lineno}: {e}"))?;

        if let Section::Agent = section {
            if let Some(agent) = agents.last_mut() {
                match (key, value) {
                    ("name", Value::Str(s)) => agent.name = Some(s),
                    ("kind", Value::Str(s)) => agent.kind = Some(s),
                    ("executable" | "path", Value::Str(s)) => agent.executable = Some(s),
                    ("command", Value::Array(items)) => agent.command = items,
                    ("command", Value::Str(s)) => agent.command = vec![s],
                    _ => {}
                }
            }
        }
    }

    Ok(agents)
}

fn kind_from_label(label: &str) -> AcpAgentKind {
    let stem = Path::new(label)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(label);
    match stem.trim().to_ascii_lowercase().as_str() {
        "agy" => AcpAgentKind::Agy,
        "claude" => AcpAgentKind::Claude,
        _ => AcpAgentKind::Custom,
    }
}

fn into_agent(raw: RawAgent) -> Option<AcpAgent> {
    let executable = match raw.executable {
        Some(exe) if !exe.is_empty() => exe,
        _ => raw.command.first().filter(|c| !c.is_empty())?.clone(),
    };
    let kind = match raw.kind.as_deref().or(raw.name.as_deref()) {
        Some(label) => kind_from_label(label),
        None => kind_from_label(&executable),
    };
    Some(AcpAgent {
        kind,
        executable_path: PathBuf::from(executable),
    })
}

pub fn resolve_agents_configuration<P: AsRef<Path>>(
    config_path: Option<P>,
    search_path: Option<&str>,
) -> Result<Vec<AcpAgent>, DiscoveryError> {
    let path = match config_path.as_ref().map(|p| p.as_ref()) {
        Some(p) if p.exists() => p,
        _ => return Ok(discover_acp_agents(search_path)),
    };

    let contents = fs::read_to_string(path)
        .map_err(|e| DiscoveryError::IoError(format!("{}: {e}", path.display())))?;
    let raw_agents = parse_agents(&contents)
        .map_err(|e| DiscoveryError::IoError(format!("{}: {e}", path.display())))?;

    let agents: Vec<AcpAgent> = raw_agents.into_iter().filter_map(into_agent).collect();
    if agents.is_empty() {
        Ok(discover_acp_agents(search_path))
    } else {
        Ok(agents)
    }
}
