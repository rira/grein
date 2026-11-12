use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PermissionScope {
    AllowOnce,
    Deny,
    AlwaysThread,
    AlwaysSession,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRequest {
    pub session_id: String,
    pub thread_id: String,
    pub tool_name: String,
    pub argument: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvaluationResult {
    Allowed,
    Denied,
    PromptRequired(PermissionRequest),
}

#[derive(Debug, Default, Clone)]
pub struct PermissionGate {
    always_session: HashSet<(String, String, String)>,
    always_thread: HashSet<(String, String, String, String)>,
    denied: HashSet<(String, String, String, String)>,
}

impl PermissionGate {
    pub fn new() -> Self {
        Self {
            always_session: HashSet::new(),
            always_thread: HashSet::new(),
            denied: HashSet::new(),
        }
    }

    pub fn evaluate(
        &self,
        session_id: &str,
        thread_id: &str,
        tool_name: &str,
        argument: &str,
    ) -> EvaluationResult {
        let is_intercepted = matches!(tool_name, "run_command" | "write_file" | "edit_file");
        if !is_intercepted {
            return EvaluationResult::Allowed;
        }

        if self.denied.contains(&(
            session_id.to_string(),
            thread_id.to_string(),
            tool_name.to_string(),
            argument.to_string(),
        )) {
            return EvaluationResult::Denied;
        }

        if self.always_session.contains(&(
            session_id.to_string(),
            tool_name.to_string(),
            argument.to_string(),
        )) {
            return EvaluationResult::Allowed;
        }

        if self.always_thread.contains(&(
            session_id.to_string(),
            thread_id.to_string(),
            tool_name.to_string(),
            argument.to_string(),
        )) {
            return EvaluationResult::Allowed;
        }

        EvaluationResult::PromptRequired(PermissionRequest {
            session_id: session_id.to_string(),
            thread_id: thread_id.to_string(),
            tool_name: tool_name.to_string(),
            argument: argument.to_string(),
        })
    }

    pub fn resolve_prompt(
        &mut self,
        session_id: &str,
        thread_id: &str,
        tool_name: &str,
        argument: &str,
        scope: PermissionScope,
    ) -> bool {
        match scope {
            PermissionScope::AllowOnce => true,
            PermissionScope::AlwaysThread => {
                self.always_thread.insert((
                    session_id.to_string(),
                    thread_id.to_string(),
                    tool_name.to_string(),
                    argument.to_string(),
                ));
                true
            }
            PermissionScope::AlwaysSession => {
                self.always_session.insert((
                    session_id.to_string(),
                    tool_name.to_string(),
                    argument.to_string(),
                ));
                true
            }
            PermissionScope::Deny => {
                self.denied.insert((
                    session_id.to_string(),
                    thread_id.to_string(),
                    tool_name.to_string(),
                    argument.to_string(),
                ));
                false
            }
        }
    }
}
