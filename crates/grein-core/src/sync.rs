use crate::blackboard::{Blackboard, BlackboardEntryKind, DecisionLandmark, KnowledgeImport};
use crate::dag::DagStorage;
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
    /// Root -> `parent_node_id` path shared by every thread branching from that node.
    #[serde(default)]
    pub trunk_path: Vec<ConversationNode>,
    /// Markdown header describing the anchor the thread branched from.
    #[serde(default)]
    pub anchor_header: Option<String>,
    /// Turns strictly after `parent_node_id` up to the thread's `active_leaf_id`.
    #[serde(default)]
    pub branch_turns: Vec<ConversationNode>,
}

impl ContextPayload {
    pub fn new(base_system_prompt: impl Into<String>, blackboard_notes: impl Into<String>) -> Self {
        Self {
            thread_id: None,
            base_system_prompt: base_system_prompt.into(),
            blackboard_notes: blackboard_notes.into(),
            ..Self::default()
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

    pub fn build_acp_system_prompt_append(
        &self,
        include_branch_turns: bool,
        max_chars: usize,
    ) -> String {
        let mut preserved_sections = Vec::new();
        if !self.base_system_prompt.trim().is_empty() {
            preserved_sections.push(self.base_system_prompt.trim().to_string());
        }
        if !self.blackboard_notes.trim().is_empty() {
            preserved_sections.push(format!(
                "# Shared Blackboard Working Memory\n{}",
                self.blackboard_notes.trim()
            ));
        }
        if let Some(header) = self.anchor_header.as_deref() {
            if !header.trim().is_empty() {
                preserved_sections.push(format!("# Branch Context\n{}", header.trim()));
            }
        }
        let preserved = preserved_sections.join("\n\n");

        let trunk_nodes: &[ConversationNode] = &self.trunk_path;
        let branch_nodes: &[ConversationNode] = if include_branch_turns {
            &self.branch_turns
        } else {
            &[]
        };

        let full_history = format_history_sections(trunk_nodes, branch_nodes);
        let full = join_preserved_and_history(&preserved, &full_history);

        if max_chars == 0 || full.chars().count() <= max_chars {
            return full;
        }

        let preserved_chars = preserved.chars().count();
        let history_budget = if preserved.is_empty() {
            max_chars
        } else {
            let needed_with_sep = preserved_chars.saturating_add(2);
            if needed_with_sep >= max_chars {
                return preserved;
            }
            max_chars - needed_with_sep
        };

        let mut trunk_slice = trunk_nodes;
        let mut branch_slice = branch_nodes;

        while !trunk_slice.is_empty()
            && !(trunk_slice.len() == 1 && branch_slice.is_empty())
            && format_history_sections(trunk_slice, branch_slice)
                .chars()
                .count()
                > history_budget
        {
            trunk_slice = &trunk_slice[1..];
        }

        while branch_slice.len() > 1
            && format_history_sections(trunk_slice, branch_slice)
                .chars()
                .count()
                > history_budget
        {
            branch_slice = &branch_slice[1..];
        }

        let trimmed_history = format_history_sections(trunk_slice, branch_slice);
        if trimmed_history.chars().count() <= history_budget {
            return join_preserved_and_history(&preserved, &trimmed_history);
        }

        let single_section_trimmed = if let Some(node) = branch_slice.first() {
            trim_single_node_section("# Branch History\n", node, history_budget)
        } else if let Some(node) = trunk_slice.first() {
            trim_single_node_section("# Ancestor Conversation Trunk\n", node, history_budget)
        } else {
            String::new()
        };

        if !single_section_trimmed.is_empty() {
            return join_preserved_and_history(&preserved, &single_section_trimmed);
        }

        if preserved.is_empty() {
            tail_chars_with_boundary(&full_history, history_budget).to_string()
        } else {
            preserved
        }
    }
}

fn format_node_entry(node: &ConversationNode) -> String {
    format!("[{}]\n{}", node.role, node.content)
}

fn format_history_sections(
    trunk_nodes: &[ConversationNode],
    branch_nodes: &[ConversationNode],
) -> String {
    let mut parts = Vec::new();
    if !trunk_nodes.is_empty() {
        let entries: Vec<String> = trunk_nodes.iter().map(format_node_entry).collect();
        parts.push(format!(
            "# Ancestor Conversation Trunk\n{}",
            entries.join("\n\n")
        ));
    }
    if !branch_nodes.is_empty() {
        let entries: Vec<String> = branch_nodes.iter().map(format_node_entry).collect();
        parts.push(format!("# Branch History\n{}", entries.join("\n\n")));
    }
    parts.join("\n\n")
}

fn join_preserved_and_history(preserved: &str, history: &str) -> String {
    match (preserved.is_empty(), history.is_empty()) {
        (true, true) => String::new(),
        (false, true) => preserved.to_string(),
        (true, false) => history.to_string(),
        (false, false) => format!("{preserved}\n\n{history}"),
    }
}

fn tail_chars_with_boundary(s: &str, max_chars: usize) -> &str {
    if max_chars == 0 {
        return "";
    }
    let total_chars = s.chars().count();
    if total_chars <= max_chars {
        return s;
    }
    let skip = total_chars - max_chars;
    let mut start = s
        .char_indices()
        .nth(skip)
        .map(|(idx, _)| idx)
        .unwrap_or(s.len());
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

fn trim_single_node_section(
    header: &str,
    node: &ConversationNode,
    history_budget: usize,
) -> String {
    let header_chars = header.chars().count();
    if history_budget <= header_chars {
        return String::new();
    }
    let entry_budget = history_budget - header_chars;
    let role_prefix = format!("[{}]\n", node.role);
    let role_chars = role_prefix.chars().count();
    if entry_budget > role_chars {
        let tail = tail_chars_with_boundary(&node.content, entry_budget - role_chars);
        format!("{header}{role_prefix}{tail}")
    } else {
        let formatted = format_node_entry(node);
        let tail = tail_chars_with_boundary(&formatted, entry_budget);
        format!("{header}{tail}")
    }
}

pub fn format_whiteboard_delta_prefix(blackboard: &Blackboard) -> String {
    let md = blackboard.to_markdown();
    if md.trim().is_empty() {
        String::new()
    } else {
        format!("[Shared Whiteboard Updated]\n{}\n\n---\n\n", md.trim())
    }
}

/// Builds a context payload for a given thread with empty/default blackboard notes.
pub fn build_context_prompt(thread_id: ThreadId) -> Result<ContextPayload, CoreError> {
    Ok(ContextPayload {
        thread_id: Some(thread_id),
        ..ContextPayload::default()
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
        ..ContextPayload::default()
    })
}

/// Marker identifying decision landmark turns committed back to Trunk.
const DECISION_LANDMARK_MARKER: &str = "Decision Landmark";

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
            ..ContextPayload::default()
        })
    }

    pub fn build_context_prompt_with_blackboard(
        &self,
        thread_id: ThreadId,
        blackboard: &Blackboard,
    ) -> Result<ContextPayload, CoreError> {
        build_context_prompt_with_blackboard(thread_id, &self.base_prompt, blackboard)
    }

    /// Compiles the full context for a thread: the Trunk (Root -> `parent_node_id`),
    /// the anchor header, the branch-local turns (`parent_node_id` -> `active_leaf_id`,
    /// excluding sibling branches), decision landmarks, and blackboard notes.
    pub fn compile_thread_context(
        &self,
        storage: &impl DagStorage,
        thread_id: ThreadId,
        blackboard: &Blackboard,
    ) -> Result<ContextPayload, CoreError> {
        let thread = storage
            .get_thread(&thread_id.0)?
            .ok_or_else(|| CoreError::ThreadNotFound(thread_id.to_string()))?;

        let trunk_path = storage.get_trunk_path(&thread.parent_node_id.0)?;

        let anchor_header = storage.get_anchor(&thread.anchor_id.0)?.map(|anchor| {
            format!(
                "{} {}",
                "#".repeat(usize::from(anchor.level.max(1))),
                anchor.title
            )
        });

        let leaf_path = storage.get_trunk_path(&thread.active_leaf_id.0)?;
        let branch_turns: Vec<ConversationNode> = match leaf_path
            .iter()
            .position(|node| node.id == thread.parent_node_id)
        {
            Some(idx) => leaf_path.into_iter().skip(idx + 1).collect(),
            None => {
                return Err(CoreError::SyncError(format!(
                    "active leaf {} is not a descendant of thread parent node {}",
                    thread.active_leaf_id, thread.parent_node_id
                )))
            }
        };

        let decision_landmarks: Vec<String> = trunk_path
            .iter()
            .chain(branch_turns.iter())
            .filter(|node| node.content.contains(DECISION_LANDMARK_MARKER))
            .map(|node| node.content.clone())
            .collect();

        let active_files = blackboard
            .find_by_kind(BlackboardEntryKind::ActiveFile)
            .iter()
            .map(|entry| entry.value.clone())
            .collect();

        Ok(ContextPayload {
            thread_id: Some(thread_id),
            base_system_prompt: self.base_prompt.clone(),
            blackboard_notes: blackboard.to_markdown(),
            active_files,
            decision_landmarks,
            trunk_path,
            anchor_header,
            branch_turns,
        })
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
