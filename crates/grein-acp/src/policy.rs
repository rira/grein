use crate::permissions::{PermissionDecision, ToolInvocationRequest};
use crate::session::{SessionId, ThreadId};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    Deny,
    PromptUser,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyError {
    ParseError(String),
}

impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ParseError(msg) => write!(f, "Policy parse error: {msg}"),
        }
    }
}

impl std::error::Error for PolicyError {}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SecurityPolicy {
    allow_read_tools: bool,
    allowed_command_prefixes: Vec<String>,
    allow_commands: Vec<String>,
    deny_commands: Vec<String>,
    auto_allow_writes_in_worktrees: bool,
}

impl SecurityPolicy {
    pub fn builder() -> SecurityPolicyBuilder {
        SecurityPolicyBuilder::default()
    }

    pub fn allow_read_tools(&self) -> bool {
        self.allow_read_tools
    }

    pub fn allowed_command_prefixes(&self) -> &[String] {
        &self.allowed_command_prefixes
    }

    pub fn allow_commands(&self) -> &[String] {
        &self.allow_commands
    }

    pub fn deny_commands(&self) -> &[String] {
        &self.deny_commands
    }

    pub fn auto_allow_writes_in_worktrees(&self) -> bool {
        self.auto_allow_writes_in_worktrees
    }

    pub fn from_toml_configs(
        global_toml: Option<&str>,
        local_toml: Option<&str>,
    ) -> Result<Self, PolicyError> {
        let mut policy = Self::default();

        if let Some(global) = global_toml {
            let parsed = parse_toml_policy(global)?;
            policy.merge(parsed);
        }

        if let Some(local) = local_toml {
            let parsed = parse_toml_policy(local)?;
            policy.merge(parsed);
        }

        Ok(policy)
    }

    pub fn merge(&mut self, other: Self) {
        if other.allow_read_tools {
            self.allow_read_tools = true;
        }
        if other.auto_allow_writes_in_worktrees {
            self.auto_allow_writes_in_worktrees = true;
        }
        merge_unique(
            &mut self.allowed_command_prefixes,
            other.allowed_command_prefixes,
        );
        merge_unique(&mut self.allow_commands, other.allow_commands);
        merge_unique(&mut self.deny_commands, other.deny_commands);
    }
}

fn merge_unique(target: &mut Vec<String>, incoming: Vec<String>) {
    for item in incoming {
        if !target.contains(&item) {
            target.push(item);
        }
    }
}

#[derive(Debug, Default)]
pub struct SecurityPolicyBuilder {
    allow_read_tools: bool,
    allowed_command_prefixes: Vec<String>,
    allow_commands: Vec<String>,
    deny_commands: Vec<String>,
    auto_allow_writes_in_worktrees: bool,
}

impl SecurityPolicyBuilder {
    pub fn allow_read_tools(mut self, allow: bool) -> Self {
        self.allow_read_tools = allow;
        self
    }

    pub fn allowed_command_prefixes(mut self, prefixes: Vec<String>) -> Self {
        self.allowed_command_prefixes = prefixes;
        self
    }

    pub fn allow_commands(mut self, commands: Vec<String>) -> Self {
        self.allow_commands = commands;
        self
    }

    pub fn deny_commands(mut self, commands: Vec<String>) -> Self {
        self.deny_commands = commands;
        self
    }

    pub fn auto_allow_writes_in_worktrees(mut self, allow: bool) -> Self {
        self.auto_allow_writes_in_worktrees = allow;
        self
    }

    pub fn build(self) -> SecurityPolicy {
        SecurityPolicy {
            allow_read_tools: self.allow_read_tools,
            allowed_command_prefixes: self.allowed_command_prefixes,
            allow_commands: self.allow_commands,
            deny_commands: self.deny_commands,
            auto_allow_writes_in_worktrees: self.auto_allow_writes_in_worktrees,
        }
    }
}

fn parse_bool(key: &str, val: &str) -> Result<bool, PolicyError> {
    val.parse::<bool>()
        .map_err(|e| PolicyError::ParseError(format!("Invalid boolean for {key}: {e}")))
}

fn is_array_key(key: &str) -> bool {
    matches!(
        key,
        "allowed_command_prefixes" | "allow_commands" | "deny_commands"
    )
}

fn store_array(policy: &mut SecurityPolicy, key: &str, items: Vec<String>) {
    match key {
        "allowed_command_prefixes" => policy.allowed_command_prefixes.extend(items),
        "allow_commands" => policy.allow_commands.extend(items),
        "deny_commands" => policy.deny_commands.extend(items),
        _ => {}
    }
}

fn parse_toml_policy(content: &str) -> Result<SecurityPolicy, PolicyError> {
    let mut policy = SecurityPolicy::default();

    let mut inside_array: Option<String> = None;
    let mut array_buffer = String::new();

    for raw_line in content.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        if let Some(key) = &inside_array {
            array_buffer.push(' ');
            array_buffer.push_str(line);
            if line.contains(']') {
                let items = parse_string_array(&array_buffer)?;
                store_array(&mut policy, key, items);
                inside_array = None;
                array_buffer.clear();
            }
            continue;
        }

        // Section headers: [permissions], [policy], [worktree] (keys are section-agnostic).
        if line.starts_with('[') && line.ends_with(']') && !line.contains('=') {
            continue;
        }

        if let Some((key, val)) = line.split_once('=') {
            let key = key.trim();
            let val = val.trim();

            match key {
                "allow_read_tools" | "auto_allow_read_tools" => {
                    if parse_bool(key, val)? {
                        policy.allow_read_tools = true;
                    }
                }
                "auto_allow_writes_in_worktrees" => {
                    if parse_bool(key, val)? {
                        policy.auto_allow_writes_in_worktrees = true;
                    }
                }
                k if is_array_key(k) => {
                    if val.starts_with('[') && val.ends_with(']') {
                        let items = parse_string_array(val)?;
                        store_array(&mut policy, k, items);
                    } else if val.starts_with('[') {
                        inside_array = Some(k.to_string());
                        array_buffer = val.to_string();
                    } else {
                        return Err(PolicyError::ParseError(format!(
                            "Expected array for {k}, got {val}"
                        )));
                    }
                }
                _ => {}
            }
        }
    }

    Ok(policy)
}

