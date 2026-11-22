use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::io::IsTerminal;
use thiserror::Error;

use crate::engine_port::{HydratedBlackboardEntry, SubagentSnapshot, ThreadMeta};
use crate::transcript::TranscriptViewport;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TuiError {
    #[error("Terminal I/O error: {0}")]
    Io(String),
}

impl From<std::io::Error> for TuiError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum FocusedPane {
    Tree,
    Transcript,
    #[default]
    Composer,
    Blackboard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FocusDirection {
    Forward,
    Backward,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPermission {
    pub request_id: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppAction {
    Scrolled { delta: i32 },
    None,
    SendPrompt {
        thread_id: String,
        prompt: String,
    },
    RunShell {
        thread_id: String,
        command: String,
    },
    ForkBranch {
        source_thread_id: String,
        title: Option<String>,
    },
    CreateRoot {
        title: Option<String>,
    },
    RenameThread {
        thread_id: String,
        title: String,
    },
    MergeBranch {
        source_thread_id: String,
        target_thread_id: String,
        summary: Option<String>,
    },
    DeleteThread(String),
    UpsertBlackboard {
        key: String,
        value: String,
    },
    DeleteBlackboard {
        key: String,
    },
    CancelTurn {
        thread_id: String,
    },
    ResolvePermission {
        request_id: String,
        approved: bool,
    },
}

pub trait TuiComponent {
    fn is_focused(&self) -> bool;
    fn handle_event(&mut self, event: &AppEvent) -> bool;
    fn dispatch_key(&mut self, event: &AppEvent) -> Option<AppAction>;
}

#[allow(non_upper_case_globals)]
impl FocusedPane {
    pub const Sidebar: Self = Self::Tree;
    pub const Inspector: Self = Self::Blackboard;
    pub const Stream: Self = Self::Transcript;
    pub const Chat: Self = Self::Transcript;
    pub const Governance: Self = Self::Blackboard;
    pub const Details: Self = Self::Blackboard;
    pub const Sessions: Self = Self::Tree;
    pub const Diff: Self = Self::Blackboard;
    pub const Workspace: Self = Self::Tree;
    pub const Output: Self = Self::Transcript;

    pub fn next(&self, direction: FocusDirection) -> Self {
        match direction {
            FocusDirection::Forward => match self {
                Self::Tree => Self::Transcript,
                Self::Transcript => Self::Composer,
                Self::Composer => Self::Blackboard,
                Self::Blackboard => Self::Tree,
            },
            FocusDirection::Backward => match self {
                Self::Tree => Self::Blackboard,
                Self::Blackboard => Self::Composer,
                Self::Composer => Self::Transcript,
                Self::Transcript => Self::Tree,
            },
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            Self::Tree => Self::Blackboard,
            Self::Blackboard => Self::Composer,
            Self::Composer => Self::Transcript,
            Self::Transcript => Self::Tree,
        }
    }
}

impl From<crate::focus::PaneFocus> for FocusedPane {
    fn from(p: crate::focus::PaneFocus) -> Self {
        match p {
            crate::focus::PaneFocus::Composer => Self::Composer,
            crate::focus::PaneFocus::Sidebar | crate::focus::PaneFocus::Sessions => Self::Tree,
            crate::focus::PaneFocus::Transcript | crate::focus::PaneFocus::Chat => Self::Transcript,
            crate::focus::PaneFocus::Inspector
            | crate::focus::PaneFocus::Diff
            | crate::focus::PaneFocus::Details => Self::Blackboard,
        }
    }
}

impl PartialEq<crate::focus::PaneFocus> for FocusedPane {
    fn eq(&self, other: &crate::focus::PaneFocus) -> bool {
        let other_fp: FocusedPane = (*other).into();
        *self == other_fp
    }
}

impl PartialEq<FocusedPane> for crate::focus::PaneFocus {
    fn eq(&self, other: &FocusedPane) -> bool {
        let self_fp: FocusedPane = (*self).into();
        self_fp == *other
    }
}

pub type FocusPane = FocusedPane;
pub type PaneFocus = FocusedPane;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeAction {
    ActivateAnchor(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AnchorItem {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ThreadItem {
    pub id: String,
    pub title: String,
    pub anchors: Vec<AnchorItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppConfig {
    pub poll_interval: std::time::Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    Key(KeyEvent),
    Tick,
    Quit,
    FocusChanged(FocusedPane),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    pub focused_pane: FocusedPane,
    pub focus: FocusedPane,
    pub active_focus: FocusedPane,
    pub blackboard_collapsed: bool,
    pub blackboard_notes: String,
    pub blackboard_entries: Vec<HydratedBlackboardEntry>,
    pub blackboard_selected: usize,
    pub input_buffer: String,
    pub cursor_position: usize,
    pub kill_ring: String,
    pub should_quit: bool,
    pub active_transcript_md: String,
    pub threads: Vec<ThreadItem>,
    pub tree: crate::ui::tree::TreeState,
    pub blackboard_pane: crate::panes::blackboard::BlackboardPane,
    pub last_submitted_prompt: Option<String>,
    pub active_thread_id: String,
    pub thread_transcripts: BTreeMap<String, String>,
    pub thread_statuses: BTreeMap<String, String>,
    pub streaming_assistant_threads: BTreeSet<String>,
    pub thread_thoughts: BTreeMap<String, String>,
    pub thread_viewports: BTreeMap<String, TranscriptViewport>,
    pub thread_drafts: BTreeMap<String, String>,
    pub pending_permission: Option<PendingPermission>,
    pub pending_permissions: VecDeque<PendingPermission>,
    pub pending_actions: VecDeque<AppAction>,
    pub config: AppConfig,
    pub thread_meta: HashMap<String, ThreadMeta>,
    pub thread_subagents: HashMap<String, Vec<SubagentSnapshot>>,
    pub thread_agent_meta: HashMap<String, (Option<String>, Option<String>)>,
    pub thread_active_tool: HashMap<String, Option<String>>,
    pub thread_turn_started_ms: HashMap<String, u64>,
    pub tick_count: u64,
    pub show_help_modal: bool,
    pub status_notice: Option<String>,
    pub pending_rename_thread_id: Option<String>,
}

pub type AppState = App;

impl Default for App {
    fn default() -> Self {
        Self {
            focused_pane: FocusedPane::Composer,
            focus: FocusedPane::Composer,
            active_focus: FocusedPane::Composer,
            blackboard_collapsed: false,
            blackboard_notes: String::new(),
            blackboard_entries: Vec::new(),
            blackboard_selected: 0,
            input_buffer: String::new(),
            cursor_position: 0,
            kill_ring: String::new(),
            should_quit: false,
            active_transcript_md: String::new(),
            threads: Vec::new(),
            tree: crate::ui::tree::TreeState::default(),
            blackboard_pane: crate::panes::blackboard::BlackboardPane::new(),
            last_submitted_prompt: None,
            active_thread_id: "trunk".to_string(),
            thread_transcripts: BTreeMap::new(),
            thread_statuses: BTreeMap::new(),
            streaming_assistant_threads: BTreeSet::new(),
            thread_thoughts: BTreeMap::new(),
            thread_viewports: BTreeMap::new(),
            thread_drafts: BTreeMap::new(),
            pending_permission: None,
            pending_permissions: VecDeque::new(),
            pending_actions: VecDeque::new(),
            config: AppConfig::default(),
            thread_meta: HashMap::new(),
            thread_subagents: HashMap::new(),
            thread_agent_meta: HashMap::new(),
            thread_active_tool: HashMap::new(),
            thread_turn_started_ms: HashMap::new(),
            tick_count: 0,
            show_help_modal: false,
            status_notice: None,
            pending_rename_thread_id: None,
        }
    }
}

fn clamp_char_boundary(buf: &str, pos: usize) -> usize {
    let mut idx = pos.min(buf.len());
    while idx > 0 && !buf.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

pub fn line_start(buf: &str, pos: usize) -> usize {
    let pos = clamp_char_boundary(buf, pos);
    match buf[..pos].rfind('\n') {
        Some(nl) => nl + 1,
        None => 0,
    }
}

pub fn line_end(buf: &str, pos: usize) -> usize {
    let pos = clamp_char_boundary(buf, pos);
    match buf[pos..].find('\n') {
        Some(rel) => pos + rel,
        None => buf.len(),
    }
}

pub fn prev_word_boundary(buf: &str, pos: usize) -> usize {
    let mut idx = clamp_char_boundary(buf, pos);
    let mut iter = buf[..idx].char_indices().rev().peekable();
    while let Some(&(byte_idx, ch)) = iter.peek() {
        if is_word_char(ch) {
            break;
        }
        idx = byte_idx;
        iter.next();
    }
    while let Some(&(byte_idx, ch)) = iter.peek() {
        if !is_word_char(ch) {
            break;
        }
        idx = byte_idx;
        iter.next();
    }
    idx
}

pub fn next_word_boundary(buf: &str, pos: usize) -> usize {
    let mut idx = clamp_char_boundary(buf, pos);
    for ch in buf[idx..].chars() {
        if is_word_char(ch) {
            break;
        }
        idx += ch.len_utf8();
    }
    for ch in buf[idx..].chars() {
        if !is_word_char(ch) {
            break;
        }
        idx += ch.len_utf8();
    }
    idx
}

impl App {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(config: AppConfig) -> Self {
        Self {
            config,
            ..Self::default()
        }
    }

    pub fn active_thread_id(&self) -> &str {
        &self.active_thread_id
    }

    pub fn switch_thread(&mut self, thread_id: impl Into<String>) {
        let target_thread = thread_id.into();
        self.pending_rename_thread_id = None;
        self.thread_drafts
            .insert(self.active_thread_id.clone(), self.input_buffer.clone());
        self.thread_transcripts.insert(
            self.active_thread_id.clone(),
            self.active_transcript_md.clone(),
        );
        self.active_thread_id = target_thread;
        let restored = self
            .thread_drafts
            .get(&self.active_thread_id)
            .cloned()
            .unwrap_or_default();
        self.input_buffer = restored;
        self.cursor_position = self.input_buffer.len();
        self.active_transcript_md = self
            .thread_transcripts
            .get(&self.active_thread_id)
            .cloned()
            .unwrap_or_default();
        self.thread_viewports
            .entry(self.active_thread_id.clone())
            .or_default();
        self.sync_anchors_and_blackboard();
        self.select_active_thread_in_tree();
    }

    pub fn select_active_thread_in_tree(&mut self) {
        if let Some(idx) = self.tree.nodes.iter().position(|n| {
            matches!(n, crate::ui::tree::TreeNode::Thread(t) if t.id == self.active_thread_id)
        }) {
            self.tree.select_index(idx);
        }
    }

    /// Queues thread deletion or records a status notice when only a single context remains.
    pub fn request_delete_thread(&mut self, thread_id: impl Into<String>) {
        if self.threads.len() <= 1 {
            self.status_notice = Some("Cannot delete the only context".to_string());
            return;
        }
        self.status_notice = None;
        self.pending_actions
            .push_back(AppAction::DeleteThread(thread_id.into()));
    }

    pub fn sync_blackboard_notes(&mut self) {
        self.blackboard_notes = self
            .blackboard_entries
            .iter()
            .map(|e| format!("• {}: {}", e.key, e.value))
            .collect::<Vec<_>>()
            .join("\n");
        if self.blackboard_entries.is_empty() {
            self.blackboard_selected = 0;
        } else if self.blackboard_selected >= self.blackboard_entries.len() {
            self.blackboard_selected = self.blackboard_entries.len() - 1;
        }
    }

    pub fn upsert_blackboard_entry(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key = key.into();
        let value = value.into();
        if let Some(existing) = self.blackboard_entries.iter_mut().find(|e| e.key == key) {
            existing.value = value;
        } else {
            self.blackboard_entries
                .push(HydratedBlackboardEntry { key, value });
        }
        self.sync_blackboard_notes();
    }

    pub fn remove_blackboard_entry(&mut self, key: &str) {
        self.blackboard_entries.retain(|e| e.key != key);
        self.sync_blackboard_notes();
    }

    pub fn push_permission(&mut self, perm: PendingPermission) {
        if let Some(active) = &self.pending_permission {
            if active.request_id == perm.request_id {
                return;
            }
        }
        if self
            .pending_permissions
            .iter()
            .any(|p| p.request_id == perm.request_id)
        {
            return;
        }
        if self.pending_permission.is_none() {
            self.pending_permission = Some(perm);
        } else {
            self.pending_permissions.push_back(perm);
        }
    }

    pub fn resolve_active_permission(&mut self, approved: bool) {
        if let Some(perm) = self.pending_permission.take() {
            self.pending_actions
                .push_back(AppAction::ResolvePermission {
                    request_id: perm.request_id,
                    approved,
                });
            self.pending_permission = self.pending_permissions.pop_front();
        }
    }

    pub fn transcript_viewport(&self, thread_id: &str) -> TranscriptViewport {
        self.thread_viewports
            .get(thread_id)
            .copied()
            .unwrap_or_default()
    }

    pub fn transcript_viewport_mut(&mut self, thread_id: &str) -> &mut TranscriptViewport {
        self.thread_viewports
            .entry(thread_id.to_string())
            .or_default()
    }

    pub fn active_transcript_scroll_offset(&self) -> usize {
        self.transcript_viewport(&self.active_thread_id)
            .scroll_offset()
    }

    fn sync_focus_in(&mut self) {
        if self.focus != self.focused_pane {
            if self.focus != FocusedPane::Composer {
                self.focused_pane = self.focus;
            } else {
                self.focus = self.focused_pane;
            }
        }
        if self.active_focus != self.focused_pane {
            if self.active_focus != FocusedPane::Composer {
                self.focused_pane = self.active_focus;
            } else {
                self.active_focus = self.focused_pane;
            }
        }
    }

    fn sync_focus_out(&mut self) {
        self.focus = self.focused_pane;
        self.active_focus = self.focused_pane;
    }

    pub fn focused_pane(&self) -> FocusedPane {
        self.focused_pane
    }

    pub fn focus(&self) -> FocusedPane {
        self.focus
    }

    pub fn active_focus(&self) -> FocusedPane {
        self.active_focus
    }

    pub fn set_focus(&mut self, pane: impl Into<FocusedPane>) {
        self.focused_pane = pane.into();
        self.sync_focus_out();
    }

    pub fn set_focused_pane(&mut self, pane: impl Into<FocusedPane>) {
        self.set_focus(pane);
    }

    pub fn is_running(&self) -> bool {
        !self.should_quit
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    pub fn composer_input(&self) -> &str {
        &self.input_buffer
    }

    pub fn composer_content(&self) -> &str {
        &self.input_buffer
    }

    pub fn composer_line_count(&self) -> u16 {
        self.input_buffer.split('\n').count().max(1) as u16
    }

    pub fn composer_cursor_2d(&self) -> (u16, u16) {
        let clamped_pos = clamp_char_boundary(&self.input_buffer, self.cursor_position);
        let prefix = &self.input_buffer[..clamped_pos];
        let row = prefix.bytes().filter(|&b| b == b'\n').count() as u16;
        let col = match prefix.rfind('\n') {
            Some(idx) => prefix[idx + 1..].chars().count() as u16,
            None => prefix.chars().count() as u16,
        };
        (col, row)
    }

    pub fn last_submitted_prompt(&self) -> Option<String> {
        self.last_submitted_prompt.clone()
    }

    pub fn poll_events(&mut self, timeout: std::time::Duration) -> Result<(), TuiError> {
        if !std::io::stdin().is_terminal() {
            return Ok(());
        }
        if crossterm::event::poll(timeout)? {
            if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
                self.handle_key_event(key);
            }
        }
        Ok(())
    }

    pub fn is_blackboard_collapsed(&self) -> bool {
        self.blackboard_collapsed || self.blackboard_pane.is_collapsed()
    }

    pub fn set_blackboard_collapsed(&mut self, collapsed: bool) {
        self.blackboard_collapsed = collapsed;
        if collapsed {
            self.blackboard_pane.collapse();
            if self.focused_pane == FocusedPane::Blackboard {
                self.focused_pane = FocusedPane::Tree;
            }
        } else {
            self.blackboard_pane.expand();
        }
        self.sync_focus_out();
    }

    pub fn blackboard_pane(&self) -> &crate::panes::blackboard::BlackboardPane {
        &self.blackboard_pane
    }

    pub fn blackboard_pane_mut(&mut self) -> &mut crate::panes::blackboard::BlackboardPane {
        &mut self.blackboard_pane
    }

    pub fn cycle_focus(&mut self) {
        self.focus_next();
    }

    pub fn cycle_focus_reverse(&mut self) {
        self.focus_prev();
    }

    pub fn focus_next(&mut self) {
        self.sync_focus_in();
        let is_collapsed = self.blackboard_collapsed || self.blackboard_pane.is_collapsed();
        self.focused_pane = match self.focused_pane {
            FocusedPane::Tree => FocusedPane::Transcript,
            FocusedPane::Transcript => FocusedPane::Composer,
            FocusedPane::Composer => {
                if is_collapsed {
                    FocusedPane::Tree
                } else {
                    FocusedPane::Blackboard
                }
            }
            FocusedPane::Blackboard => FocusedPane::Tree,
        };
        self.sync_focus_out();
    }

    pub fn next_pane(&mut self) {
        self.focus_next();
    }

    pub fn focus_prev(&mut self) {
        self.sync_focus_in();
        let is_collapsed = self.blackboard_collapsed || self.blackboard_pane.is_collapsed();
        self.focused_pane = match self.focused_pane {
            FocusedPane::Tree => {
                if is_collapsed {
                    FocusedPane::Composer
                } else {
                    FocusedPane::Blackboard
                }
            }
            FocusedPane::Blackboard => FocusedPane::Composer,
            FocusedPane::Composer => FocusedPane::Transcript,
            FocusedPane::Transcript => FocusedPane::Tree,
        };
        self.sync_focus_out();
    }

    pub fn prev_pane(&mut self) {
        self.focus_prev();
    }

    pub fn focus_previous(&mut self) {
        self.focus_prev();
    }

    pub fn toggle_blackboard(&mut self) {
        self.sync_focus_in();
        let collapsed = !self.is_blackboard_collapsed();
        self.set_blackboard_collapsed(collapsed);
    }

    pub fn handle_event(&mut self, event: AppEvent) -> bool {
        match event {
            AppEvent::Key(k) => self.handle_key_event(k),
            AppEvent::Tick => {
                self.tick_count = self.tick_count.wrapping_add(1);
                false
            }
            AppEvent::Quit => {
                self.should_quit = true;
                true
            }
            AppEvent::FocusChanged(pane) => {
                self.set_focus(pane);
                true
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        self.handle_key_event(key)
    }

    fn thread_title_or_empty(&self, thread_id: &str) -> String {
        self.threads
            .iter()
            .find(|t| t.id == thread_id)
            .map(|t| t.title.clone())
            .unwrap_or_default()
    }

    fn selected_tree_thread_info(&self) -> (String, String) {
        let selected_id = self
            .tree
            .selected_index()
            .and_then(|idx| self.tree.nodes.get(idx))
            .map(|node| match node {
                crate::ui::tree::TreeNode::Thread(t) => t.id.clone(),
                crate::ui::tree::TreeNode::Anchor(a) => a.thread_id.clone(),
            })
            .unwrap_or_else(|| self.active_thread_id.clone());
        let title = self
            .tree
            .selected_index()
            .and_then(|idx| self.tree.nodes.get(idx))
            .and_then(|node| match node {
                crate::ui::tree::TreeNode::Thread(t) => Some(t.title.clone()),
                crate::ui::tree::TreeNode::Anchor(_) => None,
            })
            .unwrap_or_else(|| self.thread_title_or_empty(&selected_id));
        (selected_id, title)
    }

    pub(crate) fn resolve_merge_target(&self, source_thread_id: &str) -> String {
        self.thread_meta
            .get(source_thread_id)
            .and_then(|m| m.parent_thread_id.as_deref())
            .unwrap_or("")
            .to_string()
    }

    fn begin_rename_prefill(&mut self) {
        let (target_id, target_title) = if self.focused_pane == FocusedPane::Tree {
            self.selected_tree_thread_info()
        } else {
            let id = self.active_thread_id.clone();
            let title = self.thread_title_or_empty(&id);
            (id, title)
        };
        self.pending_rename_thread_id = Some(target_id);
        self.input_buffer = format!("/rename {target_title}");
        self.cursor_position = self.input_buffer.len();
        self.focused_pane = FocusedPane::Composer;
        self.sync_focus_out();
    }

    pub fn handle_key_tree(&mut self, key: KeyEvent) -> Option<TreeAction> {
        self.sync_focus_in();
        if self.focused_pane != FocusedPane::Tree {
            return None;
        }

        let no_ctrl_alt = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);

        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.tree.nodes.is_empty() {
                    let curr = self.tree.selected_index().unwrap_or(0);
                    if curr + 1 < self.tree.nodes.len() {
                        self.tree.select_index(curr + 1);
                    }
                }
                None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if !self.tree.nodes.is_empty() {
                    let curr = self.tree.selected_index().unwrap_or(0);
                    if curr > 0 {
                        self.tree.select_index(curr - 1);
                    }
                }
                None
            }
            KeyCode::Char('n') if no_ctrl_alt => {
                self.status_notice = None;
                self.pending_actions
                    .push_back(AppAction::CreateRoot { title: None });
                None
            }
            KeyCode::Char('r') if no_ctrl_alt => {
                self.begin_rename_prefill();
                None
            }
            KeyCode::Char('m') if no_ctrl_alt => {
                let (source_thread_id, _) = self.selected_tree_thread_info();
                let target_thread_id = self.resolve_merge_target(&source_thread_id);
                self.status_notice = None;
                self.pending_actions.push_back(AppAction::MergeBranch {
                    source_thread_id,
                    target_thread_id,
                    summary: None,
                });
                None
            }
            KeyCode::Char('d') | KeyCode::Char('x') if no_ctrl_alt => {
                let (target_id, _) = self.selected_tree_thread_info();
                self.request_delete_thread(target_id);
                None
            }
            KeyCode::Enter => {
                if let Some(idx) = self.tree.selected_index() {
                    match self.tree.nodes.get(idx).cloned() {
                        Some(crate::ui::tree::TreeNode::Anchor(anchor)) => {
                            let id = anchor.id;
                            let thread_id = anchor.thread_id;
                            if thread_id != self.active_thread_id
                                && self.threads.iter().any(|t| t.id == thread_id)
                            {
                                self.switch_thread(&thread_id);
                            }
                            self.tree.set_active_anchor_id(Some(id.clone()));
                            return Some(TreeAction::ActivateAnchor(id));
                        }
                        Some(crate::ui::tree::TreeNode::Thread(thread)) => {
                            self.switch_thread(&thread.id);
                            self.focused_pane = FocusedPane::Composer;
                            self.sync_focus_out();
                            return None;
                        }
                        None => {}
                    }
                }
                None
            }
            _ => None,
        }
    }

    fn handle_key_blackboard(&mut self, key: KeyEvent) -> bool {
        if self.is_blackboard_collapsed() {
            return false;
        }
        let no_ctrl_alt = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if no_ctrl_alt => {
                self.blackboard_selected = self.blackboard_selected.saturating_sub(1);
                self.sync_focus_out();
                true
            }
            KeyCode::Down | KeyCode::Char('j') if no_ctrl_alt => {
                if self.blackboard_selected + 1 < self.blackboard_entries.len() {
                    self.blackboard_selected += 1;
                }
                self.sync_focus_out();
                true
            }
            KeyCode::Char('a') if no_ctrl_alt => {
                self.input_buffer = "/wb set ".to_string();
                self.cursor_position = self.input_buffer.len();
                self.focused_pane = FocusedPane::Composer;
                self.sync_focus_out();
                true
            }
            KeyCode::Char('e') if no_ctrl_alt => {
                if let Some(entry) = self.blackboard_entries.get(self.blackboard_selected) {
                    self.input_buffer = format!("/wb set {} {}", entry.key, entry.value);
                    self.cursor_position = self.input_buffer.len();
                    self.focused_pane = FocusedPane::Composer;
                    self.sync_focus_out();
                    true
                } else {
                    false
                }
            }
            KeyCode::Char('d') if no_ctrl_alt => {
                if let Some(entry) = self.blackboard_entries.get(self.blackboard_selected) {
                    let key = entry.key.clone();
                    self.pending_actions
                        .push_back(AppAction::DeleteBlackboard { key });
                    self.sync_focus_out();
                    true
                } else {
                    false
                }
            }
            _ => self.blackboard_pane.handle_key(key),
        }
    }

    fn kill_range(&mut self, start: usize, end: usize) {
        let start = clamp_char_boundary(&self.input_buffer, start);
        let end = clamp_char_boundary(&self.input_buffer, end);
        if end > start {
            self.kill_ring = self.input_buffer.drain(start..end).collect();
        }
        self.cursor_position = start;
    }

    fn handle_key_composer(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let pos = clamp_char_boundary(&self.input_buffer, self.cursor_position);

        match (key.code, ctrl, alt) {
            (KeyCode::Enter, _, _) => {
                if key
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT)
                {
                    self.input_buffer.insert(pos, '\n');
                    self.cursor_position = pos + 1;
                } else {
                    let prompt = std::mem::take(&mut self.input_buffer);
                    self.cursor_position = 0;
                    self.submit_composer_prompt(prompt);
                }
                true
            }
            (KeyCode::Char('j' | 'J'), true, false) => {
                self.input_buffer.insert(pos, '\n');
                self.cursor_position = pos + 1;
                true
            }
            (KeyCode::Char('a' | 'A'), true, false) | (KeyCode::Home, false, false) => {
                self.cursor_position = line_start(&self.input_buffer, pos);
                true
            }
            (KeyCode::Char('e' | 'E'), true, false) | (KeyCode::End, false, false) => {
                self.cursor_position = line_end(&self.input_buffer, pos);
                true
            }
            (KeyCode::Char('f' | 'F'), true, false) => {
                self.cursor_position = match self.input_buffer[pos..].chars().next() {
                    Some(ch) => pos + ch.len_utf8(),
                    None => pos,
                };
                true
            }
            (KeyCode::Char('d' | 'D'), true, false) => {
                self.cursor_position = pos;
                if pos < self.input_buffer.len() {
                    self.input_buffer.remove(pos);
                }
                true
            }
            (KeyCode::Char('k' | 'K'), true, false) => {
                let end = line_end(&self.input_buffer, pos);
                let kill_end = if pos < end {
                    end
                } else if pos < self.input_buffer.len() {
                    pos + 1
                } else {
                    pos
                };
                self.kill_range(pos, kill_end);
                true
            }
            (KeyCode::Char('u' | 'U'), true, false) => {
                let start = line_start(&self.input_buffer, pos);
                let kill_start = if pos > start {
                    start
                } else if pos > 0 {
                    pos - 1
                } else {
                    pos
                };
                self.kill_range(kill_start, pos);
                true
            }
            (KeyCode::Char('w' | 'W'), true, false) | (KeyCode::Backspace, false, true) => {
                let start = prev_word_boundary(&self.input_buffer, pos);
                self.kill_range(start, pos);
                true
            }
            (KeyCode::Char('d' | 'D'), false, true)
            | (KeyCode::Delete, true, false)
            | (KeyCode::Delete, false, true) => {
                let end = next_word_boundary(&self.input_buffer, pos);
                self.kill_range(pos, end);
                true
            }
            (KeyCode::Left, true, false)
            | (KeyCode::Left, false, true)
            | (KeyCode::Char('b' | 'B'), false, true) => {
                self.cursor_position = prev_word_boundary(&self.input_buffer, pos);
                true
            }
            (KeyCode::Right, true, false)
            | (KeyCode::Right, false, true)
            | (KeyCode::Char('f' | 'F'), false, true) => {
                self.cursor_position = next_word_boundary(&self.input_buffer, pos);
                true
            }
            (KeyCode::Char('y' | 'Y'), true, false) => {
                if !self.kill_ring.is_empty() {
                    self.input_buffer.insert_str(pos, &self.kill_ring);
                    self.cursor_position = pos + self.kill_ring.len();
                } else {
                    self.cursor_position = pos;
                }
                true
            }
            (KeyCode::Char(c), false, false) => {
                self.input_buffer.insert(pos, c);
                self.cursor_position = pos + c.len_utf8();
                true
            }
            (KeyCode::Backspace, false, false) if pos > 0 => {
                if let Some((idx, _)) = self.input_buffer[..pos].char_indices().next_back() {
                    self.input_buffer.remove(idx);
                    self.cursor_position = idx;
                    true
                } else {
                    false
                }
            }
            (KeyCode::Delete, false, false) if pos < self.input_buffer.len() => {
                self.input_buffer.remove(pos);
                self.cursor_position = pos;
                true
            }
            (KeyCode::Left, false, false) if pos > 0 => {
                if let Some((idx, _)) = self.input_buffer[..pos].char_indices().next_back() {
                    self.cursor_position = idx;
                    true
                } else {
                    false
                }
            }
            (KeyCode::Right, false, false) if pos < self.input_buffer.len() => {
                if let Some(ch) = self.input_buffer[pos..].chars().next() {
                    self.cursor_position = pos + ch.len_utf8();
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) -> bool {
        self.sync_focus_in();

        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(
                key.code,
                KeyCode::Char('c') | KeyCode::Char('q') | KeyCode::Char('Q')
            )
        {
            self.should_quit = true;
            self.sync_focus_out();
            return true;
        }

        if self.pending_permission.is_some() {
            let no_ctrl_alt = !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter if no_ctrl_alt => {
                    self.resolve_active_permission(true);
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc if no_ctrl_alt => {
                    self.resolve_active_permission(false);
                }
                _ => {}
            }
            self.sync_focus_out();
            return true;
        }

        if self.show_help_modal {
            let no_ctrl_alt = !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
            match key.code {
                KeyCode::Esc | KeyCode::Enter => {
                    self.show_help_modal = false;
                }
                KeyCode::Char('q') | KeyCode::Char('?') if no_ctrl_alt => {
                    self.show_help_modal = false;
                }
                _ => {}
            }
            self.sync_focus_out();
            return true;
        }

        if key.code == KeyCode::Char('?')
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && (self.focused_pane != FocusedPane::Composer || self.input_buffer.is_empty())
        {
            self.show_help_modal = true;
            self.sync_focus_out();
            return true;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT)
        {
            match key.code {
                KeyCode::Char('b') | KeyCode::Char('B') => {
                    self.toggle_blackboard();
                    return true;
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    self.pending_actions.push_back(AppAction::ForkBranch {
                        source_thread_id: self.active_thread_id.clone(),
                        title: None,
                    });
                    self.sync_focus_out();
                    return true;
                }
                _ => {}
            }
        }

        if key.modifiers.contains(KeyModifiers::ALT)
            && !key.modifiers.contains(KeyModifiers::CONTROL)
        {
            match key.code {
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    self.status_notice = None;
                    self.pending_actions
                        .push_back(AppAction::CreateRoot { title: None });
                    self.sync_focus_out();
                    return true;
                }
                KeyCode::Char('r') | KeyCode::Char('R') => {
                    self.begin_rename_prefill();
                    return true;
                }
                _ => {}
            }
        }

        if key.code == KeyCode::F(2)
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            self.begin_rename_prefill();
            return true;
        }

        match key.code {
            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.focus_prev();
                } else {
                    self.focus_next();
                }
                return true;
            }
            KeyCode::BackTab => {
                self.focus_prev();
                return true;
            }
            KeyCode::Esc => {
                let status = self
                    .thread_statuses
                    .get(&self.active_thread_id)
                    .map(String::as_str)
                    .unwrap_or("idle");
                if matches!(status, "streaming" | "executing" | "thinking") {
                    self.pending_actions.push_back(AppAction::CancelTurn {
                        thread_id: self.active_thread_id.clone(),
                    });
                    self.sync_focus_out();
                    return true;
                }
                if self.focused_pane == FocusedPane::Composer {
                    self.pending_rename_thread_id = None;
                    self.focused_pane = FocusedPane::Tree;
                    self.sync_focus_out();
                    return true;
                }
            }
            _ if self.focused_pane == FocusedPane::Composer => {
                let handled = self.handle_key_composer(key);
                self.sync_focus_out();
                return handled;
            }
            KeyCode::Char('[') => {
                self.set_blackboard_collapsed(true);
                return true;
            }
            KeyCode::Char(']') => {
                self.set_blackboard_collapsed(false);
                return true;
            }
            KeyCode::Char('q') => {
                self.should_quit = true;
                self.sync_focus_out();
                return true;
            }
            KeyCode::Up if self.focused_pane == FocusedPane::Transcript => {
                let active = self.active_thread_id.clone();
                self.transcript_viewport_mut(&active).scroll_up(1);
                self.sync_focus_out();
                return true;
            }
            KeyCode::Down if self.focused_pane == FocusedPane::Transcript => {
                let active = self.active_thread_id.clone();
                self.transcript_viewport_mut(&active).scroll_down(1);
                self.sync_focus_out();
                return true;
            }
            KeyCode::PageUp if self.focused_pane == FocusedPane::Transcript => {
                let active = self.active_thread_id.clone();
                self.transcript_viewport_mut(&active).scroll_up(10);
                self.sync_focus_out();
                return true;
            }
            KeyCode::PageDown if self.focused_pane == FocusedPane::Transcript => {
                let active = self.active_thread_id.clone();
                self.transcript_viewport_mut(&active).scroll_down(10);
                self.sync_focus_out();
                return true;
            }
            KeyCode::Home if self.focused_pane == FocusedPane::Transcript => {
                let active = self.active_thread_id.clone();
                self.transcript_viewport_mut(&active).scroll_to_top();
                self.sync_focus_out();
                return true;
            }
            KeyCode::End if self.focused_pane == FocusedPane::Transcript => {
                let active = self.active_thread_id.clone();
                self.transcript_viewport_mut(&active).scroll_to_bottom();
                self.sync_focus_out();
                return true;
            }
            _ => {
                if self.focused_pane == FocusedPane::Tree {
                    let _ = self.handle_key_tree(key);
                    self.sync_focus_out();
                    return true;
                }
                if self.focused_pane == FocusedPane::Blackboard {
                    return self.handle_key_blackboard(key);
                }
            }
        }

        self.sync_focus_out();
        false
    }

    pub fn render(&mut self, _frame: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        if self.is_blackboard_collapsed() {
            self.blackboard_pane
                .set_rendered_rect(ratatui::layout::Rect::default());
        } else {
            let layout = crate::layout::compute_layout(area, false);
            if let Some(r) = layout.blackboard_area {
                self.blackboard_pane.set_rendered_rect(r);
            } else {
                self.blackboard_pane.set_rendered_rect(area);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn test_initial_focus_is_composer() {
        let app = App::default();
        assert_eq!(app.focused_pane(), FocusedPane::Composer);
        assert_eq!(app.focus, FocusedPane::Composer);
    }

    #[test]
    fn test_tab_cycles_focus_forward() {
        let mut app = App::default();
        assert_eq!(app.focused_pane(), FocusedPane::Composer);

        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Blackboard);

        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Tree);

        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Transcript);

        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Composer);
    }

    #[test]
    fn test_backtab_cycles_focus_backward() {
        let mut app = App::default();
        assert_eq!(app.focused_pane(), FocusedPane::Composer);

        app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Transcript);

        app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Tree);

        app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Blackboard);

        app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Composer);
    }

    #[test]
    fn test_shift_tab_cycles_focus_backward() {
        let mut app = App::default();
        assert_eq!(app.focused_pane(), FocusedPane::Composer);

        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT));
        assert_eq!(app.focused_pane(), FocusedPane::Transcript);
    }

    #[test]
    fn test_collapsed_blackboard_skips_focus() {
        let mut app = App::default();
        app.set_blackboard_collapsed(true);

        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Tree);

        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Transcript);

        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Composer);

        app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Transcript);
    }

    #[test]
    fn test_ctrl_b_toggles_blackboard_collapse() {
        let mut app = App::default();
        assert!(!app.is_blackboard_collapsed());

        app.handle_key_event(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert!(app.is_blackboard_collapsed());

        app.handle_key_event(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert!(!app.is_blackboard_collapsed());
    }

    #[test]
    fn test_composer_typing_immunity_for_q_key() {
        let mut app = App::new();
        app.set_focus(FocusPane::Composer);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));

        assert!(!app.should_quit());
        assert_eq!(app.composer_content(), "q");
    }

    #[test]
    fn test_ctrl_q_and_ctrl_c_quit_from_composer() {
        let mut app1 = App::new();
        app1.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
        assert!(app1.should_quit());

        let mut app2 = App::new();
        app2.handle_key_event(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app2.should_quit());
    }

    #[test]
    fn test_composer_typing_immunity_for_bracket_keys() {
        let mut app = App::new();
        app.set_focus(FocusPane::Composer);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('['), KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));

        assert!(!app.is_blackboard_collapsed());
        assert_eq!(app.composer_content(), "[]");
    }

    #[test]
    fn test_composer_esc_returns_to_tree_focus_when_idle_and_cancels_when_streaming() {
        let mut app = App::new();
        app.focus = FocusPane::Composer;

        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.focus, FocusPane::Tree);

        app.set_focus(FocusPane::Composer);
        app.thread_statuses
            .insert(app.active_thread_id.clone(), "streaming".to_string());
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(
            app.pending_actions.pop_front(),
            Some(AppAction::CancelTurn {
                thread_id: app.active_thread_id.clone()
            })
        );
    }

    #[test]
    fn test_composer_input_routing() {
        let mut app = App::default();
        app.set_focus(FocusedPane::Tree);

        // Input ignored when not in Composer
        app.handle_key_event(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert_eq!(app.composer_input(), "");

        // Focus Composer
        app.set_focus(FocusedPane::Composer);
        app.handle_key_event(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
        assert_eq!(app.composer_input(), "hi");
        assert_eq!(app.cursor_position, 2);

        app.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(app.composer_input(), "h");
        assert_eq!(app.cursor_position, 1);
    }

    #[test]
    fn test_composer_command_parsing_dispatches_actions() {
        let mut app = App::default();
        app.active_thread_id = "t-main".to_string();

        app.input_buffer = "!cargo test".to_string();
        app.cursor_position = app.input_buffer.len();
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.active_transcript_md.contains("cargo test"));
        assert_eq!(
            app.pending_actions.pop_front(),
            Some(AppAction::RunShell {
                thread_id: "t-main".to_string(),
                command: "cargo test".to_string(),
            })
        );

        app.input_buffer = "/branch feature-x".to_string();
        app.cursor_position = app.input_buffer.len();
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.pending_actions.pop_front(),
            Some(AppAction::ForkBranch {
                source_thread_id: "t-main".to_string(),
                title: Some("feature-x".to_string()),
            })
        );

        app.input_buffer = "/fork".to_string();
        app.cursor_position = app.input_buffer.len();
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.pending_actions.pop_front(),
            Some(AppAction::ForkBranch {
                source_thread_id: "t-main".to_string(),
                title: None,
            })
        );

        app.input_buffer = "/wb set arch two-tier".to_string();
        app.cursor_position = app.input_buffer.len();
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.pending_actions.pop_front(),
            Some(AppAction::UpsertBlackboard {
                key: "arch".to_string(),
                value: "two-tier".to_string(),
            })
        );

        app.input_buffer = "/pin db sqlite".to_string();
        app.cursor_position = app.input_buffer.len();
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.pending_actions.pop_front(),
            Some(AppAction::UpsertBlackboard {
                key: "db".to_string(),
                value: "sqlite".to_string(),
            })
        );

        app.input_buffer = "/wb del arch".to_string();
        app.cursor_position = app.input_buffer.len();
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.pending_actions.pop_front(),
            Some(AppAction::DeleteBlackboard {
                key: "arch".to_string(),
            })
        );

        app.input_buffer = "explain tree".to_string();
        app.cursor_position = app.input_buffer.len();
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.active_transcript_md.contains("# User: explain tree"));
        assert_eq!(
            app.pending_actions.pop_front(),
            Some(AppAction::SendPrompt {
                thread_id: "t-main".to_string(),
                prompt: "explain tree".to_string(),
            })
        );

        app.input_buffer = "/quit".to_string();
        app.cursor_position = app.input_buffer.len();
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.should_quit());
    }

    #[test]
    fn test_permission_modal_intercepts_keys_and_queues_multiple() {
        let mut app = App::default();
        app.push_permission(PendingPermission {
            request_id: "req-1".to_string(),
            description: "Run git status".to_string(),
        });
        app.push_permission(PendingPermission {
            request_id: "req-2".to_string(),
            description: "Write file".to_string(),
        });

        app.handle_key_event(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(app.composer_input(), "");
        assert!(app.pending_actions.is_empty());

        app.handle_key_event(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        assert_eq!(
            app.pending_actions.pop_front(),
            Some(AppAction::ResolvePermission {
                request_id: "req-1".to_string(),
                approved: true,
            })
        );
        assert_eq!(
            app.pending_permission.as_ref().map(|p| p.request_id.as_str()),
            Some("req-2")
        );

        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(
            app.pending_actions.pop_front(),
            Some(AppAction::ResolvePermission {
                request_id: "req-2".to_string(),
                approved: false,
            })
        );
        assert!(app.pending_permission.is_none());
    }

    #[test]
    fn test_ctrl_n_forks_and_tree_enter_switches_thread_to_composer() {
        let mut app = App {
            active_thread_id: "t-1".to_string(),
            active_transcript_md: "# User: root\n\n".to_string(),
            threads: vec![
                ThreadItem {
                    id: "t-1".to_string(),
                    title: "Root".to_string(),
                    anchors: Vec::new(),
                },
                ThreadItem {
                    id: "t-2".to_string(),
                    title: "Branch".to_string(),
                    anchors: Vec::new(),
                },
            ],
            ..App::default()
        };
        app.thread_transcripts
            .insert("t-1".to_string(), "# User: root\n\n".to_string());
        app.thread_transcripts
            .insert("t-2".to_string(), "# User: branch\n\n".to_string());
        app.sync_anchors_and_blackboard();

        app.handle_key_event(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
        assert_eq!(
            app.pending_actions.pop_front(),
            Some(AppAction::ForkBranch {
                source_thread_id: "t-1".to_string(),
                title: None,
            })
        );

        app.set_focus(FocusedPane::Tree);
        let t2_idx = app
            .tree
            .nodes
            .iter()
            .position(|n| matches!(n, crate::ui::tree::TreeNode::Thread(t) if t.id == "t-2"))
            .unwrap();
        app.tree.select_index(t2_idx);
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(app.active_thread_id(), "t-2");
        assert!(app.active_transcript_md.contains("branch"));
        assert_eq!(app.focused_pane(), FocusedPane::Composer);
    }

    #[test]
    fn test_blackboard_navigation_and_editing_hotkeys() {
        let mut app = App::default();
        app.upsert_blackboard_entry("k1", "v1");
        app.upsert_blackboard_entry("k2", "v2");
        app.set_focus(FocusedPane::Blackboard);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(app.blackboard_selected, 1);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Composer);
        assert_eq!(app.composer_input(), "/wb set k2 v2");

        app.set_focus(FocusedPane::Blackboard);
        app.handle_key_event(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(app.blackboard_selected, 0);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert_eq!(
            app.pending_actions.pop_front(),
            Some(AppAction::DeleteBlackboard {
                key: "k1".to_string(),
            })
        );

        app.handle_key_event(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Composer);
        assert_eq!(app.composer_input(), "/wb set ");
    }
}
