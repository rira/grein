use crate::error::CoreError;
pub use crate::model::BlackboardEntryId;
use crate::model::{AuthorRole, ConversationNode, NodeId, ThreadId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BlackboardEntryKind {
    #[default]
    Decision,
    Invariant,
    PortMapping,
    ActiveFile,
    Note,
}

impl BlackboardEntryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Decision => "decision",
            Self::Invariant => "invariant",
            Self::PortMapping => "port_mapping",
            Self::ActiveFile => "active_file",
            Self::Note => "note",
        }
    }
}

impl std::fmt::Display for BlackboardEntryKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for BlackboardEntryKind {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "decision" => Ok(Self::Decision),
            "invariant" => Ok(Self::Invariant),
            "port_mapping" => Ok(Self::PortMapping),
            "active_file" => Ok(Self::ActiveFile),
            "note" => Ok(Self::Note),
            other => Err(CoreError::Serialization(serde::de::Error::custom(format!(
                "Unknown blackboard entry kind: {other}"
            )))),
        }
    }
}

pub type BlackboardCategory = BlackboardEntryKind;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlackboardEntry {
    pub id: BlackboardEntryId,
    pub key: String,
    pub value: String,
    pub kind: BlackboardEntryKind,
    pub created_at: i64,
}

impl BlackboardEntry {
    pub fn new(
        key: impl Into<String>,
        value: impl Into<String>,
        kind: BlackboardEntryKind,
    ) -> Self {
        let created_at = chrono::Utc::now().timestamp();
        Self {
            id: BlackboardEntryId::new_v7(),
            key: key.into(),
            value: value.into(),
            kind,
            created_at,
        }
    }