fn parse_string_array(raw: &str) -> Result<Vec<String>, PolicyError> {
    let trimmed = raw.trim();
    let content = trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .ok_or_else(|| PolicyError::ParseError(format!("Malformed array: {raw}")))?;

    let mut items = Vec::new();
    for piece in content.split(',') {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        let stripped = piece
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| piece.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
            .ok_or_else(|| PolicyError::ParseError(format!("Malformed string element: {piece}")))?;
        items.push(stripped.to_string());
    }
    Ok(items)
}

#[derive(Debug)]
pub struct PolicyGate {
    policy: SecurityPolicy,
    session_permissions: HashMap<(SessionId, String), PermissionDecision>,
    thread_permissions: HashMap<(SessionId, ThreadId, String), PermissionDecision>,
    once_permissions: HashSet<(SessionId, ThreadId, String)>,
}

impl PolicyGate {
    pub fn new(policy: SecurityPolicy) -> Self {
        Self {
            policy,
            session_permissions: HashMap::new(),
            thread_permissions: HashMap::new(),
            once_permissions: HashSet::new(),
        }
    }

    pub fn evaluate(&self, request: &ToolInvocationRequest) -> PolicyDecision {
        let command = if request.tool_name() == "run_command" {
            request.command_line().map(str::trim)
        } else {
            None
        };

        if let Some(cmd) = command {
            if self
                .policy
                .deny_commands()
                .iter()
                .any(|pattern| command_matches(pattern, cmd))
            {
                return PolicyDecision::Deny;
            }
        }

        if self.policy.allow_read_tools() && is_read_tool(request.tool_name()) {
            return PolicyDecision::Allow;
        }

        if self.policy.auto_allow_writes_in_worktrees()
            && request.is_write_tool()
            && request.is_in_isolated_worktree()
        {
            return PolicyDecision::Allow;
        }

        if let Some(cmd) = command {
            if contains_shell_metacharacter(cmd) {
                return PolicyDecision::PromptUser;
            }
            let allowed = self
                .policy
                .allow_commands()
                .iter()
                .chain(self.policy.allowed_command_prefixes().iter())
                .any(|pattern| command_matches(pattern, cmd));
            if allowed {
                return PolicyDecision::Allow;
            }
        }

        PolicyDecision::PromptUser
    }

    pub fn evaluate_scoped(
        &mut self,
        session_id: &SessionId,
        thread_id: &ThreadId,
        request: &ToolInvocationRequest,
    ) -> PolicyDecision {
        let req_key = request_key(request);

        if let Some(decision) = self
            .session_permissions
            .get(&(session_id.clone(), req_key.clone()))
        {
            match decision {
                PermissionDecision::AlwaysSession => return PolicyDecision::Allow,
                PermissionDecision::Deny => return PolicyDecision::Deny,
                _ => {}
            }
        }

        let thread_key = (session_id.clone(), thread_id.clone(), req_key);

        if let Some(decision) = self.thread_permissions.get(&thread_key) {
            match decision {
                PermissionDecision::AlwaysThread => return PolicyDecision::Allow,
                PermissionDecision::Deny => return PolicyDecision::Deny,
                _ => {}
            }
        }

        if self.once_permissions.remove(&thread_key) {
            return PolicyDecision::Allow;
        }

        self.evaluate(request)
    }

    pub fn record_decision(
        &mut self,
        session_id: &SessionId,
        thread_id: &ThreadId,
        request: &ToolInvocationRequest,
        decision: PermissionDecision,
    ) {
        let req_key = request_key(request);
        match decision {
            PermissionDecision::AlwaysSession => {
                self.session_permissions
                    .insert((session_id.clone(), req_key), decision);
            }
            PermissionDecision::AlwaysThread => {
                self.thread_permissions
                    .insert((session_id.clone(), thread_id.clone(), req_key), decision);
            }
            PermissionDecision::AllowOnce => {
                self.once_permissions
                    .insert((session_id.clone(), thread_id.clone(), req_key));
            }
            PermissionDecision::Deny => {
                self.session_permissions
                    .insert((session_id.clone(), req_key), decision);
            }
        }
    }
}

fn request_key(request: &ToolInvocationRequest) -> String {
    format!("{}:{}", request.tool_name(), request.parameters())
}

fn is_read_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "read_file" | "read" | "list_files" | "list_dir" | "search"
    )
}

fn contains_shell_metacharacter(cmd: &str) -> bool {
    cmd.contains([';', '|', '`', '>', '<', '&', '\n', '\r']) || cmd.contains("$(")
}

/// Matches a command against a pattern. A trailing `*` is a plain prefix glob;
/// otherwise the pattern must match exactly or on a whitespace word boundary.
fn command_matches(pattern: &str, cmd: &str) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return false;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return cmd.starts_with(prefix);
    }
    match cmd.strip_prefix(pattern) {
        Some(rest) => rest.is_empty() || rest.starts_with(char::is_whitespace),
        None => false,
    }
}
