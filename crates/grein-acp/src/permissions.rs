use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PermissionDecision {
    AllowOnce,
    Deny,
    AlwaysThread,
    AlwaysSession,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInvocationRequest {
    tool_name: String,
    parameters: Value,
    in_isolated_worktree: bool,
}

impl ToolInvocationRequest {
    pub fn new(tool_name: impl Into<String>, parameters: Value) -> Self {
        Self {
            tool_name: tool_name.into(),
            parameters,
            in_isolated_worktree: false,
        }
    }

    pub fn with_isolated_worktree(mut self, isolated: bool) -> Self {
        self.in_isolated_worktree = isolated;
        self
    }

    pub fn is_in_isolated_worktree(&self) -> bool {
        self.in_isolated_worktree
    }

    pub fn is_write_tool(&self) -> bool {
        matches!(
            self.tool_name.as_str(),
            "write_file" | "edit_file" | "delete_file" | "create_file" | "write"
        )
    }

    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }

    pub fn parameters(&self) -> &Value {
        &self.parameters
    }

    pub fn command_line(&self) -> Option<&str> {
        self.parameters
            .get("CommandLine")
            .or_else(|| self.parameters.get("command"))
            .or_else(|| self.parameters.get("cmd"))
            .and_then(Value::as_str)
    }

    pub fn path(&self) -> Option<&str> {
        self.parameters
            .get("Path")
            .or_else(|| self.parameters.get("path"))
            .and_then(Value::as_str)
    }
}
