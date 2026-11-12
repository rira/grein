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
}

impl ToolInvocationRequest {
    pub fn new(tool_name: impl Into<String>, parameters: Value) -> Self {
        Self {
            tool_name: tool_name.into(),
            parameters,
        }
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
