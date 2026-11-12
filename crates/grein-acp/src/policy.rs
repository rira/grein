use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDecision {
    Allowed,
    Prompt,
    Denied,
}

#[derive(Debug)]
pub enum PolicyError {
    ParseError(String),
    IoError(String),
}

impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolicyError::ParseError(msg) => write!(f, "Policy parse error: {msg}"),
            PolicyError::IoError(msg) => write!(f, "Policy IO error: {msg}"),
        }
    }
}

impl std::error::Error for PolicyError {}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct SecurityPolicy {
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub allowed_command_prefixes: Vec<String>,
    #[serde(default)]
    pub denied_commands: Vec<String>,
}

impl SecurityPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_toml(content: &str) -> Result<Self, PolicyError> {
        toml::from_str(content).map_err(|e| PolicyError::ParseError(e.to_string()))
    }

    pub fn allowed_tools(&self) -> &[String] {
        &self.allowed_tools
    }

    pub fn allowed_command_prefixes(&self) -> &[String] {
        &self.allowed_command_prefixes
    }

    pub fn denied_commands(&self) -> &[String] {
        &self.denied_commands
    }

    pub fn evaluate_tool(&self, tool_name: &str) -> PolicyDecision {
        if self.allowed_tools.iter().any(|t| t == tool_name) {
            PolicyDecision::Allowed
        } else {
            PolicyDecision::Prompt
        }
    }

    pub fn evaluate_command(&self, command: &str) -> PolicyDecision {
        let trimmed = command.trim();

        for denied in &self.denied_commands {
            let d = denied.trim();
            if trimmed == d || trimmed.starts_with(&format!("{d} ")) || trimmed.starts_with(d) {
                return PolicyDecision::Denied;
            }
        }

        for prefix in &self.allowed_command_prefixes {
            let p = prefix.trim();
            if trimmed == p || trimmed.starts_with(&format!("{p} ")) || trimmed.starts_with(prefix) {
                return PolicyDecision::Allowed;
            }
        }

        PolicyDecision::Prompt
    }

    pub fn merge(&mut self, other: SecurityPolicy) {
        for tool in other.allowed_tools {
            if !self.allowed_tools.contains(&tool) {
                self.allowed_tools.push(tool);
            }
        }
        for prefix in other.allowed_command_prefixes {
            if !self.allowed_command_prefixes.contains(&prefix) {
                self.allowed_command_prefixes.push(prefix);
            }
        }
        for denied in other.denied_commands {
            if !self.denied_commands.contains(&denied) {
                self.denied_commands.push(denied);
            }
        }
    }

    pub fn load_merged(
        global_path: Option<impl AsRef<Path>>,
        local_path: Option<impl AsRef<Path>>,
    ) -> Result<Self, PolicyError> {
        let mut policy = SecurityPolicy::default();

        if let Some(path) = global_path {
            let p = path.as_ref();
            let content = std::fs::read_to_string(p)
                .map_err(|e| PolicyError::IoError(e.to_string()))?;
            let loaded = Self::from_toml(&content)?;
            policy.merge(loaded);
        } else if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            let default_path = home.join(".config").join("grein").join("policy.toml");
            if default_path.exists() {
                let content = std::fs::read_to_string(&default_path)
                    .map_err(|e| PolicyError::IoError(e.to_string()))?;
                let loaded = Self::from_toml(&content)?;
                policy.merge(loaded);
            }
        }

        if let Some(path) = local_path {
            let p = path.as_ref();
            let content = std::fs::read_to_string(p)
                .map_err(|e| PolicyError::IoError(e.to_string()))?;
            let loaded = Self::from_toml(&content)?;
            policy.merge(loaded);
        } else {
            let default_path = PathBuf::from(".grein-policy.toml");
            if default_path.exists() {
                let content = std::fs::read_to_string(&default_path)
                    .map_err(|e| PolicyError::IoError(e.to_string()))?;
                let loaded = Self::from_toml(&content)?;
                policy.merge(loaded);
            }
        }

        Ok(policy)
    }
}