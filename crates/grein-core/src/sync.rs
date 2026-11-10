use crate::blackboard::{Blackboard, BlackboardEntryKind, DecisionLandmark, KnowledgeImport};
use crate::error::CoreError;
use crate::model::{ConversationNode, NodeId, ThreadId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ContextPayload {
    pub thread_id: Option<ThreadId>,
    pub base_system_prompt: String,
    pub blackboard_notes: String,
    pub active_files: Vec<String>,
    pub decision_landmarks: Vec<String>,
}

impl ContextPayload {
    pub fn new(base_system_prompt: impl Into<String>, blackboard_notes: impl Into<String>) -> Self {
        Self {
            thread_id: None,
            base_system_prompt: base_system_prompt.into(),
            blackboard_notes: blackboard_notes.into(),
            active_files: Vec::new(),
            decision_landmarks: Vec::new(),
        }
    }

    pub fn with_thread(mut self, thread_id: ThreadId) -> Self {
        self.thread_id = Some(thread_id);
        self
    }

    pub fn build_system_prompt(&self) -> String {
        if self.blackboard_notes.trim().is_empty() {
            return self.base_system_prompt.clone();
        }
        if self.base_system_prompt.trim().is_empty() {
            return format!(
                "# Shared Blackboard Working Memory\n{}",
                self.blackboard_notes.trim()
            );
        }
        format!(
            "{}\n\n# Shared Blackboard Working Memory\n{}",
            self.base_system_prompt.trim(),
            self.blackboard_notes.trim()
        )
    }
}

/// Builds a context payload for a given thread with empty/default blackboard notes.
pub fn build_context_prompt(thread_id: ThreadId) -> Result<ContextPayload, CoreError> {
    Ok(ContextPayload {
        thread_id: Some(thread_id),
        base_system_prompt: String::new(),
        blackboard_notes: String::new(),
        active_files: Vec::new(),
        decision_landmarks: Vec::new(),
    })
}

/// Automatically injects active Blackboard notes into the system prompt across threads.
pub fn build_context_prompt_with_blackboard(
    thread_id: ThreadId,
    base_prompt: &str,
    blackboard: &Blackboard,
) -> Result<ContextPayload, CoreError> {
    let markdown_notes = blackboard.to_markdown();
    let active_files = blackboard
        .find_by_kind(BlackboardEntryKind::ActiveFile)
        .iter()
        .map(|entry| entry.value.clone())
        .collect();

    Ok(ContextPayload {
        thread_id: Some(thread_id),
        base_system_prompt: base_prompt.to_string(),
        blackboard_notes: markdown_notes,
        active_files,
        decision_landmarks: Vec::new(),
    })
}

/// Compiler service for building context prompts with injected blackboard working memory.
#[derive(Debug, Clone, Default)]
pub struct ContextCompiler {
    base_prompt: String,
}

impl ContextCompiler {
    pub fn new(base_prompt: impl Into<String>) -> Self {
        Self {
            base_prompt: base_prompt.into(),
        }
    }

    pub fn base_prompt(&self) -> &str {
        &self.base_prompt
    }

    pub fn build_context_prompt(&self, thread_id: ThreadId) -> Result<ContextPayload, CoreError> {
        Ok(ContextPayload {
            thread_id: Some(thread_id),
            base_system_prompt: self.base_prompt.clone(),
            blackboard_notes: String::new(),
            active_files: Vec::new(),
            decision_landmarks: Vec::new(),
        })
    }

    pub fn build_context_prompt_with_blackboard(
        &self,
        thread_id: ThreadId,
        blackboard: &Blackboard,
    ) -> Result<ContextPayload, CoreError> {
        build_context_prompt_with_blackboard(thread_id, &self.base_prompt, blackboard)
    }
}

/// On-demand thread import mechanism: pulls synthesized decision summary turns
/// from a source thread into a target thread without full DAG rebase.
pub fn import_thread_knowledge(
    _source_thread_id: ThreadId,
    target_thread_id: ThreadId,
    import: &KnowledgeImport,
    parent_node_id: Option<NodeId>,
) -> Result<ConversationNode, CoreError> {
    tracing::debug!(
        target_thread = %target_thread_id,
        source = %import.source_thread,
        topic = %import.topic,
        "Importing synthesized knowledge turn into target thread"
    );
    let mut node = import.to_conversation_node(parent_node_id);
    node.content = import.to_turn_content();
    Ok(node)
}

/// Thread merge synthesizer: produces a permanent 2-3 line decision landmark
/// committed back to Trunk.
pub fn synthesize_thread_merge(
    thread_id: ThreadId,
    topic: impl Into<String>,
    summary: impl Into<String>,
    parent_node_id: Option<NodeId>,
) -> Result<ConversationNode, CoreError> {
    let landmark = DecisionLandmark::new(thread_id, topic, summary);
    Ok(landmark.to_conversation_node(parent_node_id))
}
