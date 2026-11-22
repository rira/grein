use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::io::IsTerminal;
use thiserror::Error;

use crate::engine_port::{
    AgentConfigChoice, AgentConfigOption, AgentSlashCommand, PtyScreenSnapshot, SubagentSnapshot,
    ThreadMeta,
};
use crate::runner::{compute_ordered_thread_entries, OrderedThreadEntry};
use crate::transcript::{
    extract_last_fenced_code_block, ThreadTranscript, TranscriptBlock, TranscriptViewport,
};

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
    Terminal,
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
pub struct ConfigPickerModal {
    pub thread_id: String,
    pub config_id: String,
    pub title: String,
    pub current_value: String,
    pub options: Vec<AgentConfigChoice>,
    pub selected_index: usize,
    pub set_default: bool,
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
    CancelTurn {
        thread_id: String,
    },
    ResolvePermission {
        request_id: String,
        approved: bool,
    },
    OpenConfigPicker {
        thread_id: String,
        config_id: String,
    },
    SetAgentConfig {
        thread_id: String,
        config_id: String,
        value: String,
        set_default: bool,
    },
    ApplyAgentConfigQuery {
        thread_id: String,
        config_id: String,
        query: String,
        set_default: bool,
    },
    FetchAgentCommands {
        thread_id: String,
    },
    CreateTerminal {
        parent_thread_id: Option<String>,
        title: Option<String>,
        cwd: Option<String>,
        rows: u16,
        cols: u16,
    },
    PtyWrite {
        thread_id: String,
        bytes: Vec<u8>,
    },
    PtyResize {
        thread_id: String,
        rows: u16,
        cols: u16,
    },
    PtySnapshot {
        thread_id: String,
        rows: u16,
        cols: u16,
    },
    PtyScroll {
        thread_id: String,
        delta: isize,
    },
    PtyRestart {
        thread_id: String,
        rows: u16,
        cols: u16,
    },
    EnsureThreadWorktree {
        thread_id: String,
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
    pub const Stream: Self = Self::Transcript;
    pub const Chat: Self = Self::Transcript;
    pub const Sessions: Self = Self::Tree;
    pub const Workspace: Self = Self::Tree;
    pub const Output: Self = Self::Transcript;

    pub fn next(&self, direction: FocusDirection) -> Self {
        match direction {
            FocusDirection::Forward => match self {
                Self::Tree => Self::Transcript,
                Self::Transcript => Self::Composer,
                Self::Composer => Self::Tree,
                Self::Terminal => Self::Tree,
            },
            FocusDirection::Backward => match self {
                Self::Tree => Self::Composer,
                Self::Composer => Self::Transcript,
                Self::Transcript => Self::Tree,
                Self::Terminal => Self::Tree,
            },
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            Self::Tree => Self::Composer,
            Self::Composer => Self::Transcript,
            Self::Transcript => Self::Tree,
            Self::Terminal => Self::Tree,
        }
    }
}

impl From<crate::focus::PaneFocus> for FocusedPane {
    fn from(p: crate::focus::PaneFocus) -> Self {
        match p {
            crate::focus::PaneFocus::Composer => Self::Composer,
            crate::focus::PaneFocus::Sidebar | crate::focus::PaneFocus::Sessions => Self::Tree,
            crate::focus::PaneFocus::Transcript | crate::focus::PaneFocus::Chat => Self::Transcript,
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
    pub input_buffer: String,
    pub cursor_position: usize,
    pub kill_ring: String,
    pub should_quit: bool,
    pub active_transcript_md: String,
    pub threads: Vec<ThreadItem>,
    pub tree: crate::ui::tree::TreeState,
    pub last_submitted_prompt: Option<String>,
    pub active_thread_id: String,
    pub thread_transcripts: BTreeMap<String, String>,
    pub thread_blocks: BTreeMap<String, ThreadTranscript>,
    pub thread_blocks_synced_md: BTreeMap<String, String>,
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
    pub thread_config_options: HashMap<String, Vec<AgentConfigOption>>,
    pub thread_commands: HashMap<String, Vec<AgentSlashCommand>>,
    pub thread_active_tool: HashMap<String, Option<String>>,
    pub thread_turn_started_ms: HashMap<String, u64>,
    pub tick_count: u64,
    pub show_help_modal: bool,
    pub help_scroll_offset: u16,
    pub config_picker: Option<ConfigPickerModal>,
    pub sidebar_hidden: bool,
    pub composer_scroll_y: u16,
    pub transcript_viewport_height: u16,
    pub status_notice: Option<String>,
    pub pending_rename_thread_id: Option<String>,
    pub last_copied_text: Option<String>,
    pub pty_screens: HashMap<String, PtyScreenSnapshot>,
    pub pty_exit_codes: HashMap<String, Option<i32>>,
    pub last_pty_sizes: HashMap<String, (u16, u16)>,
    pub terminal_pane_size: (u16, u16),
    pub last_frame_area: Option<ratatui::layout::Rect>,
    pub transcript_search: crate::search::TranscriptSearchState,
    pub pending_g_prefix: bool,
    pub collapsed_threads: BTreeSet<String>,
    pub tree_has_children: BTreeSet<String>,
    pub tree_hidden_counts: BTreeMap<String, usize>,
}

pub type AppState = App;

impl Default for App {
    fn default() -> Self {
        Self {
            focused_pane: FocusedPane::Composer,
            focus: FocusedPane::Composer,
            active_focus: FocusedPane::Composer,
            input_buffer: String::new(),
            cursor_position: 0,
            kill_ring: String::new(),
            should_quit: false,
            active_transcript_md: String::new(),
            threads: Vec::new(),
            tree: crate::ui::tree::TreeState::default(),
            last_submitted_prompt: None,
            active_thread_id: "trunk".to_string(),
            thread_transcripts: BTreeMap::new(),
            thread_blocks: BTreeMap::new(),
            thread_blocks_synced_md: BTreeMap::new(),
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
            thread_config_options: HashMap::new(),
            thread_commands: HashMap::new(),
            thread_active_tool: HashMap::new(),
            thread_turn_started_ms: HashMap::new(),
            tick_count: 0,
            show_help_modal: false,
            help_scroll_offset: 0,
            config_picker: None,
            sidebar_hidden: false,
            composer_scroll_y: 0,
            transcript_viewport_height: 0,
            status_notice: None,
            pending_rename_thread_id: None,
            last_copied_text: None,
            pty_screens: HashMap::new(),
            pty_exit_codes: HashMap::new(),
            last_pty_sizes: HashMap::new(),
            terminal_pane_size: (0, 0),
            last_frame_area: None,
            transcript_search: crate::search::TranscriptSearchState::default(),
            pending_g_prefix: false,
            collapsed_threads: BTreeSet::new(),
            tree_has_children: BTreeSet::new(),
            tree_hidden_counts: BTreeMap::new(),
        }
    }
}

pub fn shell_label_from_env(val: Option<&str>) -> String {
    val.map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| std::path::Path::new(s).file_name())
        .and_then(|n| n.to_str())
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("sh")
        .to_string()
}

pub fn active_shell_label() -> String {
    let val = std::env::var("SHELL").ok();
    shell_label_from_env(val.as_deref())
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

#[derive(Debug, Clone, Copy)]
enum TranscriptScroll {
    Up(usize),
    Down(usize),
    Top,
    Bottom,
}

fn step_wrapped(pos: usize, len: usize, forward: bool) -> usize {
    if forward {
        (pos + 1) % len
    } else {
        (pos + len - 1) % len
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CopyMode {
    LastReply,
    CodeBlock,
    All,
}

pub(crate) fn is_config_option_kind(opt: &AgentConfigOption, kind: &str) -> bool {
    let id = opt.id.to_ascii_lowercase();
    let cat = opt.category.as_deref().unwrap_or("").to_ascii_lowercase();
    match kind {
        "model" => id == "model" || cat == "model",
        "mode" => id == "mode" || cat == "mode",
        "thought_level" | "thinking" | "effort" => matches!(
            id.as_str(),
            "thought_level" | "thinking" | "effort" | "reasoning" | "reasoning_effort"
        ) || matches!(
            cat.as_str(),
            "thought_level" | "thinking" | "effort" | "reasoning" | "reasoning_effort"
        ),
        _ => id.eq_ignore_ascii_case(kind) || cat.eq_ignore_ascii_case(kind),
    }
}

pub(crate) fn find_config_option<'a>(
    options: &'a [AgentConfigOption],
    requested_id: &str,
) -> Option<&'a AgentConfigOption> {
    if let Some(exact) = options
        .iter()
        .find(|opt| is_config_option_kind(opt, requested_id))
    {
        return Some(exact);
    }
    let needle = requested_id.to_ascii_lowercase();
    options.iter().find(|opt| {
        let id = opt.id.to_ascii_lowercase();
        let cat = opt.category.as_deref().unwrap_or("").to_ascii_lowercase();
        if needle == "thought_level" || needle == "thinking" || needle == "effort" {
            id.contains("thought")
                || id.contains("think")
                || id.contains("effort")
                || cat.contains("thought")
                || cat.contains("think")
                || cat.contains("effort")
        } else {
            id.contains(&needle) || cat.contains(&needle)
        }
    })
}

fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut chars = needle.chars();
    let Some(mut cur) = chars.next() else {
        return true;
    };
    for h in haystack.chars() {
        if h == cur {
            if let Some(next) = chars.next() {
                cur = next;
            } else {
                return true;
            }
        }
    }
    false
}

pub(crate) fn match_config_choice<'a>(
    choices: &'a [AgentConfigChoice],
    query: &str,
) -> Option<&'a AgentConfigChoice> {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() || choices.is_empty() {
        return None;
    }

    let mut best: Option<(u8, usize, &'a AgentConfigChoice)> = None;
    for choice in choices {
        let val = choice.value.to_ascii_lowercase();
        let name = choice.name.to_ascii_lowercase();
        let tier = if val == q || name == q {
            Some(0u8)
        } else if val.starts_with(&q) || name.starts_with(&q) {
            Some(1u8)
        } else if val.contains(&q) || name.contains(&q) {
            Some(2u8)
        } else if is_subsequence(&q, &val) || is_subsequence(&q, &name) {
            Some(3u8)
        } else {
            None
        };
        if let Some(t) = tier {
            let len = choice.value.len();
            let replace = match best {
                None => true,
                Some((bt, bl, _)) => t < bt || (t == bt && len < bl),
            };
            if replace {
                best = Some((t, len, choice));
            }
        }
    }
    best.map(|(_, _, c)| c)
}