    pub fn with_id(
        id: impl Into<BlackboardEntryId>,
        key: impl Into<String>,
        value: impl Into<String>,
        kind: BlackboardEntryKind,
        created_at: i64,
    ) -> Self {
        Self {
            id: id.into(),
            key: key.into(),
            value: value.into(),
            kind,
            created_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundedScratchpad {
    content: String,
    max_chars: usize,
}

impl BoundedScratchpad {
    pub const DEFAULT_MAX_CHARS: usize = 4096;

    pub fn new(max_chars: usize) -> Self {
        Self {
            content: String::new(),
            max_chars,
        }
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn max_chars(&self) -> usize {
        self.max_chars
    }

    pub fn remaining_capacity(&self) -> usize {
        self.max_chars.saturating_sub(self.content.chars().count())
    }

    pub fn append(&mut self, text: &str) -> Result<(), CoreError> {
        let current_len = self.content.chars().count();
        let addition_len = text.chars().count();
        let attempted = current_len + addition_len;
        if attempted > self.max_chars {
            return Err(CoreError::ScratchpadLimitExceeded {
                limit: self.max_chars,
                attempted,
            });
        }
        self.content.push_str(text);
        Ok(())
    }

    pub fn set_content(&mut self, text: &str) -> Result<(), CoreError> {
        let attempted = text.chars().count();
        if attempted > self.max_chars {
            return Err(CoreError::ScratchpadLimitExceeded {
                limit: self.max_chars,
                attempted,
            });
        }
        self.content = text.to_string();
        Ok(())
    }

    pub fn clear(&mut self) {
        self.content.clear();
    }
}

impl Default for BoundedScratchpad {
    fn default() -> Self {
        Self::new(Self::DEFAULT_MAX_CHARS)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Blackboard {
    entries: Vec<BlackboardEntry>,
    scratchpad: BoundedScratchpad,
}

impl Blackboard {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            scratchpad: BoundedScratchpad::default(),
        }
    }

    pub fn with_scratchpad_bound(max_chars: usize) -> Self {
        Self {
            entries: Vec::new(),
            scratchpad: BoundedScratchpad::new(max_chars),
        }
    }

    pub fn from_entries(entries: impl IntoIterator<Item = BlackboardEntry>) -> Self {
        let mut blackboard = Self::new();
        for entry in entries {
            blackboard.add_entry(entry);
        }
        blackboard
    }

    pub fn entries(&self) -> &[BlackboardEntry] {
        &self.entries
    }

    pub fn add_entry(&mut self, entry: BlackboardEntry) {
        if let Some(pos) = self.entries.iter().position(|e| e.key == entry.key) {
            self.entries[pos] = entry;
        } else {
            self.entries.push(entry);
        }
    }

    pub fn get(&self, key: &str) -> Option<&BlackboardEntry> {
        self.entries.iter().find(|e| e.key == key)
    }

    pub fn find_by_kind(&self, kind: BlackboardEntryKind) -> Vec<&BlackboardEntry> {
        self.entries.iter().filter(|e| e.kind == kind).collect()
    }

    pub fn remove_entry(&mut self, key: &str) -> Option<BlackboardEntry> {
        if let Some(pos) = self.entries.iter().position(|e| e.key == key) {
            Some(self.entries.remove(pos))
        } else {
            None
        }
    }

    pub fn scratchpad(&self) -> &BoundedScratchpad {
        &self.scratchpad
    }

    pub fn scratchpad_mut(&mut self) -> &mut BoundedScratchpad {
        &mut self.scratchpad
    }

    pub fn add_decision(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.add_entry(BlackboardEntry::new(
            key,
            value,
            BlackboardEntryKind::Decision,
        ));
    }

    pub fn add_invariant(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.add_entry(BlackboardEntry::new(
            key,
            value,
            BlackboardEntryKind::Invariant,
        ));
    }

    pub fn register_active_file(&mut self, path: impl Into<String>) {
        let p = path.into();
        self.add_entry(BlackboardEntry::new(
            p.clone(),
            p,
            BlackboardEntryKind::ActiveFile,
        ));
    }

    pub fn set_port_mapping(&mut self, service: impl Into<String>, port: u16) {
        let s = service.into();
        self.add_entry(BlackboardEntry::new(
            s,
            port.to_string(),
            BlackboardEntryKind::PortMapping,
        ));
    }

    pub fn to_markdown(&self) -> String {
        let mut md = String::new();

        let decisions = self.find_by_kind(BlackboardEntryKind::Decision);
        if !decisions.is_empty() {
            md.push_str("## Active Architectural Decisions\n");
            for d in decisions {
                md.push_str(&format!("- **{}**: {}\n", d.key, d.value));
            }
        }

        let invariants = self.find_by_kind(BlackboardEntryKind::Invariant);
        if !invariants.is_empty() {
            if !md.is_empty() {
                md.push('\n');
            }
            md.push_str("## Established Domain Invariants\n");
            for inv in invariants {
                md.push_str(&format!("- **{}**: {}\n", inv.key, inv.value));
            }
        }

        let ports = self.find_by_kind(BlackboardEntryKind::PortMapping);
        if !ports.is_empty() {
            if !md.is_empty() {
                md.push('\n');
            }
            md.push_str("## Port Mappings\n");
            for p in ports {
                md.push_str(&format!("- **{}**: {}\n", p.key, p.value));
            }
        }

        let active_files = self.find_by_kind(BlackboardEntryKind::ActiveFile);
        if !active_files.is_empty() {
            if !md.is_empty() {
                md.push('\n');
            }
            md.push_str("## Active Files Registry\n");
            for f in active_files {
                md.push_str(&format!("- `{}`\n", f.value));
            }
        }

        let notes = self.find_by_kind(BlackboardEntryKind::Note);
        if !notes.is_empty() {
            if !md.is_empty() {
                md.push('\n');
            }
            md.push_str("## Notes\n");
            for n in notes {
                md.push_str(&format!("- **{}**: {}\n", n.key, n.value));
            }
        }

        if !self.scratchpad.content().is_empty() {
            if !md.is_empty() {
                md.push('\n');
            }
            md.push_str("## Scratchpad Notes\n");
            md.push_str(self.scratchpad.content());
            if !self.scratchpad.content().ends_with('\n') {
                md.push('\n');
            }
        }

        md
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeImport {
    pub source_thread: String,
    pub topic: String,
    pub conclusion: String,
}

impl KnowledgeImport {
    pub fn new(
        source_thread: impl Into<String>,
        topic: impl Into<String>,
        conclusion: impl Into<String>,
    ) -> Self {
        Self {
            source_thread: source_thread.into(),
            topic: topic.into(),
            conclusion: conclusion.into(),
        }
    }

    pub fn to_turn_content(&self) -> String {
        format!(
            "[Context Import from {} ({}): {}]",
            self.source_thread, self.topic, self.conclusion
        )
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let prefix = "[Context Import from ";
        if !raw.starts_with(prefix) || !raw.ends_with(']') {
            return None;
        }

        let inner = &raw[prefix.len()..raw.len() - 1];
        let paren_open = inner.find(" (")?;
        let source_thread = inner[..paren_open].trim();

        let after_paren = &inner[paren_open + 2..];
        let delim = "): ";
        let delim_pos = after_paren.find(delim)?;
        let topic = &after_paren[..delim_pos];
        let conclusion = &after_paren[delim_pos + delim.len()..];

        if source_thread.is_empty() || topic.is_empty() || conclusion.is_empty() {
            return None;
        }

        Some(Self::new(source_thread, topic, conclusion))
    }

    pub fn to_conversation_node(&self, parent_id: Option<NodeId>) -> ConversationNode {
        ConversationNode::new(
            NodeId::new_v7(),
            parent_id,
            AuthorRole::System,
            self.to_turn_content(),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionLandmark {
    pub thread_id: ThreadId,
    pub topic: String,
    pub summary: String,
}

impl DecisionLandmark {
    pub fn new(
        thread_id: impl Into<ThreadId>,
        topic: impl Into<String>,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            thread_id: thread_id.into(),
            topic: topic.into(),
            summary: summary.into(),
        }
    }

    pub fn to_turn_content(&self) -> String {
        format!(
            "[Decision Landmark (Thread {} - {})]:\n{}",
            self.thread_id, self.topic, self.summary
        )
    }

    pub fn to_conversation_node(&self, parent_id: Option<NodeId>) -> ConversationNode {
        ConversationNode::new(
            NodeId::new_v7(),
            parent_id,
            AuthorRole::System,
            self.to_turn_content(),
        )
    }
}
