use std::collections::HashSet;
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

impl AcpAgentKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Agy => "agy",
            Self::Claude => "claude",
            Self::Custom => "custom",
        }
    }
}

const CLAUDE_NATIVE_ACP_STEMS: &[&str] = &["claude-agent-acp", "claude-code-acp"];

const DISCOVERY_CANDIDATES: &[(AcpAgentKind, &[&str])] = &[
    (AcpAgentKind::Agy, &["agy"]),
    (AcpAgentKind::Claude, CLAUDE_NATIVE_ACP_STEMS),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpAgent {
    pub kind: AcpAgentKind,
    pub executable_path: PathBuf,
    pub args: Vec<String>,
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

fn find_kind_executable(
    kind: AcpAgentKind,
    candidate_names: &[&str],
    dirs: &[PathBuf],
) -> Option<AcpAgent> {
    for name in candidate_names {
        for dir in dirs {
            if dir.as_os_str().is_empty() {
                continue;
            }
            let candidate_path = dir.join(name);
            if is_executable_file(&candidate_path) {
                return Some(AcpAgent {
                    kind,
                    executable_path: candidate_path,
                    args: Vec::new(),
                });
            }
        }
    }
    None
}

pub fn discover_acp_agents(search_path: Option<&str>) -> Vec<AcpAgent> {
    let dirs: Vec<PathBuf> = match search_path {
        Some(p) => std::env::split_paths(p).collect(),
        None => std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default(),
    };

    let mut discovered = Vec::new();
    let mut seen_kinds = HashSet::new();

    for &(kind, candidate_names) in DISCOVERY_CANDIDATES {
        if !seen_kinds.insert(kind) {
            continue;
        }
        if let Some(agent) = find_kind_executable(kind, candidate_names, &dirs) {
            discovered.push(agent);
        }
    }

    discovered
}

pub(crate) fn claude_acp_args(_executable: &Path) -> Vec<String> {
    Vec::new()
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

pub(crate) fn kind_from_label(label: &str) -> AcpAgentKind {
    let trimmed = label.trim();
    let stem = Path::new(trimmed)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(trimmed);
    match stem.trim().to_ascii_lowercase().as_str() {
        "agy" => AcpAgentKind::Agy,
        s if s == "claude" || CLAUDE_NATIVE_ACP_STEMS.contains(&s) => AcpAgentKind::Claude,
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
    let args = raw.command.get(1..).unwrap_or_default().to_vec();
    Some(AcpAgent {
        kind,
        executable_path: PathBuf::from(executable),
        args,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn write_exec(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write exec");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&path).expect("meta").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).expect("chmod");
        }
        path
    }

    #[test]
    fn claude_agent_acp_in_later_path_beats_claude_in_earlier_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let early = temp.path().join("early");
        let late = temp.path().join("late");
        fs::create_dir_all(&early).unwrap();
        fs::create_dir_all(&late).unwrap();

        let _early_claude = write_exec(&early, "claude");
        let late_acp = write_exec(&late, "claude-agent-acp");

        let search = std::env::join_paths([&early, &late])
            .expect("join_paths")
            .to_string_lossy()
            .into_owned();
        let discovered = discover_acp_agents(Some(&search));

        assert_eq!(
            discovered,
            vec![AcpAgent {
                kind: AcpAgentKind::Claude,
                executable_path: late_acp,
                args: Vec::new(),
            }]
        );
    }

    #[test]
    fn claude_code_acp_beats_claude_when_claude_agent_acp_is_absent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let early = temp.path().join("early");
        let late = temp.path().join("late");
        fs::create_dir_all(&early).unwrap();
        fs::create_dir_all(&late).unwrap();

        let _early_claude = write_exec(&early, "claude");
        let late_code_acp = write_exec(&late, "claude-code-acp");

        let search = std::env::join_paths([&early, &late])
            .expect("join_paths")
            .to_string_lossy()
            .into_owned();
        let discovered = discover_acp_agents(Some(&search));

        assert_eq!(
            discovered,
            vec![AcpAgent {
                kind: AcpAgentKind::Claude,
                executable_path: late_code_acp,
                args: Vec::new(),
            }]
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_and_duplicate_path_entries_discover_cleanly_without_duplicates() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let dir1 = temp.path().join("dir1");
        let dir2 = temp.path().join("dir2");
        let target_dir = temp.path().join("targets");
        fs::create_dir_all(&dir1).unwrap();
        fs::create_dir_all(&dir2).unwrap();
        fs::create_dir_all(&target_dir).unwrap();

        let _code_acp = write_exec(&dir1, "claude-code-acp");
        let real_agent_acp = write_exec(&target_dir, "real-claude-agent-acp");
        let linked_agent_acp = dir2.join("claude-agent-acp");
        symlink(&real_agent_acp, &linked_agent_acp).expect("symlink executable");
        symlink(target_dir.join("missing"), dir1.join("agy")).expect("broken symlink");

        let search = std::env::join_paths([&dir1, &dir2, &dir1])
            .expect("join_paths")
            .to_string_lossy()
            .into_owned();
        let discovered = discover_acp_agents(Some(&search));

        assert_eq!(
            discovered,
            vec![AcpAgent {
                kind: AcpAgentKind::Claude,
                executable_path: linked_agent_acp,
                args: Vec::new(),
            }]
        );
    }

    #[test]
    fn kind_from_label_maps_claude_acp_aliases_and_preserves_existing() {
        assert_eq!(kind_from_label("agy"), AcpAgentKind::Agy);
        assert_eq!(kind_from_label("AGY"), AcpAgentKind::Agy);
        assert_eq!(kind_from_label("/usr/bin/agy"), AcpAgentKind::Agy);
        assert_eq!(kind_from_label("claude"), AcpAgentKind::Claude);
        assert_eq!(kind_from_label("claude-agent-acp"), AcpAgentKind::Claude);
        assert_eq!(kind_from_label("claude-code-acp"), AcpAgentKind::Claude);
        assert_eq!(
            kind_from_label("/nix/store/123-claude/bin/CLAUDE-AGENT-ACP"),
            AcpAgentKind::Claude
        );
        assert_eq!(kind_from_label("custom-tool"), AcpAgentKind::Custom);
        assert_eq!(AcpAgentKind::Agy.label(), "agy");
        assert_eq!(AcpAgentKind::Claude.label(), "claude");
        assert_eq!(AcpAgentKind::Custom.label(), "custom");
    }

    #[test]
    fn plain_claude_cli_is_never_discovered_on_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bin_dir = temp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let _plain_claude = write_exec(&bin_dir, "claude");

        let search = bin_dir.to_string_lossy().into_owned();
        let discovered = discover_acp_agents(Some(&search));
        assert!(discovered.is_empty());
    }

    #[test]
    fn claude_acp_args_always_returns_empty_without_invalid_acp_flag() {
        assert!(claude_acp_args(Path::new("claude-agent-acp")).is_empty());
        assert!(claude_acp_args(Path::new("/nix/store/abc/bin/claude-agent-acp")).is_empty());
        assert!(claude_acp_args(Path::new("/usr/local/bin/claude-code-acp")).is_empty());
        assert!(claude_acp_args(Path::new("CLAUDE-CODE-ACP.exe")).is_empty());
        assert!(claude_acp_args(Path::new("/usr/bin/claude")).is_empty());
        assert!(claude_acp_args(Path::new("claude")).is_empty());
        assert!(claude_acp_args(Path::new("/opt/wrapped-claude-acp")).is_empty());
    }

    #[test]
    fn toml_command_array_preserves_trailing_args_on_acp_agent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cfg_path = temp.path().join("grein.toml");
        fs::write(
            &cfg_path,
            "[[agents]]\nname = \"claude\"\ncommand = [\"npx\", \"-y\", \"@agentclientprotocol/claude-agent-acp\"]\n",
        )
        .expect("write cfg");

        let resolved =
            resolve_agents_configuration(Some(&cfg_path), None).expect("resolve agents");
        assert_eq!(
            resolved,
            vec![AcpAgent {
                kind: AcpAgentKind::Claude,
                executable_path: PathBuf::from("npx"),
                args: vec![
                    "-y".to_string(),
                    "@agentclientprotocol/claude-agent-acp".to_string()
                ],
            }]
        );
    }
}