fn extract_last_assistant_md(md: &str) -> Option<String> {
    let mut last_start: Option<usize> = None;
    for marker in ["## Agent (agy)\n", "## Assistant\n"] {
        if let Some(idx) = md.rfind(marker) {
            let start = idx + marker.len();
            if last_start.is_none_or(|prev| start > prev) {
                last_start = Some(start);
            }
        }
    }
    if let Some(start) = last_start {
        let tail = &md[start..];
        let end = tail
            .find("\n# User: ")
            .or_else(|| tail.find("\n$ "))
            .unwrap_or(tail.len());
        let body = tail[..end].trim();
        if !body.is_empty() {
            return Some(body.to_string());
        }
    }
    let trimmed = md.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
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

    pub fn is_terminal_thread(&self, thread_id: &str) -> bool {
        self.thread_meta
            .get(thread_id)
            .is_some_and(|m| m.kind == grein_core::ThreadKind::Terminal)
    }

    pub fn active_is_terminal(&self) -> bool {
        self.is_terminal_thread(&self.active_thread_id)
    }

    pub fn terminal_exit_code(&self, thread_id: &str) -> Option<Option<i32>> {
        if let Some(&code) = self.pty_exit_codes.get(thread_id) {
            Some(code)
        } else {
            self.pty_screens
                .get(thread_id)
                .filter(|s| s.exited)
                .map(|s| s.exit_code)
        }
    }

    pub fn is_terminal_exited(&self, thread_id: &str) -> bool {
        self.terminal_exit_code(thread_id).is_some()
    }

    fn find_existing_terminal_for(&self, target_id: &str) -> Option<String> {
        if self.is_terminal_thread(target_id) {
            return Some(target_id.to_string());
        }
        let mut candidates: Vec<&str> = self
            .threads
            .iter()
            .filter_map(|t| {
                let meta = self.thread_meta.get(&t.id)?;
                (meta.kind == grein_core::ThreadKind::Terminal
                    && meta.parent_thread_id.as_deref() == Some(target_id))
                .then_some(t.id.as_str())
            })
            .collect();
        if candidates.is_empty() {
            let mut meta_ids: Vec<&str> = self
                .thread_meta
                .values()
                .filter(|m| {
                    m.kind == grein_core::ThreadKind::Terminal
                        && m.parent_thread_id.as_deref() == Some(target_id)
                })
                .map(|m| m.thread_id.as_str())
                .collect();
            meta_ids.sort_unstable();
            candidates = meta_ids;
        }
        candidates
            .iter()
            .rev()
            .copied()
            .find(|id| !self.is_terminal_exited(id))
            .or_else(|| candidates.last().copied())
            .map(str::to_string)
    }

    fn focus_tree(&mut self) {
        self.pending_rename_thread_id = None;
        self.pending_g_prefix = false;
        self.focused_pane = FocusedPane::Tree;
        self.sidebar_hidden = false;
        self.sync_focus_out();
    }

    fn open_or_jump_to_terminal(&mut self) {
        self.pending_rename_thread_id = None;
        let target_id = if self.focused_pane == FocusedPane::Tree {
            self.selected_tree_thread_info().0
        } else {
            self.active_thread_id.clone()
        };
        if let Some(term_id) = self.find_existing_terminal_for(&target_id) {
            if term_id != self.active_thread_id {
                self.switch_thread(&term_id);
            } else {
                self.maybe_request_pty_snapshot(&term_id);
                self.maybe_queue_terminal_resize();
            }
            self.focused_pane = FocusedPane::Terminal;
            self.sync_focus_out();
        } else {
            self.dispatch_create_terminal(Some(target_id), None);
            self.sync_focus_out();
        }
    }

    pub fn current_terminal_dims_or_default(&self) -> (u16, u16) {
        if self.terminal_pane_size.0 >= 1 && self.terminal_pane_size.1 >= 1 {
            return self.terminal_pane_size;
        }
        if let Some(area) = self.last_frame_area {
            let sub_count = self
                .thread_subagents
                .get(&self.active_thread_id)
                .map_or(0, Vec::len);
            let strip_h = crate::layout::subagent_strip_height(sub_count);
            let (rows, cols) =
                crate::layout::terminal_inner_size(area, strip_h, self.sidebar_hidden);
            if rows >= 1 && cols >= 1 {
                return (rows, cols);
            }
        }
        (24, 80)
    }

    pub(crate) fn maybe_request_pty_snapshot(&mut self, thread_id: &str) {
        if !self.is_terminal_thread(thread_id) || self.pty_screens.contains_key(thread_id) {
            return;
        }
        let already_queued = self.pending_actions.iter().any(|a| {
            matches!(a, AppAction::PtySnapshot { thread_id: tid, .. } if tid == thread_id)
        });
        if already_queued {
            return;
        }
        let (rows, cols) = self.current_terminal_dims_or_default();
        self.pending_actions.push_back(AppAction::PtySnapshot {
            thread_id: thread_id.to_string(),
            rows,
            cols,
        });
    }

    pub(crate) fn maybe_queue_terminal_resize(&mut self) {
        if !self.active_is_terminal() {
            return;
        }
        let Some(area) = self.last_frame_area else {
            return;
        };
        let sub_count = self
            .thread_subagents
            .get(&self.active_thread_id)
            .map_or(0, Vec::len);
        let strip_h = crate::layout::subagent_strip_height(sub_count);
        let (rows, cols) =
            crate::layout::terminal_inner_size(area, strip_h, self.sidebar_hidden);
        if rows >= 1 && cols >= 1 {
            self.terminal_pane_size = (rows, cols);
            if self.last_pty_sizes.get(&self.active_thread_id).copied() != Some((rows, cols)) {
                self.last_pty_sizes
                    .insert(self.active_thread_id.clone(), (rows, cols));
                self.pending_actions.push_back(AppAction::PtyResize {
                    thread_id: self.active_thread_id.clone(),
                    rows,
                    cols,
                });
            }
        }
    }

    pub(crate) fn sync_focus_for_active_thread(&mut self) {
        if self.active_is_terminal() {
            if matches!(
                self.focused_pane,
                FocusedPane::Composer | FocusedPane::Transcript
            ) {
                self.focused_pane = FocusedPane::Terminal;
                self.sync_focus_out();
            }
        } else if self.focused_pane == FocusedPane::Terminal {
            self.focused_pane = FocusedPane::Composer;
            self.sync_focus_out();
        }
    }

    pub(crate) fn dispatch_create_terminal(
        &mut self,
        parent_thread_id: Option<String>,
        cwd: Option<String>,
    ) {
        self.pending_rename_thread_id = None;
        let (rows, cols) = self.current_terminal_dims_or_default();
        self.status_notice = None;
        self.pending_actions.push_back(AppAction::CreateTerminal {
            parent_thread_id,
            title: None,
            cwd,
            rows,
            cols,
        });
    }

    fn has_active_modal(&self) -> bool {
        self.pending_permission.is_some() || self.config_picker.is_some() || self.show_help_modal
    }

    pub fn handle_paste_event(&mut self, text: &str) -> bool {
        self.sync_focus_in();
        if self.has_active_modal() {
            return false;
        }
        if self.focused_pane == FocusedPane::Terminal {
            if self.is_terminal_exited(&self.active_thread_id) {
                return false;
            }
            let bracketed = self
                .pty_screens
                .get(&self.active_thread_id)
                .is_some_and(|s| s.bracketed_paste);
            let bytes = crate::vt_input::encode_paste(text, bracketed);
            if !bytes.is_empty() {
                self.pending_actions.push_back(AppAction::PtyWrite {
                    thread_id: self.active_thread_id.clone(),
                    bytes,
                });
            }
            self.sync_focus_out();
            return true;
        }
        if self.focused_pane == FocusedPane::Composer {
            let pos = clamp_char_boundary(&self.input_buffer, self.cursor_position);
            self.input_buffer.insert_str(pos, text);
            self.cursor_position = pos + text.len();
            self.sync_focus_out();
            return true;
        }
        false
    }

    pub fn unfold_ancestors_for(&mut self, thread_id: &str) -> bool {
        let mut changed = false;
        let mut visited = std::collections::HashSet::new();
        let mut cur = self
            .thread_meta
            .get(thread_id)
            .and_then(|m| m.parent_thread_id.as_deref())
            .map(str::to_string);
        while let Some(pid) = cur {
            if !visited.insert(pid.clone()) {
                break;
            }
            if self.collapsed_threads.remove(&pid) {
                changed = true;
            }
            cur = self
                .thread_meta
                .get(&pid)
                .and_then(|m| m.parent_thread_id.as_deref())
                .map(str::to_string);
        }
        changed
    }

    pub fn thread_has_children(&self, thread_id: &str) -> bool {
        self.tree_has_children.contains(thread_id)
            || self.thread_meta.values().any(|m| {
                m.parent_thread_id.as_deref() == Some(thread_id)
                    && self.threads.iter().any(|t| t.id == m.thread_id)
            })
    }

    pub fn toggle_tree_fold(&mut self, thread_id: &str) -> bool {
        if !self.thread_has_children(thread_id) {
            return false;
        }
        if !self.collapsed_threads.insert(thread_id.to_string()) {
            self.collapsed_threads.remove(thread_id);
        }
        crate::runner::rebuild_tree_nodes(self);
        true
    }

    pub fn fold_or_jump_parent_in_tree(&mut self) -> bool {
        let Some(idx) = self.tree.selected_index() else {
            return false;
        };
        match self.tree.nodes.get(idx).cloned() {
            Some(crate::ui::tree::TreeNode::Anchor(anchor)) => {
                if let Some(parent_idx) = self.tree.nodes.iter().position(|n| {
                    matches!(n, crate::ui::tree::TreeNode::Thread(t) if t.id == anchor.thread_id)
                }) {
                    self.tree.select_index(parent_idx);
                    self.maybe_request_pty_snapshot(&anchor.thread_id);
                    return true;
                }
                false
            }
            Some(crate::ui::tree::TreeNode::Thread(thread)) => {
                if self.thread_has_children(&thread.id)
                    && !self.collapsed_threads.contains(&thread.id)
                {
                    self.collapsed_threads.insert(thread.id);
                    crate::runner::rebuild_tree_nodes(self);
                    return true;
                }
                if let Some(parent_id) = self
                    .thread_meta
                    .get(&thread.id)
                    .and_then(|m| m.parent_thread_id.as_deref())
                    .map(str::to_string)
                {
                    if let Some(parent_idx) = self.tree.nodes.iter().position(|n| {
                        matches!(n, crate::ui::tree::TreeNode::Thread(t) if t.id == parent_id)
                    }) {
                        self.tree.select_index(parent_idx);
                        self.maybe_request_pty_snapshot(&parent_id);
                        return true;
                    }
                }
                false
            }
            None => false,
        }
    }

    pub fn switch_thread(&mut self, thread_id: impl Into<String>) {
        let target_thread = thread_id.into();
        self.pending_rename_thread_id = None;
        self.pending_g_prefix = false;
        if target_thread != self.active_thread_id {
            self.transcript_search.clear();
        }
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
        let active_id = self.active_thread_id.clone();
        self.unfold_ancestors_for(&active_id);
        self.sync_anchors();
        self.select_active_thread_in_tree();
        self.sync_focus_for_active_thread();
        self.maybe_request_pty_snapshot(&active_id);
        self.maybe_queue_terminal_resize();
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

    pub(crate) fn request_join_thread(
        &mut self,
        source_thread_id: String,
        summary: Option<String>,
    ) {
        let target_thread_id = self.resolve_merge_target(&source_thread_id);
        if target_thread_id.is_empty() {
            self.status_notice = Some("Cannot join a root context (no parent branch)".to_string());
            return;
        }
        self.status_notice = None;
        self.pending_actions.push_back(AppAction::MergeBranch {
            source_thread_id,
            target_thread_id,
            summary,
        });
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
        let prev = self.focused_pane;
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
        if prev != FocusedPane::Tree && self.focused_pane == FocusedPane::Tree {
            self.sidebar_hidden = false;
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
        self.pending_g_prefix = false;
        let pane = pane.into();
        if pane == FocusedPane::Tree {
            self.focus_tree();
        } else {
            self.focused_pane = pane;
            self.sync_focus_out();
        }
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

    fn activate_selected_tree_node(&mut self) -> Option<TreeAction> {
        self.pending_rename_thread_id = None;
        let Some(idx) = self.tree.selected_index() else {
            self.focused_pane = if self.active_is_terminal() {
                FocusedPane::Terminal
            } else {
                FocusedPane::Composer
            };
            self.sync_focus_out();
            return None;
        };
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
                Some(TreeAction::ActivateAnchor(id))
            }
            Some(crate::ui::tree::TreeNode::Thread(thread)) => {
                if thread.id != self.active_thread_id {
                    self.switch_thread(&thread.id);
                }
                self.focused_pane = if self.is_terminal_thread(&thread.id) {
                    FocusedPane::Terminal
                } else {
                    FocusedPane::Composer
                };
                self.sync_focus_out();
                None
            }
            None => {
                self.focused_pane = if self.active_is_terminal() {
                    FocusedPane::Terminal
                } else {
                    FocusedPane::Composer
                };
                self.sync_focus_out();
                None
            }
        }
    }

    fn toggle_composer_tree_focus(&mut self) {
        if matches!(
            self.focused_pane,
            FocusedPane::Composer | FocusedPane::Terminal
        ) {
            self.focus_tree();
        } else if self.focused_pane == FocusedPane::Tree {
            let _ = self.activate_selected_tree_node();
        } else if self.active_is_terminal() {
            self.focused_pane = FocusedPane::Terminal;
            self.sync_focus_out();
        } else {
            self.focused_pane = FocusedPane::Composer;
            self.sync_focus_out();
        }
    }

    pub fn cycle_focus(&mut self) {
        self.focus_next();
    }

    pub fn cycle_focus_reverse(&mut self) {
        self.focus_prev();
    }

    pub fn focus_next(&mut self) {
        self.sync_focus_in();
        self.toggle_composer_tree_focus();
    }

    pub fn next_pane(&mut self) {
        self.focus_next();
    }

    pub fn focus_prev(&mut self) {
        self.sync_focus_in();
        self.toggle_composer_tree_focus();
    }

    pub fn prev_pane(&mut self) {
        self.focus_prev();
    }

    pub fn focus_previous(&mut self) {
        self.focus_prev();
    }

    fn active_root_subtree_bounds(
        ordered: &[OrderedThreadEntry],
        threads: &[ThreadItem],
        active_thread_id: &str,
    ) -> Option<(usize, usize, usize)> {
        let active_pos = ordered
            .iter()
            .position(|e| threads[e.index].id == active_thread_id)?;
        let root_start = (0..=active_pos).rev().find(|&i| ordered[i].depth == 0)?;
        let root_end = (root_start + 1..ordered.len())
            .find(|&i| ordered[i].depth == 0)
            .unwrap_or(ordered.len());
        Some((root_start, root_end, active_pos))
    }

    pub fn cycle_branch_in_active_root(&mut self, forward: bool) {
        let ordered = compute_ordered_thread_entries(&self.threads, &self.thread_meta);
        let Some((root_start, root_end, active_pos)) =
            Self::active_root_subtree_bounds(&ordered, &self.threads, &self.active_thread_id)
        else {
            return;
        };
        let subtree = &ordered[root_start..root_end];
        if subtree.len() <= 1 {
            return;
        }
        let next_pos = step_wrapped(active_pos - root_start, subtree.len(), forward);
        let target_id = self.threads[subtree[next_pos].index].id.clone();
        self.switch_thread(target_id);
    }

    pub fn switch_root(&mut self, forward: bool) {
        let ordered = compute_ordered_thread_entries(&self.threads, &self.thread_meta);
        let roots: Vec<(usize, String)> = ordered
            .iter()
            .enumerate()
            .filter(|(_, e)| e.depth == 0)
            .map(|(ord_pos, e)| (ord_pos, self.threads[e.index].id.clone()))
            .collect();
        if roots.len() <= 1 {
            return;
        }
        let Some((root_start, _, _)) =
            Self::active_root_subtree_bounds(&ordered, &self.threads, &self.active_thread_id)
        else {
            return;
        };
        let Some(root_idx) = roots
            .iter()
            .position(|(ord_pos, _)| *ord_pos == root_start)
        else {
            return;
        };
        let next_root_idx = step_wrapped(root_idx, roots.len(), forward);
        let target_root_id = roots[next_root_idx].1.clone();
        self.switch_thread(target_root_id);
    }

    fn transcript_scroll_target_id(&self) -> String {
        if self.focused_pane == FocusedPane::Tree {
            self.selected_tree_thread_info().0
        } else {
            self.active_thread_id.clone()
        }
    }

    fn half_page_scroll_lines(&self) -> usize {
        if self.transcript_viewport_height > 0 {
            (self.transcript_viewport_height as usize / 2).max(1)
        } else {
            10
        }
    }

    pub(crate) fn has_synchronized_blocks(&self, thread_id: &str, display_md: &str) -> bool {
        match self.thread_blocks.get(thread_id) {
            Some(blocks) if !blocks.is_empty() => {
                display_md.is_empty()
                    || self
                        .thread_blocks_synced_md
                        .get(thread_id)
                        .is_some_and(|synced| synced == display_md)
            }
            _ => false,
        }
    }

    pub(crate) fn sync_agent_meta_from_config_options(
        &mut self,
        thread_id: &str,
        options: &[AgentConfigOption],
    ) {
        let entry = self
            .thread_agent_meta
            .entry(thread_id.to_string())
            .or_default();
        if let Some(model_opt) = find_config_option(options, "model") {
            if !model_opt.current_value.trim().is_empty() {
                entry.0 = Some(model_opt.current_value.clone());
            }
        }
        if let Some(mode_opt) = find_config_option(options, "mode") {
            if !mode_opt.current_value.trim().is_empty() {
                entry.1 = Some(mode_opt.current_value.clone());
            }
        }
    }

    pub fn open_config_picker_from_options(&mut self, thread_id: &str, config_id: &str) -> bool {
        let Some(opts) = self.thread_config_options.get(thread_id) else {
            self.status_notice = Some(format!("No options exposed for {config_id}"));
            return false;
        };
        let Some(opt) = find_config_option(opts, config_id) else {
            self.status_notice = Some(format!("Option '{config_id}' not exposed by agent"));
            return false;
        };
        if opt.options.is_empty() {
            self.status_notice = Some(format!("No choices available for {}", opt.name));
            return false;
        }
        let selected_index = opt
            .options
            .iter()
            .position(|c| c.value == opt.current_value)
            .unwrap_or(0);
        let title = if opt.name.trim().is_empty() {
            opt.id.clone()
        } else {
            opt.name.clone()
        };
        self.config_picker = Some(ConfigPickerModal {
            thread_id: thread_id.to_string(),
            config_id: opt.id.clone(),
            title,
            current_value: opt.current_value.clone(),
            options: opt.options.clone(),
            selected_index,
            set_default: false,
        });
        true
    }

    pub fn toggle_transcript_cards(&mut self) {
        let target_id = self.transcript_scroll_target_id();
        let Some(blocks) = self.thread_blocks.get_mut(&target_id) else {
            self.status_notice = Some("No collapsible tool/shell cards".to_string());
            return;
        };
        let expanded = blocks.toggle_all_collapsible();
        if expanded {
            self.status_notice = Some("Expanded tool/shell cards".to_string());
        } else if blocks.blocks.iter().any(|b| {
            matches!(
                b,
                TranscriptBlock::ToolCall {
                    collapsed: true,
                    ..
                } | TranscriptBlock::ShellExecution {
                    collapsed: true,
                    ..
                }
            )
        }) {
            self.status_notice = Some("Collapsed tool/shell cards".to_string());
        } else {
            self.status_notice = Some("No collapsible tool/shell cards".to_string());
        }
    }

    pub fn toggle_sidebar_zoom(&mut self) {
        self.sidebar_hidden = !self.sidebar_hidden;
        self.status_notice = Some(if self.sidebar_hidden {
            "Sidebar hidden (copy zoom)".to_string()
        } else {
            "Sidebar shown".to_string()
        });
        self.maybe_queue_terminal_resize();
    }

    pub(crate) fn copy_transcript_content(&mut self, mode: CopyMode) {
        let target_id = self.transcript_scroll_target_id();
        if self.is_terminal_thread(&target_id) {
            let text_opt = self
                .pty_screens
                .get(&target_id)
                .map(PtyScreenSnapshot::plain_text)
                .filter(|s| !s.trim().is_empty());
            let Some(text) = text_opt else {
                self.status_notice =
                    Some("Nothing to copy (terminal screen is empty)".to_string());
                return;
            };
            self.finish_copy_to_clipboard("terminal screen", text);
            return;
        }
        let display_md = if target_id == self.active_thread_id {
            self.active_transcript_md.as_str()
        } else {
            self.thread_transcripts
                .get(&target_id)
                .map(String::as_str)
                .unwrap_or("")
        };
        let use_blocks = self.has_synchronized_blocks(&target_id, display_md);

        let extracted = match mode {
            CopyMode::LastReply => {
                if use_blocks {
                    self.thread_blocks
                        .get(&target_id)
                        .and_then(ThreadTranscript::latest_assistant_text)
                } else {
                    extract_last_assistant_md(display_md)
                }
            }
            CopyMode::CodeBlock => {
                if use_blocks {
                    self.thread_blocks
                        .get(&target_id)
                        .and_then(ThreadTranscript::latest_code_block)
                } else {
                    let mut last_code = None;
                    extract_last_fenced_code_block(display_md, &mut last_code);
                    last_code
                }
            }
            CopyMode::All => {
                if use_blocks {
                    self.thread_blocks
                        .get(&target_id)
                        .map(ThreadTranscript::full_plain_text)
                        .filter(|s| !s.trim().is_empty())
                } else {
                    let trimmed = display_md.trim();
                    (!trimmed.is_empty()).then(|| trimmed.to_string())
                }
            }
        };

        let Some(text) = extracted else {
            self.status_notice = Some(match mode {
                CopyMode::LastReply => "Nothing to copy (no assistant reply yet)".to_string(),
                CopyMode::CodeBlock => "Nothing to copy (no code block found)".to_string(),
                CopyMode::All => "Nothing to copy (transcript is empty)".to_string(),
            });
            return;
        };

        let kind = match mode {
            CopyMode::LastReply => "last reply",
            CopyMode::CodeBlock => "code block",
            CopyMode::All => "transcript",
        };
        self.finish_copy_to_clipboard(kind, text);
    }

    fn finish_copy_to_clipboard(&mut self, kind: &str, text: String) {
        let char_count = text.chars().count();
        self.last_copied_text = Some(text.clone());
        match crate::clipboard::copy_to_clipboard(&text) {
            Ok(backend) => {
                self.status_notice =
                    Some(format!("Copied {kind} ({char_count} chars) via {backend}"));
            }
            Err(err) => {
                self.status_notice = Some(format!(
                    "Copy failed for {kind} ({char_count} chars): {err}"
                ));
            }
        }
    }

    fn handle_key_config_picker(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let no_ctrl_alt = !ctrl && !alt;

        match key.code {
            KeyCode::Up | KeyCode::Char('k' | 'K') if no_ctrl_alt => {
                if let Some(ref mut picker) = self.config_picker {
                    picker.selected_index = picker.selected_index.saturating_sub(1);
                }
            }
            KeyCode::Down | KeyCode::Char('j' | 'J') if no_ctrl_alt => {
                if let Some(ref mut picker) = self.config_picker {
                    if !picker.options.is_empty() {
                        picker.selected_index =
                            (picker.selected_index + 1).min(picker.options.len() - 1);
                    }
                }
            }
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Char('d' | 'D') if no_ctrl_alt => {
                if let Some(ref mut picker) = self.config_picker {
                    picker.set_default = !picker.set_default;
                }
            }
            KeyCode::Enter if no_ctrl_alt => {
                if let Some(picker) = self.config_picker.take() {
                    if let Some(choice) = picker
                        .options
                        .get(picker.selected_index)
                        .or_else(|| picker.options.first())
                    {
                        self.pending_actions.push_back(AppAction::SetAgentConfig {
                            thread_id: picker.thread_id,
                            config_id: picker.config_id,
                            value: choice.value.clone(),
                            set_default: picker.set_default,
                        });
                    }
                }
            }
            KeyCode::Esc | KeyCode::Char('q' | 'Q') if no_ctrl_alt => {
                self.config_picker = None;
            }
            _ => {}
        }
    }

    pub fn current_transcript_inner_width(&self) -> usize {
        if let Some(area) = self.last_frame_area {
            let sub_count = self
                .thread_subagents
                .get(&self.active_thread_id)
                .map_or(0, Vec::len);
            let strip_h = crate::layout::subagent_strip_height(sub_count);
            let layout = crate::layout::compute_layout_with_options(
                area,
                strip_h,
                self.sidebar_hidden,
                false,
            );
            let w = if self.sidebar_hidden {
                layout.transcript_area.width
            } else {
                layout.transcript_area.width.saturating_sub(2)
            };
            if w >= 1 {
                return w as usize;
            }
        }
        80
    }

    fn search_anchor_for(&self, target_id: &str, total_lines: usize) -> Option<usize> {
        let vp = self.transcript_viewport(target_id);
        match self.transcript_search.direction {
            crate::search::SearchDirection::Forward => Some(vp.scroll_offset()),
            crate::search::SearchDirection::Reverse => {
                let vp_h = if self.transcript_viewport_height > 0 {
                    self.transcript_viewport_height as usize
                } else {
                    total_lines.max(1)
                };
                Some(
                    vp.scroll_offset()
                        .saturating_add(vp_h.saturating_sub(1))
                        .min(total_lines.saturating_sub(1)),
                )
            }
        }
    }

    fn scroll_transcript_to_line(&mut self, target_id: &str, line_idx: usize, total_lines: usize) {
        let vp_h = if self.transcript_viewport_height > 0 {
            self.transcript_viewport_height as usize
        } else {
            20
        };
        let max_scroll = total_lines.saturating_sub(vp_h);
        let has_vp_height = self.transcript_viewport_height > 0;
        let vp = self.transcript_viewport_mut(target_id);
        if has_vp_height || vp.max_scroll == 0 {
            vp.max_scroll = max_scroll;
        }
        let eff_max = vp.max_scroll;
        let cur = vp.scroll_offset.min(eff_max);
        let new_offset = if line_idx < cur {
            line_idx.saturating_sub(vp_h / 4).min(eff_max)
        } else if line_idx >= cur + vp_h {
            (line_idx + 1)
                .saturating_sub(vp_h)
                .saturating_add(vp_h / 4)
                .min(eff_max)
        } else {
            cur
        };
        vp.scroll_offset = new_offset;
        vp.auto_scroll = false;
    }

    fn recompute_transcript_search_and_scroll(&mut self) {
        let target_id = self.transcript_scroll_target_id();
        if self.is_terminal_thread(&target_id) {
            self.transcript_search.recompute_matches(&[], None);
            return;
        }
        let width = self.current_transcript_inner_width();
        let lines = crate::render::build_transcript_wrapped_lines(self, &target_id, width);
        let anchor = self.search_anchor_for(&target_id, lines.len());
        self.transcript_search.recompute_matches(&lines, anchor);
        if let Some(line_idx) = self.transcript_search.active_line_index() {
            self.scroll_transcript_to_line(&target_id, line_idx, lines.len());
        }
    }

    fn step_transcript_search(&mut self, next: bool) {
        let target_id = self.transcript_scroll_target_id();
        if self.is_terminal_thread(&target_id) {
            return;
        }
        let width = self.current_transcript_inner_width();
        let lines = crate::render::build_transcript_wrapped_lines(self, &target_id, width);
        if !self.transcript_search.query.is_empty() {
            let old_matches = std::mem::take(&mut self.transcript_search.matches);
            let old_idx = self.transcript_search.current_index;
            let prev_active = old_idx.and_then(|i| old_matches.get(i)).cloned();
            let anchor = self.search_anchor_for(&target_id, lines.len());
            self.transcript_search.recompute_matches(&lines, anchor);
            if self.transcript_search.matches == old_matches {
                self.transcript_search.current_index = old_idx;
            } else if let Some(prev_m) = prev_active {
                if let Some(pos) = self
                    .transcript_search
                    .matches
                    .iter()
                    .position(|m| *m == prev_m)
                {
                    self.transcript_search.current_index = Some(pos);
                } else if let (Some(oi), false) = (old_idx, self.transcript_search.matches.is_empty())
                {
                    self.transcript_search.current_index =
                        Some(oi.min(self.transcript_search.matches.len() - 1));
                }
            }
        }
        let moved = if next {
            self.transcript_search.next_match().is_some()
        } else {
            self.transcript_search.prev_match().is_some()
        };
        if moved {
            if let Some(line_idx) = self.transcript_search.active_line_index() {
                self.scroll_transcript_to_line(&target_id, line_idx, lines.len());
            }
        }
    }

    fn open_transcript_search(&mut self, direction: crate::search::SearchDirection) {
        self.transcript_search.open(direction);
        self.pending_g_prefix = false;
    }

    pub fn handle_mouse_event(&mut self, mouse: crossterm::event::MouseEvent) -> bool {
        use crossterm::event::MouseEventKind;
        let is_up = match mouse.kind {
            MouseEventKind::ScrollUp => true,
            MouseEventKind::ScrollDown => false,
            _ => return false,
        };
        self.sync_focus_in();
        if self.show_help_modal {
            if is_up {
                self.help_scroll_offset = self.help_scroll_offset.saturating_sub(3);
            } else {
                self.help_scroll_offset = self.help_scroll_offset.saturating_add(3);
            }
            self.sync_focus_out();
            return true;
        }
        if self.has_active_modal() {
            self.sync_focus_out();
            return false;
        }
        let delta: isize = if is_up { 3 } else { -3 };
        let over_tree_pane = self.focused_pane == FocusedPane::Tree
            && self.last_frame_area.is_none_or(|area| {
                if self.sidebar_hidden || area.width == 0 || area.height == 0 {
                    false
                } else {
                    let sub_count = self
                        .thread_subagents
                        .get(&self.active_thread_id)
                        .map_or(0, Vec::len);
                    let strip_h = crate::layout::subagent_strip_height(sub_count);
                    let layout = crate::layout::compute_layout_with_options(
                        area,
                        strip_h,
                        self.sidebar_hidden,
                        self.active_is_terminal() || self.pending_rename_thread_id.is_some(),
                    );
                    !layout
                        .transcript_area
                        .contains(ratatui::layout::Position::new(mouse.column, mouse.row))
                }
            });

        if self.active_is_terminal() && !over_tree_pane {
            self.pending_actions.push_back(AppAction::PtyScroll {
                thread_id: self.active_thread_id.clone(),
                delta,
            });
            self.sync_focus_out();
            return true;
        }

        if over_tree_pane {
            let target_id = self.selected_tree_thread_info().0;
            if self.is_terminal_thread(&target_id)
                && self.last_frame_area.is_some()
                && self.active_is_terminal()
            {
                self.pending_actions.push_back(AppAction::PtyScroll {
                    thread_id: target_id,
                    delta,
                });
            } else if is_up {
                self.tree.select_previous();
                let (sel_id, _) = self.selected_tree_thread_info();
                self.maybe_request_pty_snapshot(&sel_id);
            } else {
                self.tree.select_next();
                let (sel_id, _) = self.selected_tree_thread_info();
                self.maybe_request_pty_snapshot(&sel_id);
            }
            self.sync_focus_out();
            return true;
        }

        let target_id = self.transcript_scroll_target_id();
        if self.is_terminal_thread(&target_id) {
            self.pending_actions.push_back(AppAction::PtyScroll {
                thread_id: target_id,
                delta,
            });
        } else if is_up {
            self.scroll_target_transcript(TranscriptScroll::Up(3));
        } else {
            self.scroll_target_transcript(TranscriptScroll::Down(3));
        }
        self.sync_focus_out();
        true
    }

    fn scroll_target_transcript(&mut self, action: TranscriptScroll) {
        let target_id = self.transcript_scroll_target_id();
        let vp = self.transcript_viewport_mut(&target_id);
        match action {
            TranscriptScroll::Up(lines) => vp.scroll_up(lines),
            TranscriptScroll::Down(lines) => vp.scroll_down(lines),
            TranscriptScroll::Top => vp.scroll_to_top(),
            TranscriptScroll::Bottom => vp.scroll_to_bottom(),
        }
        self.sync_focus_out();
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
            KeyCode::Down | KeyCode::Char('j') if no_ctrl_alt => {
                if !self.tree.nodes.is_empty() {
                    let curr = self.tree.selected_index().unwrap_or(0);
                    if curr + 1 < self.tree.nodes.len() {
                        self.tree.select_index(curr + 1);
                        let (sel_id, _) = self.selected_tree_thread_info();
                        self.maybe_request_pty_snapshot(&sel_id);
                    }
                }
                None
            }
            KeyCode::Up | KeyCode::Char('k') if no_ctrl_alt => {
                if !self.tree.nodes.is_empty() {
                    let curr = self.tree.selected_index().unwrap_or(0);
                    if curr > 0 {
                        self.tree.select_index(curr - 1);
                        let (sel_id, _) = self.selected_tree_thread_info();
                        self.maybe_request_pty_snapshot(&sel_id);
                    }
                }
                None
            }
            KeyCode::Char('J') if no_ctrl_alt => {
                self.switch_root(true);
                None
            }
            KeyCode::Char('K') if no_ctrl_alt => {
                self.switch_root(false);
                None
            }
            KeyCode::Char('u' | 'U')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                let lines = self.half_page_scroll_lines();
                self.scroll_target_transcript(TranscriptScroll::Up(lines));
                None
            }
            KeyCode::Char('d' | 'D')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                let lines = self.half_page_scroll_lines();
                self.scroll_target_transcript(TranscriptScroll::Down(lines));
                None
            }
            KeyCode::Char('g') if no_ctrl_alt => {
                self.scroll_target_transcript(TranscriptScroll::Top);
                self.pending_g_prefix = true;
                None
            }
            KeyCode::Char('G') if no_ctrl_alt => {
                self.scroll_target_transcript(TranscriptScroll::Bottom);
                None
            }
            KeyCode::Char('f') if no_ctrl_alt => {
                let (source_thread_id, _) = self.selected_tree_thread_info();
                self.status_notice = None;
                self.pending_actions.push_back(AppAction::ForkBranch {
                    source_thread_id,
                    title: None,
                });
                None
            }
            KeyCode::Char('z') if no_ctrl_alt => {
                self.toggle_sidebar_zoom();
                None
            }
            KeyCode::Char('y') if no_ctrl_alt => {
                self.copy_transcript_content(CopyMode::LastReply);
                None
            }
            KeyCode::Char('n') if no_ctrl_alt => {
                self.status_notice = None;
                self.pending_actions
                    .push_back(AppAction::CreateRoot { title: None });
                None
            }
            KeyCode::Char('t') if no_ctrl_alt => {
                let (parent_thread_id, _) = self.selected_tree_thread_info();
                self.dispatch_create_terminal(Some(parent_thread_id), None);
                None
            }
            KeyCode::Char('T') if no_ctrl_alt => {
                self.dispatch_create_terminal(None, None);
                None
            }
            KeyCode::Char('r') if no_ctrl_alt => {
                self.begin_rename_prefill();
                None
            }
            KeyCode::Char('m') if no_ctrl_alt => {
                let (source_thread_id, _) = self.selected_tree_thread_info();
                self.request_join_thread(source_thread_id, None);
                None
            }
            KeyCode::Char('d') | KeyCode::Char('x') if no_ctrl_alt => {
                let (target_id, _) = self.selected_tree_thread_info();
                self.request_delete_thread(target_id);
                None
            }
            KeyCode::Char(' ') if no_ctrl_alt => {
                let (sel_id, _) = self.selected_tree_thread_info();
                self.toggle_tree_fold(&sel_id);
                None
            }
            KeyCode::Left | KeyCode::Char('h') if no_ctrl_alt => {
                self.fold_or_jump_parent_in_tree();
                None
            }
            KeyCode::Right | KeyCode::Char('l') if no_ctrl_alt => {
                let (sel_id, _) = self.selected_tree_thread_info();
                if self.collapsed_threads.remove(&sel_id) {
                    crate::runner::rebuild_tree_nodes(self);
                    None
                } else {
                    self.activate_selected_tree_node()
                }
            }
            KeyCode::Enter => self.activate_selected_tree_node(),
            _ => None,
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
            (KeyCode::Char('b' | 'B'), true, false) => {
                if let Some((idx, _)) = self.input_buffer[..pos].char_indices().next_back() {
                    self.cursor_position = idx;
                } else {
                    self.cursor_position = 0;
                }
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
        if key.kind == KeyEventKind::Release {
            return false;
        }
        self.sync_focus_in();

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        if ctrl && !alt && matches!(key.code, KeyCode::Char('q' | 'Q')) {
            self.pending_g_prefix = false;
            self.should_quit = true;
            self.sync_focus_out();
            return true;
        }

        if ctrl
            && !alt
            && matches!(key.code, KeyCode::Char('c' | 'C'))
            && (self.focused_pane != FocusedPane::Terminal || self.has_active_modal())
        {
            self.pending_g_prefix = false;
            self.should_quit = true;
            self.sync_focus_out();
            return true;
        }

        if self.focused_pane == FocusedPane::Terminal
            && ctrl
            && !alt
            && matches!(key.code, KeyCode::Char('g' | 'G'))
        {
            self.focus_tree();
            return true;
        }

        if self.pending_permission.is_some() {
            self.pending_g_prefix = false;
            let no_ctrl_alt = !ctrl && !alt;
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

        if self.config_picker.is_some() {
            self.pending_g_prefix = false;
            self.handle_key_config_picker(key);
            self.sync_focus_out();
            return true;
        }

        if self.show_help_modal {
            self.pending_g_prefix = false;
            let no_ctrl_alt = !ctrl && !alt;
            match key.code {
                KeyCode::Esc | KeyCode::Enter => {
                    self.show_help_modal = false;
                    self.help_scroll_offset = 0;
                }
                KeyCode::Char('q' | 'Q' | '?') if no_ctrl_alt => {
                    self.show_help_modal = false;
                    self.help_scroll_offset = 0;
                }
                KeyCode::Down | KeyCode::Char('j') if no_ctrl_alt => {
                    self.help_scroll_offset = self.help_scroll_offset.saturating_add(1);
                }
                KeyCode::Up | KeyCode::Char('k') if no_ctrl_alt => {
                    self.help_scroll_offset = self.help_scroll_offset.saturating_sub(1);
                }
                KeyCode::PageDown if no_ctrl_alt => {
                    self.help_scroll_offset = self.help_scroll_offset.saturating_add(5);
                }
                KeyCode::PageUp if no_ctrl_alt => {
                    self.help_scroll_offset = self.help_scroll_offset.saturating_sub(5);
                }
                KeyCode::Home if no_ctrl_alt => {
                    self.help_scroll_offset = 0;
                }
                _ => {}
            }
            self.sync_focus_out();
            return true;
        }

        if self.focused_pane == FocusedPane::Terminal {
            self.pending_g_prefix = false;
            if ctrl && !alt && matches!(key.code, KeyCode::Char('t' | 'T')) {
                self.toggle_sidebar_zoom();
                self.sync_focus_out();
                return true;
            }
            if self.is_terminal_exited(&self.active_thread_id) {
                if key.code == KeyCode::Esc && !ctrl && !alt {
                    self.focus_tree();
                    return true;
                }
                if key.code == KeyCode::Enter && !ctrl && !alt {
                    let (rows, cols) = self.current_terminal_dims_or_default();
                    self.pending_actions.push_back(AppAction::PtyRestart {
                        thread_id: self.active_thread_id.clone(),
                        rows,
                        cols,
                    });
                    self.sync_focus_out();
                    return true;
                }
                self.sync_focus_out();
                return false;
            }
            if (shift || alt) && !ctrl {
                match key.code {
                    KeyCode::PageUp => {
                        self.pending_actions.push_back(AppAction::PtyScroll {
                            thread_id: self.active_thread_id.clone(),
                            delta: 5,
                        });
                        self.sync_focus_out();
                        return true;
                    }
                    KeyCode::PageDown => {
                        self.pending_actions.push_back(AppAction::PtyScroll {
                            thread_id: self.active_thread_id.clone(),
                            delta: -5,
                        });
                        self.sync_focus_out();
                        return true;
                    }
                    _ => {}
                }
            }
            let app_cursor = self
                .pty_screens
                .get(&self.active_thread_id)
                .is_some_and(|s| s.application_cursor);
            if let Some(bytes) = crate::vt_input::encode_key_event(key, app_cursor) {
                if !bytes.is_empty() {
                    self.pending_actions.push_back(AppAction::PtyWrite {
                        thread_id: self.active_thread_id.clone(),
                        bytes,
                    });
                }
                self.sync_focus_out();
                return true;
            }
            self.sync_focus_out();
            return false;
        }

        let had_g_prefix = std::mem::take(&mut self.pending_g_prefix);

        if key.code == KeyCode::F(1) && !ctrl && !alt {
            self.help_scroll_offset = 0;
            self.show_help_modal = true;
            self.sync_focus_out();
            return true;
        }

        if ctrl && !alt && matches!(key.code, KeyCode::Char('f' | 'F')) {
            self.open_transcript_search(crate::search::SearchDirection::Forward);
            self.sync_focus_out();
            return true;
        }

        if self.transcript_search.editing {
            match key.code {
                KeyCode::Esc if !ctrl && !alt => {
                    self.transcript_search.clear();
                }
                KeyCode::Enter if !ctrl && !alt => {
                    self.transcript_search.confirm();
                    if !self.transcript_search.query.is_empty() {
                        self.recompute_transcript_search_and_scroll();
                    }
                }
                KeyCode::Backspace if !ctrl && !alt => {
                    self.transcript_search.query.pop();
                    self.recompute_transcript_search_and_scroll();
                }
                KeyCode::Char(c) if !ctrl && !alt => {
                    self.transcript_search.query.push(c);
                    self.recompute_transcript_search_and_scroll();
                }
                _ => {}
            }
            self.sync_focus_out();
            return true;
        }

        if matches!(self.focused_pane, FocusedPane::Tree | FocusedPane::Transcript) && !ctrl && !alt
        {
            match key.code {
                KeyCode::Char('?') => {
                    if had_g_prefix {
                        self.help_scroll_offset = 0;
                        self.show_help_modal = true;
                    } else {
                        self.open_transcript_search(crate::search::SearchDirection::Reverse);
                    }
                    self.sync_focus_out();
                    return true;
                }
                KeyCode::Char('/') => {
                    self.open_transcript_search(crate::search::SearchDirection::Forward);
                    self.sync_focus_out();
                    return true;
                }
                KeyCode::Char('n') if !self.transcript_search.query.is_empty() => {
                    self.step_transcript_search(true);
                    self.sync_focus_out();
                    return true;
                }
                KeyCode::Char('N') if !self.transcript_search.query.is_empty() => {
                    self.step_transcript_search(false);
                    self.sync_focus_out();
                    return true;
                }
                KeyCode::Esc if !self.transcript_search.query.is_empty() => {
                    self.transcript_search.clear();
                    self.sync_focus_out();
                    return true;
                }
                _ => {}
            }
        }

        // Global Transcript scrolling (active thread, or previewed thread when focused in Tree)
        match (key.code, ctrl, alt, shift) {
            (KeyCode::Up, false, true, false) => {
                self.scroll_target_transcript(TranscriptScroll::Up(6));
                return true;
            }
            (KeyCode::Down, false, true, false) => {
                self.scroll_target_transcript(TranscriptScroll::Down(6));
                return true;
            }
            (KeyCode::PageUp, false, false, _) => {
                self.scroll_target_transcript(TranscriptScroll::Up(10));
                return true;
            }
            (KeyCode::PageDown, false, false, _) => {
                self.scroll_target_transcript(TranscriptScroll::Down(10));
                return true;
            }
            (KeyCode::Home, false, true, _) => {
                self.scroll_target_transcript(TranscriptScroll::Top);
                return true;
            }
            (KeyCode::End, false, true, _) => {
                self.scroll_target_transcript(TranscriptScroll::Bottom);
                return true;
            }
            _ => {}
        }

        // Global Root switching: Alt+Shift+Up/Down and Alt+K/J
        match key.code {
            KeyCode::Up if alt && shift && !ctrl => {
                self.switch_root(false);
                self.sync_focus_out();
                return true;
            }
            KeyCode::Down if alt && shift && !ctrl => {
                self.switch_root(true);
                self.sync_focus_out();
                return true;
            }
            KeyCode::Char('k' | 'K') if alt && !ctrl => {
                self.switch_root(false);
                self.sync_focus_out();
                return true;
            }
            KeyCode::Char('j' | 'J') if alt && !ctrl => {
                self.switch_root(true);
                self.sync_focus_out();
                return true;
            }
            _ => {}
        }

        // Global Branch cycling within active root: Tab / Shift+Tab / BackTab
        match key.code {
            KeyCode::Tab if !ctrl && !alt => {
                if self.focused_pane == FocusedPane::Tree && !shift {
                    let (sel_id, _) = self.selected_tree_thread_info();
                    if self.is_terminal_thread(&sel_id) {
                        self.switch_thread(&sel_id);
                        self.focused_pane = FocusedPane::Terminal;
                        self.sync_focus_out();
                        return true;
                    }
                }
                self.cycle_branch_in_active_root(!shift);
                self.sync_focus_out();
                return true;
            }
            KeyCode::BackTab if !ctrl && !alt => {
                self.cycle_branch_in_active_root(false);
                self.sync_focus_out();
                return true;
            }
            _ => {}
        }

        if key.code == KeyCode::Char('?')
            && !ctrl
            && !alt
            && self.focused_pane == FocusedPane::Composer
            && self.input_buffer.is_empty()
        {
            self.help_scroll_offset = 0;
            self.show_help_modal = true;
            self.sync_focus_out();
            return true;
        }

        if ctrl && !alt {
            match key.code {
                KeyCode::Char('g' | 'G') => {
                    self.toggle_composer_tree_focus();
                    return true;
                }
                KeyCode::Char('n' | 'N') => {
                    self.pending_actions.push_back(AppAction::ForkBranch {
                        source_thread_id: self.active_thread_id.clone(),
                        title: None,
                    });
                    self.sync_focus_out();
                    return true;
                }
                KeyCode::Char('o' | 'O') => {
                    self.toggle_transcript_cards();
                    self.sync_focus_out();
                    return true;
                }
                KeyCode::Char('t' | 'T') => {
                    self.toggle_sidebar_zoom();
                    self.sync_focus_out();
                    return true;
                }
                _ => {}
            }
        }

        if alt && !ctrl {
            match key.code {
                KeyCode::Char('n' | 'N') => {
                    self.status_notice = None;
                    self.pending_actions
                        .push_back(AppAction::CreateRoot { title: None });
                    self.sync_focus_out();
                    return true;
                }
                KeyCode::Char('r' | 'R') => {
                    self.begin_rename_prefill();
                    return true;
                }
                KeyCode::Char('t' | 'T') => {
                    self.open_or_jump_to_terminal();
                    return true;
                }
                KeyCode::Char('y' | 'Y') => {
                    self.copy_transcript_content(CopyMode::LastReply);
                    self.sync_focus_out();
                    return true;
                }
                _ => {}
            }
        }

        if key.code == KeyCode::F(2) && !ctrl && !alt {
            self.begin_rename_prefill();
            return true;
        }

        if key.code == KeyCode::Esc && !ctrl && !alt {
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
                self.focus_tree();
                return true;
            }
            if self.focused_pane == FocusedPane::Tree {
                let _ = self.activate_selected_tree_node();
                return true;
            }
        }

        if self.focused_pane == FocusedPane::Composer {
            let handled = self.handle_key_composer(key);
            self.sync_focus_out();
            return handled;
        }

        if alt && !ctrl && matches!(key.code, KeyCode::Char('1'..='9')) {
            self.sync_focus_out();
            return false;
        }

        match key.code {
            KeyCode::Char('q') if !ctrl && !alt => {
                self.should_quit = true;
                self.sync_focus_out();
                return true;
            }
            KeyCode::Up if !ctrl && !alt && self.focused_pane == FocusedPane::Transcript => {
                let active = self.active_thread_id.clone();
                self.transcript_viewport_mut(&active).scroll_up(1);
                self.sync_focus_out();
                return true;
            }
            KeyCode::Down if !ctrl && !alt && self.focused_pane == FocusedPane::Transcript => {
                let active = self.active_thread_id.clone();
                self.transcript_viewport_mut(&active).scroll_down(1);
                self.sync_focus_out();
                return true;
            }
            KeyCode::Home if !ctrl && !alt && self.focused_pane == FocusedPane::Transcript => {
                let active = self.active_thread_id.clone();
                self.transcript_viewport_mut(&active).scroll_to_top();
                self.sync_focus_out();
                return true;
            }
            KeyCode::End if !ctrl && !alt && self.focused_pane == FocusedPane::Transcript => {
                let active = self.active_thread_id.clone();
                self.transcript_viewport_mut(&active).scroll_to_bottom();
                self.sync_focus_out();
                return true;
            }
            KeyCode::Char('u' | 'U') if ctrl && !alt && self.focused_pane == FocusedPane::Transcript => {
                let lines = self.half_page_scroll_lines();
                self.scroll_target_transcript(TranscriptScroll::Up(lines));
                return true;
            }
            KeyCode::Char('d' | 'D') if ctrl && !alt && self.focused_pane == FocusedPane::Transcript => {
                let lines = self.half_page_scroll_lines();
                self.scroll_target_transcript(TranscriptScroll::Down(lines));
                return true;
            }
            KeyCode::Char('g') if !ctrl && !alt && self.focused_pane == FocusedPane::Transcript => {
                self.scroll_target_transcript(TranscriptScroll::Top);
                self.pending_g_prefix = !had_g_prefix;
                return true;
            }
            KeyCode::Char('G') if !ctrl && !alt && self.focused_pane == FocusedPane::Transcript => {
                self.scroll_target_transcript(TranscriptScroll::Bottom);
                return true;
            }
            KeyCode::Char('t') if !ctrl && !alt && self.focused_pane == FocusedPane::Transcript => {
                self.dispatch_create_terminal(Some(self.active_thread_id.clone()), None);
                self.sync_focus_out();
                return true;
            }
            KeyCode::Char('T') if !ctrl && !alt && self.focused_pane == FocusedPane::Transcript => {
                self.dispatch_create_terminal(None, None);
                self.sync_focus_out();
                return true;
            }
            _ => {
                if self.focused_pane == FocusedPane::Tree {
                    let _ = self.handle_key_tree(key);
                    if key.code == KeyCode::Char('g') && !ctrl && !alt && had_g_prefix {
                        self.pending_g_prefix = false;
                    }
                    self.sync_focus_out();
                    return true;
                }
            }
        }

        self.sync_focus_out();
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn multi_root_app() -> App {
        let mut app = App {
            active_thread_id: "r1".to_string(),
            threads: vec![
                ThreadItem {
                    id: "r1".to_string(),
                    title: "Root 1".to_string(),
                    anchors: Vec::new(),
                },
                ThreadItem {
                    id: "b1".to_string(),
                    title: "Branch 1A".to_string(),
                    anchors: Vec::new(),
                },
                ThreadItem {
                    id: "g1".to_string(),
                    title: "Grand 1A1".to_string(),
                    anchors: Vec::new(),
                },
                ThreadItem {
                    id: "b2".to_string(),
                    title: "Branch 1B".to_string(),
                    anchors: Vec::new(),
                },
                ThreadItem {
                    id: "r2".to_string(),
                    title: "Root 2".to_string(),
                    anchors: Vec::new(),
                },
                ThreadItem {
                    id: "r2b".to_string(),
                    title: "Root 2 Branch".to_string(),
                    anchors: Vec::new(),
                },
                ThreadItem {
                    id: "r3".to_string(),
                    title: "Lone Root 3".to_string(),
                    anchors: Vec::new(),
                },
            ],
            ..App::default()
        };
        app.thread_meta.insert(
            "r1".to_string(),
            ThreadMeta {
                thread_id: "r1".to_string(),
                parent_thread_id: None,
                inherited_turn_count: 0,
                merged: false,
                kind: grein_core::ThreadKind::Agent,
                cwd: None,
                git_branch: None,
                git_dirty: 0,
            },
        );
        app.thread_meta.insert(
            "b1".to_string(),
            ThreadMeta {
                thread_id: "b1".to_string(),
                parent_thread_id: Some("r1".to_string()),
                inherited_turn_count: 0,
                merged: false,
                kind: grein_core::ThreadKind::Agent,
                cwd: None,
                git_branch: None,
                git_dirty: 0,
            },
        );
        app.thread_meta.insert(
            "g1".to_string(),
            ThreadMeta {
                thread_id: "g1".to_string(),
                parent_thread_id: Some("b1".to_string()),
                inherited_turn_count: 0,
                merged: false,
                kind: grein_core::ThreadKind::Agent,
                cwd: None,
                git_branch: None,
                git_dirty: 0,
            },
        );
        app.thread_meta.insert(
            "b2".to_string(),
            ThreadMeta {
                thread_id: "b2".to_string(),
                parent_thread_id: Some("r1".to_string()),
                inherited_turn_count: 0,
                merged: false,
                kind: grein_core::ThreadKind::Agent,
                cwd: None,
                git_branch: None,
                git_dirty: 0,
            },
        );
        app.thread_meta.insert(
            "r2".to_string(),
            ThreadMeta {
                thread_id: "r2".to_string(),
                parent_thread_id: None,
                inherited_turn_count: 0,
                merged: false,
                kind: grein_core::ThreadKind::Agent,
                cwd: None,
                git_branch: None,
                git_dirty: 0,
            },
        );
        app.thread_meta.insert(
            "r2b".to_string(),
            ThreadMeta {
                thread_id: "r2b".to_string(),
                parent_thread_id: Some("r2".to_string()),
                inherited_turn_count: 0,
                merged: false,
                kind: grein_core::ThreadKind::Agent,
                cwd: None,
                git_branch: None,
                git_dirty: 0,
            },
        );
        app.thread_meta.insert(
            "r3".to_string(),
            ThreadMeta {
                thread_id: "r3".to_string(),
                parent_thread_id: None,
                inherited_turn_count: 0,
                merged: false,
                kind: grein_core::ThreadKind::Agent,
                cwd: None,
                git_branch: None,
                git_dirty: 0,
            },
        );
        app.sync_anchors();
        app.select_active_thread_in_tree();
        app
    }

    #[test]
    fn test_initial_focus_is_composer() {
        let app = App::default();
        assert_eq!(app.focused_pane(), FocusedPane::Composer);
        assert_eq!(app.focus, FocusedPane::Composer);
    }

    #[test]
    fn test_focus_next_prev_ctrl_g_and_esc_toggle_composer_tree() {
        let mut app = App::default();
        assert_eq!(app.focused_pane(), FocusedPane::Composer);

        app.focus_next();
        assert_eq!(app.focused_pane(), FocusedPane::Tree);
        app.focus_prev();
        assert_eq!(app.focused_pane(), FocusedPane::Composer);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
        assert_eq!(app.focused_pane(), FocusedPane::Tree);
        app.handle_key_event(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
        assert_eq!(app.focused_pane(), FocusedPane::Composer);

        // Esc toggles Composer <-> Tree when idle, and Ctrl+G still toggles during streaming
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Tree);
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Composer);

        app.thread_statuses
            .insert(app.active_thread_id.clone(), "streaming".to_string());
        app.handle_key_event(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
        assert_eq!(app.focused_pane(), FocusedPane::Tree);
        assert!(app.pending_actions.is_empty());

        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(
            app.pending_actions.pop_front(),
            Some(AppAction::CancelTurn {
                thread_id: app.active_thread_id.clone(),
            })
        );
    }

    #[test]
    fn test_tab_and_shift_tab_cycle_branches_within_active_root() {
        let mut app = multi_root_app();
        assert_eq!(app.active_thread_id(), "r1");

        // Subtree for r1 in DFS order: [r1, b1, g1, b2]
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.active_thread_id(), "b1");
        assert_eq!(app.focused_pane(), FocusedPane::Composer);

        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.active_thread_id(), "g1");

        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.active_thread_id(), "b2");

        // Wraps back to r1 without crossing into r2
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.active_thread_id(), "r1");

        // Shift+Tab and BackTab step backward with wrap-around while preserving Tree focus
        app.set_focus(FocusedPane::Tree);
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT));
        assert_eq!(app.active_thread_id(), "b2");
        assert_eq!(app.focused_pane(), FocusedPane::Tree);

        app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(app.active_thread_id(), "g1");
        assert_eq!(app.focused_pane(), FocusedPane::Tree);

        // Lone root with no children stays a no-op
        app.switch_thread("r3");
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.active_thread_id(), "r3");
        app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(app.active_thread_id(), "r3");
    }

    #[test]
    fn test_alt_shift_arrows_and_alt_j_k_switch_roots_with_wrap() {
        let mut app = multi_root_app();
        app.switch_thread("g1"); // nested grandchild under r1
        app.set_focus(FocusedPane::Tree);

        app.handle_key_event(KeyEvent::new(
            KeyCode::Down,
            KeyModifiers::ALT | KeyModifiers::SHIFT,
        ));
        assert_eq!(app.active_thread_id(), "r2");
        assert_eq!(app.focused_pane(), FocusedPane::Tree);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT));
        assert_eq!(app.active_thread_id(), "r3");

        app.handle_key_event(KeyEvent::new(KeyCode::Char('J'), KeyModifiers::ALT));
        assert_eq!(app.active_thread_id(), "r1");

        app.handle_key_event(KeyEvent::new(
            KeyCode::Up,
            KeyModifiers::ALT | KeyModifiers::SHIFT,
        ));
        assert_eq!(app.active_thread_id(), "r3");

        app.handle_key_event(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::ALT));
        assert_eq!(app.active_thread_id(), "r2");

        // Alt+1..9 remain completely unbound in all panes
        for pane in [
            FocusedPane::Composer,
            FocusedPane::Tree,
            FocusedPane::Transcript,
        ] {
            app.set_focus(pane);
            for digit in '1'..='9' {
                assert!(!app.handle_key_event(KeyEvent::new(
                    KeyCode::Char(digit),
                    KeyModifiers::ALT
                )));
            }
        }
    }

    #[test]
    fn test_global_transcript_scroll_from_composer_and_tree_preview() {
        let mut app = multi_root_app();
        app.transcript_viewport_mut("r1").set_max_scroll(100);
        app.transcript_viewport_mut("r1").scroll_to_bottom();
        app.transcript_viewport_mut("b1").set_max_scroll(80);
        app.transcript_viewport_mut("b1").scroll_to_bottom();

        // From Composer: scrolls active thread r1; plain Home/End still move cursor
        app.set_focus(FocusedPane::Composer);
        app.input_buffer = "draft".to_string();
        app.cursor_position = 2;
        assert!(app.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT)));
        assert_eq!(app.transcript_viewport("r1").scroll_offset(), 94);
        assert!(app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::ALT)));
        assert_eq!(app.transcript_viewport("r1").scroll_offset(), 100);
        assert!(app.handle_key_event(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE)));
        assert_eq!(app.transcript_viewport("r1").scroll_offset(), 90);
        assert!(app.handle_key_event(KeyEvent::new(KeyCode::Home, KeyModifiers::ALT)));
        assert_eq!(app.transcript_viewport("r1").scroll_offset(), 0);
        assert!(app.handle_key_event(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE)));
        assert_eq!(app.transcript_viewport("r1").scroll_offset(), 10);
        assert!(app.handle_key_event(KeyEvent::new(KeyCode::End, KeyModifiers::ALT)));
        assert_eq!(app.transcript_viewport("r1").scroll_offset(), 100);

        assert!(app.handle_key_event(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE)));
        assert_eq!(app.cursor_position, 0);
        assert!(app.handle_key_event(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)));
        assert_eq!(app.cursor_position, 5);
        assert_eq!(app.transcript_viewport("r1").scroll_offset(), 100);

        // From Tree previewing b1: scrolls b1 without disturbing r1
        app.set_focus(FocusedPane::Tree);
        app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.active_thread_id(), "r1");
        assert!(app.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT)));
        assert_eq!(app.transcript_viewport("b1").scroll_offset(), 74);
        assert_eq!(app.transcript_viewport("r1").scroll_offset(), 100);
    }

    #[test]
    fn test_composer_typing_immunity_for_q_and_brackets() {
        let mut app = App::new();
        app.set_focus(FocusPane::Composer);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Char('['), KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));

        assert!(!app.should_quit());
        assert_eq!(app.composer_content(), "q[]");
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
    fn test_composer_command_parsing_and_root_join_guard() {
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

        // /join and Tree 'm' on a root context set status_notice without emitting MergeBranch
        app.input_buffer = "/join".to_string();
        app.cursor_position = app.input_buffer.len();
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.pending_actions.is_empty());
        assert_eq!(
            app.status_notice.as_deref(),
            Some("Cannot join a root context (no parent branch)")
        );

        app.status_notice = None;
        app.set_focus(FocusedPane::Tree);
        app.handle_key_event(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE));
        assert!(app.pending_actions.is_empty());
        assert_eq!(
            app.status_notice.as_deref(),
            Some("Cannot join a root context (no parent branch)")
        );

        app.set_focus(FocusedPane::Composer);
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
        app.sync_anchors();

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
}
