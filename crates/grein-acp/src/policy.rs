use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::Path;

#[derive(Debug)]
pub enum PolicyError {
    ParseError(String),
    IoError(std::io::Error),
}

impl fmt::Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PolicyError::ParseError(msg) => write!(f, "Failed to parse policy: {msg}"),
            PolicyError::IoError(err) => write!(f, "I/O error reading policy file: {err}"),
        }
    }
}

impl std::error::Error for PolicyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PolicyError::ParseError(_) => None,
            PolicyError::IoError(err) => Some(err),
        }
    }
}

impl From<std::io::Error> for PolicyError {
    fn from(err: std::io::Error) -> Self {
        PolicyError::IoError(err)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityPolicy {
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub allowed_commands: Vec<String>,
}

impl SecurityPolicy {
    pub fn from_toml_str(content: &str) -> Result<Self, PolicyError> {
        toml::from_str(content).map_err(|err| PolicyError::ParseError(err.to_string()))
    }

    pub fn is_tool_allowed(&self, tool: &str) -> bool {
        self.allowed_tools.iter().any(|t| t == tool)
    }

    pub fn is_command_allowed(&self, cmd: &str) -> bool {
        let cmd = cmd.trim();
        if cmd.is_empty() {
            return false;
        }
        self.allowed_commands.iter().any(|allowed| {
            let allowed = allowed.trim();
            if allowed.is_empty() {
                return false;
            }
            cmd == allowed || cmd.starts_with(&format!("{allowed} "))
        })
    }

    pub fn merge(&self, other: &Self) -> Self {
        let mut allowed_tools = self.allowed_tools.clone();
        for tool in &other.allowed_tools {
            if !allowed_tools.contains(tool) {
                allowed_tools.push(tool.clone());
            }
        }
        let mut allowed_commands = self.allowed_commands.clone();
        for cmd in &other.allowed_commands {
            if !allowed_commands.contains(cmd) {
                allowed_commands.push(cmd.clone());
            }
        }
        Self {
            allowed_tools,
            allowed_commands,
        }
    }

    pub fn load_from_paths<P1: AsRef<Path>, P2: AsRef<Path>>(
        global_path: Option<P1>,
        local_path: Option<P2>,
    ) -> Result<Self, PolicyError> {
        let mut policy = Self::default();

        if let Some(path) = global_path {
            let p = path.as_ref();
            if p.exists() {
                let content = fs::read_to_string(p)?;
                let parsed = Self::from_toml_str(&content)?;
                policy = policy.merge(&parsed);
            }
        }

        if let Some(path) = local_path {
            let p = path.as_ref();
            if p.exists() {
                let content = fs::read_to_string(p)?;
                let parsed = Self::from_toml_str(&content)?;
                policy = policy.merge(&parsed);
            }
        }

        Ok(policy)
    }
}
