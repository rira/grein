use crate::permissions::{PermissionDecision, ToolInvocationRequest};
use crate::session::{SessionId, ThreadId};
use std::collections::HashMap;

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
        for prefix in other.allowed_command_prefixes {
            if !self.allowed_command_prefixes.contains(&prefix) {
                self.allowed_command_prefixes.push(prefix);
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct SecurityPolicyBuilder {
    allow_read_tools: bool,
    allowed_command_prefixes: Vec<String>,
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

    pub fn build(self) -> SecurityPolicy {
        SecurityPolicy {
            allow_read_tools: self.allow_read_tools,
            allowed_command_prefixes: self.allowed_command_prefixes,
        }
    }
}

fn parse_toml_policy(content: &str) -> Result<SecurityPolicy, PolicyError> {
    let mut allow_read_tools = false;
    let mut allowed_command_prefixes = Vec::new();

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
                if key == "allowed_command_prefixes" {
                    allowed_command_prefixes.extend(items);
                }
                inside_array = None;
                array_buffer.clear();
            }
            continue;
        }

        if let Some((key, val)) = line.split_once('=') {
            let key = key.trim();
            let val = val.trim();

            match key {
                "allow_read_tools" => {
                    allow_read_tools = val.parse::<bool>().map_err(|e| {
                        PolicyError::ParseError(format!("Invalid boolean for {key}: {e}"))
                    })?;
                }
                "allowed_command_prefixes" => {
                    if val.starts_with('[') && val.ends_with(']') {
                        let items = parse_string_array(val)?;
                        allowed_command_prefixes.extend(items);
                    } else if val.starts_with('[') {
                        inside_array = Some(key.to_string());
                        array_buffer = val.to_string();
                    } else {
                        return Err(PolicyError::ParseError(format!(
                            "Expected array for {key}, got {val}"
                        )));
                    }
                }
                _ => {}
            }
        }
    }

    Ok(SecurityPolicy {
        allow_read_tools,
        allowed_command_prefixes,
    })
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
}

impl PolicyGate {
    pub fn new(policy: SecurityPolicy) -> Self {
        Self {
            policy,
            session_permissions: HashMap::new(),
            thread_permissions: HashMap::new(),
        }
    }

    pub fn evaluate(&self, request: &ToolInvocationRequest) -> PolicyDecision {
        if self.policy.allow_read_tools() && is_read_tool(request.tool_name()) {
            return PolicyDecision::Allow;
        }

        if request.tool_name() == "run_command" {
            if let Some(cmd) = request.command_line() {
                for prefix in self.policy.allowed_command_prefixes() {
                    if cmd.starts_with(prefix) {
                        return PolicyDecision::Allow;
                    }
                }
            }
        }

        PolicyDecision::PromptUser
    }

    pub fn evaluate_scoped(
        &self,
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
                PermissionDecision::AlwaysSession | PermissionDecision::AllowOnce => {
                    return PolicyDecision::Allow
                }
                PermissionDecision::Deny => return PolicyDecision::Deny,
                _ => {}
            }
        }

        if let Some(decision) =
            self.thread_permissions
                .get(&(session_id.clone(), thread_id.clone(), req_key))
        {
            match decision {
                PermissionDecision::AlwaysThread | PermissionDecision::AllowOnce => {
                    return PolicyDecision::Allow
                }
                PermissionDecision::Deny => return PolicyDecision::Deny,
                _ => {}
            }
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
            PermissionDecision::AlwaysThread | PermissionDecision::AllowOnce => {
                self.thread_permissions
                    .insert((session_id.clone(), thread_id.clone(), req_key), decision);
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
