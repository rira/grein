use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::io::IsTerminal;
use thiserror::Error;

use crate::engine_port::{
    AgentConfigChoice, AgentConfigOption, AgentSlashCommand, PtyScreenSnapshot, SubagentSnapshot,
    ThreadMeta, WorktreeEntry,
};
use crate::keymap::{BindableAction, Keymap};
use crate::runner::{compute_ordered_thread_entries, OrderedThreadEntry};
use crate::theme::{ThemePalette, ThemePickerModal, ThemePreset};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidebarView {
    #[default]
    Grein,
    Worktrees,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingWorktreeDelete {
    pub path: String,
    pub label: String,
    pub dirty_count: usize,
    pub attached_count: usize,
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingThreadDelete {
    pub thread_id: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingQuitConfirm {
    pub active_terminals: usize,
    pub busy_agents: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenamePromptState {
    pub thread_id: String,
    pub buffer: String,
    pub cursor: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidebarMode {
    Fixed,
    #[default]
    Dynamic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarLayoutConfig {
    pub mode: SidebarMode,
    pub min_pct: u16,
    pub max_pct: u16,
    pub fixed_cols: u16,
    pub fixed_pct: Option<u16>,
    pub overflow: bool,
}

impl Default for SidebarLayoutConfig {
    fn default() -> Self {
        Self {
            mode: SidebarMode::Dynamic,
            min_pct: 18,
            max_pct: 32,
            fixed_cols: 30,
            fixed_pct: None,
            overflow: true,
        }
    }
}

impl SidebarLayoutConfig {
    pub fn apply_ui_prefs(&mut self, ui: &BTreeMap<String, String>) {
        if let Some(mode) = ui.get("sidebar_mode") {
            match mode.trim().to_ascii_lowercase().as_str() {
                "fixed" => self.mode = SidebarMode::Fixed,
                "dynamic" => self.mode = SidebarMode::Dynamic,
                _ => {}
            }
        }
        if let Some(min_str) = ui.get("sidebar_min_pct") {
            if let Ok(v) = min_str.trim().trim_end_matches('%').parse::<u16>() {
                self.min_pct = v.clamp(10, 50);
            }
        }
        if let Some(max_str) = ui.get("sidebar_max_pct") {
            if let Ok(v) = max_str.trim().trim_end_matches('%').parse::<u16>() {
                self.max_pct = v.clamp(self.min_pct, 60);
            }
        }
        if self.max_pct < self.min_pct {
            self.max_pct = self.min_pct;
        }
        if let Some(cols_str) = ui.get("sidebar_fixed_cols") {
            if let Ok(v) = cols_str.trim().parse::<u16>() {
                self.fixed_cols = v.clamp(16, 80);
            }
        }
        if let Some(pct_str) = ui.get("sidebar_fixed_pct") {
            let trimmed = pct_str.trim().trim_end_matches('%');
            if trimmed.is_empty() {
                self.fixed_pct = None;
            } else if let Ok(v) = trimmed.parse::<u16>() {
                self.fixed_pct = Some(v.clamp(10, 60));
            }
        }
        if let Some(ov_str) = ui.get("sidebar_overflow") {
            match ov_str.trim().to_ascii_lowercase().as_str() {
                "on" | "true" | "1" | "yes" => self.overflow = true,
                "off" | "false" | "0" | "no" => self.overflow = false,
                _ => {}
            }
        }
    }

    pub fn summary(&self) -> String {
        let ov = if self.overflow { "on" } else { "off" };
        match self.mode {
            SidebarMode::Dynamic => {
                format!(
                    "Sidebar: dynamic {}%..{}% (overflow {ov})",
                    self.min_pct, self.max_pct
                )
            }
            SidebarMode::Fixed => {
                if let Some(pct) = self.fixed_pct {
                    format!("Sidebar: fixed {pct}% (overflow {ov})")
                } else {
                    format!("Sidebar: fixed {} cols (overflow {ov})", self.fixed_cols)
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeRow {
    RepoHeader {
        repo_root: String,
        repo_name: String,
        count: usize,
        collapsed: bool,
    },
    Entry(WorktreeEntry),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeAutocompleteItem {
    CreateNew,
    Existing(WorktreeEntry),
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
    ListWorktrees,
    SwitchThreadWorktree {
        thread_id: String,
        target_path: String,
    },
    RemoveWorktree {
        path: String,
        force: bool,
    },
    FetchWorktreeDiff {
        path: String,
    },
    PruneWorktrees,
    SaveThemePreference {
        theme: String,
    },
    SaveKeybindingPreference {
        action: String,
        binding: String,
    },
    RemoveKeybindingPreference {
        action: String,
    },
    SaveUiPreference {
        key: String,
        value: String,
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
    pub help_max_scroll: u16,
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
    pub sidebar_view: SidebarView,
    pub worktrees: Vec<WorktreeEntry>,
    pub worktree_selected_idx: usize,
    pub worktree_scroll_offset: usize,
    pub collapsed_worktree_repos: HashSet<String>,
    pub worktree_filter_active: bool,
    pub worktree_filter_query: String,
    pub pending_worktree_delete: Option<PendingWorktreeDelete>,
    pub worktree_autocomplete_open: bool,
    pub worktree_autocomplete_idx: usize,
    pub worktree_list_in_flight: bool,
    pub theme_preset: ThemePreset,
    pub theme_overrides: BTreeMap<String, String>,
    pub theme_palette: ThemePalette,
    pub theme_picker: Option<ThemePickerModal>,
    pub keymap: Keymap,
    pub sidebar_layout: SidebarLayoutConfig,
    pub palette_active: bool,
    pub palette_input: String,
    pub palette_cursor: usize,
    pub palette_match_idx: usize,
    pub rename_prompt: Option<RenamePromptState>,
    pub pending_thread_delete: Option<PendingThreadDelete>,
    pub pending_quit_confirm: Option<PendingQuitConfirm>,
    pub worktree_diff_path: Option<String>,
    pub worktree_diff_preview: String,
    pub worktree_diff_scroll: u16,
    pub worktree_diff_max_scroll: u16,
    pub worktree_diff_in_flight: Option<String>,
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
            help_max_scroll: 0,
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
            sidebar_view: SidebarView::Grein,
            worktrees: Vec::new(),
            worktree_selected_idx: 0,
            worktree_scroll_offset: 0,
            collapsed_worktree_repos: HashSet::new(),
            worktree_filter_active: false,
            worktree_filter_query: String::new(),
            pending_worktree_delete: None,
            worktree_autocomplete_open: false,
            worktree_autocomplete_idx: 0,
            worktree_list_in_flight: false,
            theme_preset: ThemePreset::TokyoNight,
            theme_overrides: BTreeMap::new(),
            theme_palette: ThemePalette::from_preset(ThemePreset::TokyoNight),
            theme_picker: None,
            keymap: Keymap::default(),
            sidebar_layout: SidebarLayoutConfig::default(),
            palette_active: false,
            palette_input: String::new(),
            palette_cursor: 0,
            palette_match_idx: 0,
            rename_prompt: None,
            pending_thread_delete: None,
            pending_quit_confirm: None,
            worktree_diff_path: None,
            worktree_diff_preview: String::new(),
            worktree_diff_scroll: 0,
            worktree_diff_max_scroll: 0,
            worktree_diff_in_flight: None,
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

pub fn display_thread_title(thread: &ThreadItem, meta: Option<&ThreadMeta>) -> String {
    let trimmed = thread.title.trim();
    if !trimmed.is_empty() && trimmed != "Terminal" {
        return trimmed.to_string();
    }
    if meta.is_some_and(|m| m.kind == grein_core::ThreadKind::Terminal) || trimmed == "Terminal" {
        if let Some(cwd) = meta.and_then(|m| m.cwd.as_deref()) {
            let raw = cwd.trim();
            let clean = raw.trim_end_matches('/');
            if clean.is_empty() && raw.starts_with('/') {
                return "/".to_string();
            }
            if let Some(name) = std::path::Path::new(clean)
                .file_name()
                .and_then(|n| n.to_str())
                .filter(|s| !s.is_empty())
            {
                return name.to_string();
            }
        }
        return "Terminal".to_string();
    }
    if trimmed.is_empty() {
        "Untitled".to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn shorten_terminal_cwd_with_home(cwd: &str, home_dir: Option<&str>) -> String {
    let trimmed = cwd.trim();
    if let Some(idx) = trimmed.find("/.grein/worktrees/") {
        let after = &trimmed[idx + "/.grein/worktrees/".len()..];
        let wt_part = after.trim_matches('/');
        if !wt_part.is_empty() {
            return format!("⎇ {wt_part}");
        }
    }
    if let Some(home) = home_dir.map(str::trim).filter(|h| !h.is_empty()) {
        let home_clean = home.trim_end_matches('/');
        if trimmed == home_clean {
            return "~".to_string();
        }
        if let Some(rest) = trimmed.strip_prefix(home_clean) {
            if rest.starts_with('/') {
                return format!("~{rest}");
            }
        }
    }
    trimmed.to_string()
}

pub fn shorten_terminal_cwd(cwd: &str) -> String {
    let home = std::env::var("HOME").ok();
    shorten_terminal_cwd_with_home(cwd, home.as_deref())
}

pub(crate) fn is_altgr_char(c: char, ctrl: bool, alt: bool) -> bool {
    (!ctrl && !alt) || (ctrl && alt && !c.is_ascii_control())
}

pub(crate) fn parse_config_arg(rest: &str) -> (bool, String) {
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        return (false, String::new());
    }
    if let Some(after) = trimmed.strip_prefix("--default") {
        if after.is_empty() || after.starts_with(char::is_whitespace) {
            return (true, after.trim().to_string());
        }
    }
    if let Some(before) = trimmed.strip_suffix("--default") {
        if before.ends_with(char::is_whitespace) {
            return (true, before.trim().to_string());
        }
    }
    let mut split = trimmed.splitn(2, char::is_whitespace);
    if let (Some(first), Some(second)) = (split.next(), split.next()) {
        if first.eq_ignore_ascii_case("default") && !second.trim().is_empty() {
            return (true, second.trim().to_string());
        }
    }
    (false, trimmed.to_string())
}

pub(crate) fn parse_term_cwd(rest: &str) -> Option<String> {
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        return None;
    }
    let unquoted = if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        trimmed[1..trimmed.len() - 1].trim()
    } else {
        trimmed
    };
    (!unquoted.is_empty()).then(|| unquoted.to_string())
}

pub fn palette_command_specs() -> &'static [(&'static str, &'static str)] {
    &[
        ("theme", "Open theme picker or set theme"),
        ("bind", "Bind WRITE key: :bind <action> <chord>"),
        ("unbind", "Reset/unbind action: :unbind <action>"),
        ("keybinds", "Show live keybindings"),
        ("sidebar", "Configure sidebar width/dynamic/overflow"),
        ("worktree", "Ensure or switch thread worktree"),
        ("worktrees", "Switch to Worktrees sidebar view"),
        ("prune", "Prune stale git worktrees"),
        ("term", "Spawn terminal shell [path]"),
        ("fork", "Fork selected branch"),
        ("new", "Create new root agent thread"),
        ("rename", "Rename thread"),
        ("join", "Join branch into parent"),
        ("copy", "Copy reply/screen/code/all"),
        ("zoom", "Toggle sidebar zoom"),
        ("help", "Open keybindings help"),
        ("quit", "Quit Grein"),
        ("q", "Quit Grein"),
        ("exit", "Quit Grein"),
    ]
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

pub fn worktree_display_label(entry: &WorktreeEntry) -> String {
    if let Some(branch) = entry
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty())
    {
        branch.to_string()
    } else if let Some(sha) = entry
        .head_short
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        format!("detached@{sha}")
    } else if entry.is_detached {
        "detached@HEAD".to_string()
    } else {
        std::path::Path::new(&entry.path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&entry.path)
            .to_string()
    }
}

pub(crate) fn parse_worktree_autocomplete_query(input: &str) -> Option<&str> {
    if input.contains('\n') || input.contains('\r') {
        return None;
    }
    if input.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("/wt ")) {
        return Some(&input[4..]);
    }
    if input
        .get(..10)
        .is_some_and(|p| p.eq_ignore_ascii_case("/worktree "))
    {
        return Some(&input[10..]);
    }
    None
}

fn worktree_matches_sidebar_filter(entry: &WorktreeEntry, query_lower: &str) -> bool {
    if query_lower.is_empty() {
        return true;
    }
    entry.repo_name.to_ascii_lowercase().contains(query_lower)
        || entry
            .branch
            .as_deref()
            .is_some_and(|b| b.to_ascii_lowercase().contains(query_lower))
        || entry.path.to_ascii_lowercase().contains(query_lower)
        || entry
            .head_short
            .as_deref()
            .is_some_and(|h| h.to_ascii_lowercase().contains(query_lower))
}

pub(crate) fn worktree_matches_autocomplete(entry: &WorktreeEntry, query_lower: &str) -> bool {
    if query_lower.is_empty() {
        return true;
    }
    let basename = std::path::Path::new(&entry.path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    entry
        .branch
        .as_deref()
        .is_some_and(|b| b.to_ascii_lowercase().contains(query_lower))
        || basename.to_ascii_lowercase().contains(query_lower)
        || entry.repo_name.to_ascii_lowercase().contains(query_lower)
        || entry.path.to_ascii_lowercase().contains(query_lower)
}

fn normalized_path_components(path: &str) -> Vec<std::path::Component<'_>> {
    std::path::Path::new(path.trim()).components().collect()
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
        self.maybe_queue_terminal_resize();
    }

    fn open_or_jump_to_terminal(&mut self) {
        self.pending_rename_thread_id = None;
        let target_id =
            if self.focused_pane == FocusedPane::Tree && self.sidebar_view == SidebarView::Grein {
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
            let layout = crate::render::compute_app_layout(self, area, strip_h, true);
            let term = layout.transcript_area;
            let cols = if self.sidebar_hidden {
                term.width
            } else {
                term.width.saturating_sub(2)
            };
            let rows = term.height.saturating_sub(2);
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
        let layout = crate::render::compute_app_layout(self, area, strip_h, true);
        let term = layout.transcript_area;
        let cols = if self.sidebar_hidden {
            term.width
        } else {
            term.width.saturating_sub(2)
        };
        let rows = term.height.saturating_sub(2);
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

    pub(crate) fn has_active_modal(&self) -> bool {
        self.pending_permission.is_some()
            || self.pending_worktree_delete.is_some()
            || self.pending_thread_delete.is_some()
            || self.pending_quit_confirm.is_some()
            || self.theme_picker.is_some()
            || self.config_picker.is_some()
            || self.show_help_modal
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
            self.sync_worktree_autocomplete(true);
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
        self.sync_worktree_autocomplete(true);
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
        self.sync_worktree_autocomplete(false);
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
            if self.sidebar_view == SidebarView::Worktrees {
                self.worktree_filter_active = false;
                self.focused_pane = if self.active_is_terminal() {
                    FocusedPane::Terminal
                } else {
                    FocusedPane::Composer
                };
                self.sync_focus_out();
            } else {
                let _ = self.activate_selected_tree_node();
            }
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
        if self.focused_pane == FocusedPane::Tree && self.sidebar_view == SidebarView::Grein {
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
        if self.sidebar_hidden && self.focused_pane == FocusedPane::Tree {
            self.palette_active = false;
            self.rename_prompt = None;
            self.worktree_filter_active = false;
            self.focused_pane = if self.active_is_terminal() {
                FocusedPane::Terminal
            } else {
                FocusedPane::Composer
            };
            self.sync_focus_out();
        }
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
            KeyCode::Char('c' | 'C') if ctrl && !alt => {
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
            let layout = crate::render::compute_app_layout(self, area, strip_h, false);
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
                    let composer_hidden = self.active_is_terminal()
                        || self.sidebar_view == SidebarView::Worktrees
                        || self.pending_rename_thread_id.is_some();
                    let layout =
                        crate::render::compute_app_layout(self, area, strip_h, composer_hidden);
                    !layout
                        .transcript_area
                        .contains(ratatui::layout::Position::new(mouse.column, mouse.row))
                }
            });

        if self.sidebar_view == SidebarView::Worktrees && !over_tree_pane {
            if is_up {
                self.worktree_diff_scroll = self.worktree_diff_scroll.saturating_sub(3);
            } else {
                self.worktree_diff_scroll = self
                    .worktree_diff_scroll
                    .saturating_add(3)
                    .min(self.worktree_diff_max_scroll);
            }
            self.sync_focus_out();
            return true;
        }

        if self.active_is_terminal() && !over_tree_pane {
            self.pending_actions.push_back(AppAction::PtyScroll {
                thread_id: self.active_thread_id.clone(),
                delta,
            });
            self.sync_focus_out();
            return true;
        }

        if over_tree_pane {
            if self.sidebar_view == SidebarView::Worktrees {
                let row_count = self.visible_worktree_rows().len();
                if row_count > 0 {
                    if is_up {
                        self.worktree_selected_idx = self.worktree_selected_idx.saturating_sub(1);
                    } else if self.worktree_selected_idx + 1 < row_count {
                        self.worktree_selected_idx += 1;
                    }
                    self.maybe_fetch_selected_worktree_diff(false);
                }
                self.sync_focus_out();
                return true;
            }
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

    pub fn begin_inline_rename(&mut self) {
        let (target_id, target_title) =
            if self.focused_pane == FocusedPane::Tree && self.sidebar_view == SidebarView::Grein {
                self.selected_tree_thread_info()
            } else {
                let id = self.active_thread_id.clone();
                let title = self.thread_title_or_empty(&id);
                (id, title)
            };
        if target_id.is_empty() {
            return;
        }
        self.pending_rename_thread_id = None;
        self.palette_active = false;
        self.worktree_filter_active = false;
        self.rename_prompt = Some(RenamePromptState {
            thread_id: target_id,
            cursor: target_title.len(),
            buffer: target_title,
        });
        self.sync_focus_out();
    }

    pub fn apply_preferences(&mut self, prefs: &crate::engine_port::UserPreferencesSnapshot) {
        let preset = ThemePalette::resolve_preset(prefs.theme.as_deref(), &prefs.theme_overrides);
        self.theme_preset = preset;
        self.theme_overrides = prefs.theme_overrides.clone();
        self.theme_palette =
            ThemePalette::from_prefs(prefs.theme.as_deref(), &prefs.theme_overrides);
        self.keymap = Keymap::from_prefs(&prefs.keybindings);
        self.sidebar_layout.apply_ui_prefs(&prefs.ui);
        self.maybe_queue_terminal_resize();
    }

    pub fn open_theme_picker(&mut self) {
        let selected_idx = ThemePreset::all()
            .iter()
            .position(|&p| p == self.theme_preset)
            .unwrap_or(0);
        self.palette_active = false;
        self.rename_prompt = None;
        self.theme_picker = Some(ThemePickerModal {
            selected_idx,
            original_preset: self.theme_preset,
            original_overrides: self.theme_overrides.clone(),
        });
    }

    pub fn preview_theme_at(&mut self, idx: usize) {
        let presets = ThemePreset::all();
        if presets.is_empty() {
            return;
        }
        let clamped = idx.min(presets.len() - 1);
        if let Some(ref mut picker) = self.theme_picker {
            picker.selected_idx = clamped;
        }
        let preset = presets[clamped];
        self.theme_preset = preset;
        self.theme_palette = ThemePalette::from_preset(preset);
    }

    pub fn apply_theme_by_name(&mut self, name: &str) {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            self.open_theme_picker();
            return;
        }
        let Some(preset) = ThemePreset::from_str(trimmed) else {
            self.status_notice = Some(format!(
                "Unknown theme '{trimmed}'. Available: tokyo-night, catppuccin-mocha, nord, ansi"
            ));
            return;
        };
        self.theme_preset = preset;
        self.theme_overrides.clear();
        self.theme_palette = ThemePalette::from_preset(preset);
        self.pending_actions
            .push_back(AppAction::SaveThemePreference {
                theme: preset.as_str().to_string(),
            });
        self.status_notice = Some(format!("Theme: {}", preset.display_name()));
    }

    pub fn selected_worktree_entry(&self) -> Option<WorktreeEntry> {
        self.visible_worktree_rows()
            .get(self.worktree_selected_idx)
            .and_then(|row| match row {
                WorktreeRow::Entry(entry) => Some(entry.clone()),
                WorktreeRow::RepoHeader { .. } => None,
            })
    }

    pub fn maybe_fetch_selected_worktree_diff(&mut self, force: bool) {
        if self.sidebar_view != SidebarView::Worktrees {
            return;
        }
        let Some(entry) = self.selected_worktree_entry() else {
            self.worktree_diff_path = None;
            self.worktree_diff_preview.clear();
            self.worktree_diff_scroll = 0;
            self.worktree_diff_max_scroll = 0;
            self.worktree_diff_in_flight = None;
            return;
        };
        if !force {
            if self.worktree_diff_path.as_deref() == Some(entry.path.as_str()) {
                return;
            }
            if self.worktree_diff_in_flight.as_deref() == Some(entry.path.as_str()) {
                return;
            }
        }
        self.worktree_diff_path = Some(entry.path.clone());
        self.worktree_diff_preview.clear();
        self.worktree_diff_scroll = 0;
        self.worktree_diff_max_scroll = 0;
        self.worktree_diff_in_flight = Some(entry.path.clone());
        self.pending_actions
            .push_back(AppAction::FetchWorktreeDiff { path: entry.path });
    }

    pub fn request_prune_worktrees(&mut self) {
        self.status_notice = Some("Pruning stale worktrees...".to_string());
        self.pending_actions.push_back(AppAction::PruneWorktrees);
    }

    pub fn open_command_palette(&mut self, initial: &str) {
        self.status_notice = None;
        self.pending_g_prefix = false;
        self.rename_prompt = None;
        self.worktree_filter_active = false;
        self.sidebar_hidden = false;
        self.focused_pane = FocusedPane::Tree;
        self.palette_active = true;
        self.palette_input = initial.to_string();
        self.palette_cursor = self.palette_input.len();
        self.palette_match_idx = 0;
    }

    pub fn palette_suggestions(&self) -> Vec<(String, String)> {
        let trimmed = self
            .palette_input
            .trim_start_matches([':', '/'])
            .trim_start();
        let needle = trimmed
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let mut prefix_matches = Vec::new();
        let mut substring_matches = Vec::new();
        for &(cmd, desc) in palette_command_specs() {
            if needle.is_empty() || cmd.starts_with(&needle) {
                prefix_matches.push((cmd.to_string(), desc.to_string()));
            } else if cmd.contains(&needle) || desc.to_ascii_lowercase().contains(&needle) {
                substring_matches.push((cmd.to_string(), desc.to_string()));
            }
        }
        prefix_matches.extend(substring_matches);
        prefix_matches
    }

    pub fn active_thread_is_busy(&self) -> bool {
        matches!(
            self.thread_statuses
                .get(&self.active_thread_id)
                .map(String::as_str),
            Some("streaming" | "executing" | "thinking")
        )
    }

    pub fn selected_or_active_thread_id(&self) -> String {
        if self.focused_pane == FocusedPane::Tree && self.sidebar_view == SidebarView::Grein {
            let sel = self.selected_tree_thread_info().0;
            if !sel.is_empty() {
                return sel;
            }
        }
        self.active_thread_id.clone()
    }

    pub fn request_quit_with_confirm(&mut self) {
        let mut terminal_ids: BTreeSet<&str> = self
            .thread_meta
            .values()
            .filter(|m| m.kind == grein_core::ThreadKind::Terminal)
            .map(|m| m.thread_id.as_str())
            .collect();
        for id in self.pty_screens.keys() {
            terminal_ids.insert(id.as_str());
        }
        let active_terminals = terminal_ids
            .into_iter()
            .filter(|id| !self.is_terminal_exited(id))
            .count();
        let busy_agents = self
            .thread_statuses
            .values()
            .filter(|s| matches!(s.as_str(), "streaming" | "executing" | "thinking"))
            .count();
        if active_terminals > 0 || busy_agents > 0 {
            self.pending_quit_confirm = Some(PendingQuitConfirm {
                active_terminals,
                busy_agents,
            });
        } else {
            self.should_quit = true;
        }
    }

    pub(crate) fn execute_bindable_action(&mut self, action: BindableAction) {
        match action {
            BindableAction::EnterManageMode => {
                if self.focused_pane == FocusedPane::Tree {
                    self.toggle_composer_tree_focus();
                } else {
                    self.focus_tree();
                    if self.sidebar_view == SidebarView::Worktrees {
                        self.maybe_fetch_selected_worktree_diff(false);
                    }
                }
            }
            BindableAction::ToggleZoom => {
                self.toggle_sidebar_zoom();
            }
            BindableAction::ToggleWorktrees => {
                self.toggle_worktrees_sidebar_view();
            }
            BindableAction::ScrollUp => {
                if self.sidebar_view == SidebarView::Worktrees
                    && self.focused_pane == FocusedPane::Tree
                {
                    self.worktree_diff_scroll = self.worktree_diff_scroll.saturating_sub(10);
                } else if self.active_is_terminal() && self.sidebar_view != SidebarView::Worktrees
                {
                    self.pending_actions.push_back(AppAction::PtyScroll {
                        thread_id: self.active_thread_id.clone(),
                        delta: 5,
                    });
                } else {
                    self.scroll_target_transcript(TranscriptScroll::Up(10));
                }
            }
            BindableAction::ScrollDown => {
                if self.sidebar_view == SidebarView::Worktrees
                    && self.focused_pane == FocusedPane::Tree
                {
                    self.worktree_diff_scroll = self
                        .worktree_diff_scroll
                        .saturating_add(10)
                        .min(self.worktree_diff_max_scroll);
                } else if self.active_is_terminal() && self.sidebar_view != SidebarView::Worktrees
                {
                    self.pending_actions.push_back(AppAction::PtyScroll {
                        thread_id: self.active_thread_id.clone(),
                        delta: -5,
                    });
                } else {
                    self.scroll_target_transcript(TranscriptScroll::Down(10));
                }
            }
            BindableAction::JumpTerminal => {
                self.open_or_jump_to_terminal();
            }
            BindableAction::NewAgent => {
                self.status_notice = None;
                self.pending_actions
                    .push_back(AppAction::CreateRoot { title: None });
            }
            BindableAction::ForkThread => {
                let source_thread_id = self.selected_or_active_thread_id();
                self.status_notice = None;
                self.pending_actions.push_back(AppAction::ForkBranch {
                    source_thread_id,
                    title: None,
                });
            }
            BindableAction::Worktree => {
                let thread_id = self.selected_or_active_thread_id();
                self.status_notice = Some("Allocating worktree...".to_string());
                self.pending_actions
                    .push_back(AppAction::EnsureThreadWorktree { thread_id });
            }
            BindableAction::CommandPalette => {
                self.open_command_palette("");
            }
            BindableAction::Help => {
                self.help_scroll_offset = 0;
                self.show_help_modal = true;
            }
            BindableAction::ToggleCards => {
                self.toggle_transcript_cards();
            }
            BindableAction::CopyReply => {
                self.copy_transcript_content(CopyMode::LastReply);
            }
            BindableAction::Quit => {
                self.request_quit_with_confirm();
            }
        }
    }

    pub fn display_title_for(&self, thread_id: &str, fallback_title: &str) -> String {
        let item = self
            .threads
            .iter()
            .find(|t| t.id == thread_id)
            .cloned()
            .unwrap_or_else(|| ThreadItem {
                id: thread_id.to_string(),
                title: fallback_title.to_string(),
                anchors: Vec::new(),
            });
        let pty_cwd = self.pty_screens.get(thread_id).and_then(|s| s.cwd.clone());
        let meta = self.thread_meta.get(thread_id);
        match (meta, pty_cwd) {
            (Some(m), Some(cwd)) if m.cwd.as_deref().is_none_or(|c| c.trim().is_empty()) => {
                let mut patched = m.clone();
                patched.cwd = Some(cwd);
                display_thread_title(&item, Some(&patched))
            }
            (Some(m), _) => display_thread_title(&item, Some(m)),
            (None, Some(cwd)) if item.title.trim() == "Terminal" || item.title.trim().is_empty() => {
                let synth = ThreadMeta {
                    thread_id: thread_id.to_string(),
                    parent_thread_id: None,
                    inherited_turn_count: 0,
                    merged: false,
                    kind: grein_core::ThreadKind::Terminal,
                    cwd: Some(cwd),
                    git_branch: None,
                    git_dirty: 0,
                };
                display_thread_title(&item, Some(&synth))
            }
            (None, _) => display_thread_title(&item, None),
        }
    }

    pub fn execute_slash_command(&mut self, trimmed: &str, from_palette: bool) -> bool {
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let head = parts.next().unwrap_or("");
        let head_lower = head.to_ascii_lowercase();
        let rest = parts.next().unwrap_or("").trim();

        if head_lower != "/rename" && !matches!(head_lower.as_str(), "/?" | "/help") {
            self.pending_rename_thread_id = None;
        }

        match head_lower.as_str() {
            "/?" | "/help" => {
                self.help_scroll_offset = 0;
                self.show_help_modal = true;
                true
            }
            "/zoom" => {
                self.toggle_sidebar_zoom();
                true
            }
            "/copy" => {
                match rest.to_ascii_lowercase().as_str() {
                    "" => self.copy_transcript_content(CopyMode::LastReply),
                    "code" => self.copy_transcript_content(CopyMode::CodeBlock),
                    "all" => self.copy_transcript_content(CopyMode::All),
                    _ => {
                        self.status_notice = Some("Usage: /copy [code|all]".to_string());
                    }
                }
                true
            }
            "/commands" => {
                let active_id = self.active_thread_id.clone();
                if let Some(cmds) = self.thread_commands.get(&active_id).cloned() {
                    if !cmds.is_empty() {
                        self.record_agent_commands_note(&active_id, &cmds);
                        return true;
                    }
                }
                self.status_notice = Some("Fetching agent commands...".to_string());
                self.pending_actions
                    .push_back(AppAction::FetchAgentCommands {
                        thread_id: active_id,
                    });
                true
            }
            "/model" | "/mode" | "/thinking" | "/effort" => {
                let target_config_id = match head_lower.as_str() {
                    "/model" => "model",
                    "/mode" => "mode",
                    _ => "thought_level",
                };
                let (set_default, query) = parse_config_arg(rest);
                let active_id = self.active_thread_id.clone();
                if query.is_empty() {
                    self.pending_actions
                        .push_back(AppAction::OpenConfigPicker {
                            thread_id: active_id,
                            config_id: target_config_id.to_string(),
                        });
                    return true;
                }
                if let Some(opt) = self
                    .thread_config_options
                    .get(&active_id)
                    .and_then(|opts| find_config_option(opts, target_config_id))
                    .cloned()
                {
                    if opt.options.is_empty() {
                        self.pending_actions.push_back(AppAction::SetAgentConfig {
                            thread_id: active_id,
                            config_id: opt.id,
                            value: query,
                            set_default,
                        });
                    } else if let Some(choice) = match_config_choice(&opt.options, &query) {
                        self.pending_actions.push_back(AppAction::SetAgentConfig {
                            thread_id: active_id,
                            config_id: opt.id,
                            value: choice.value.clone(),
                            set_default,
                        });
                    } else {
                        self.status_notice =
                            Some(format!("No {} option matching '{query}'", opt.name));
                    }
                } else {
                    self.pending_actions
                        .push_back(AppAction::ApplyAgentConfigQuery {
                            thread_id: active_id,
                            config_id: target_config_id.to_string(),
                            query,
                            set_default,
                        });
                }
                true
            }
            "/delete" | "/close" => {
                let target_id = if rest.is_empty() {
                    if from_palette {
                        Some(self.selected_or_active_thread_id())
                    } else {
                        Some(self.active_thread_id.clone())
                    }
                } else {
                    self.threads
                        .iter()
                        .find(|t| t.id == rest || t.title == rest)
                        .map(|t| t.id.clone())
                };
                if let Some(id) = target_id {
                    self.request_delete_thread(id);
                } else {
                    self.status_notice = Some(format!("Unknown context: {rest}"));
                }
                true
            }
            "/branch" | "/fork" | "/grein" => {
                let title = (!rest.is_empty()).then(|| rest.to_string());
                let source_thread_id = if from_palette {
                    self.selected_or_active_thread_id()
                } else {
                    self.active_thread_id.clone()
                };
                self.pending_actions.push_back(AppAction::ForkBranch {
                    source_thread_id,
                    title,
                });
                true
            }
            "/root" | "/new" => {
                let title = (!rest.is_empty()).then(|| rest.to_string());
                self.status_notice = None;
                self.pending_actions
                    .push_back(AppAction::CreateRoot { title });
                true
            }
            "/rename" => {
                if rest.is_empty() {
                    if from_palette {
                        self.begin_inline_rename();
                    } else {
                        self.status_notice = Some("Usage: /rename <new title>".to_string());
                    }
                } else {
                    let thread_id = self.pending_rename_thread_id.take().unwrap_or_else(|| {
                        if from_palette {
                            self.selected_or_active_thread_id()
                        } else {
                            self.active_thread_id.clone()
                        }
                    });
                    self.status_notice = None;
                    self.pending_actions.push_back(AppAction::RenameThread {
                        thread_id,
                        title: rest.to_string(),
                    });
                    self.sync_focus_for_active_thread();
                }
                true
            }
            "/join" | "/merge" => {
                let source_thread_id = if from_palette {
                    self.selected_or_active_thread_id()
                } else {
                    self.active_thread_id.clone()
                };
                let summary = (!rest.is_empty()).then(|| rest.to_string());
                self.request_join_thread(source_thread_id, summary);
                true
            }
            "/term" | "/terminal" | "/zsh" | "/sh" | "/shell" | "/bash" => {
                let cwd = parse_term_cwd(rest);
                let parent_id = if from_palette {
                    self.selected_or_active_thread_id()
                } else {
                    self.active_thread_id.clone()
                };
                self.dispatch_create_terminal(Some(parent_id), cwd);
                true
            }
            "/worktrees" => {
                let filter = (!rest.is_empty()).then(|| rest.to_string());
                self.open_worktrees_sidebar_view(filter);
                true
            }
            "/worktree" | "/wt" => {
                let target_thread = if from_palette {
                    self.selected_or_active_thread_id()
                } else {
                    self.active_thread_id.clone()
                };
                if rest.is_empty() {
                    self.status_notice = Some("Allocating worktree...".to_string());
                    self.pending_actions
                        .push_back(AppAction::EnsureThreadWorktree {
                            thread_id: target_thread,
                        });
                } else {
                    let query = parse_term_cwd(rest).unwrap_or_else(|| rest.to_string());
                    if let Some(target_path) = self.resolve_worktree_query_target(&query) {
                        self.status_notice = Some("Switching worktree...".to_string());
                        self.pending_actions
                            .push_back(AppAction::SwitchThreadWorktree {
                                thread_id: target_thread,
                                target_path,
                            });
                    } else {
                        self.open_worktrees_sidebar_view(Some(query));
                    }
                }
                true
            }
            "/prune" => {
                self.request_prune_worktrees();
                true
            }
            "/theme" => {
                self.apply_theme_by_name(rest);
                true
            }
            "/bind" => {
                self.execute_bind_command(rest);
                true
            }
            "/unbind" => {
                self.execute_unbind_command(rest);
                true
            }
            "/keybinds" | "/binds" => {
                self.execute_keybinds_command(rest);
                true
            }
            "/sidebar" => {
                self.execute_sidebar_layout_command(rest);
                true
            }
            "/quit" | "/q" | "/exit" => {
                self.should_quit = true;
                true
            }
            _ => false,
        }
    }

    pub fn execute_sidebar_command(&mut self, raw: &str) {
        let trimmed = raw.trim().trim_start_matches([':', '/']).trim();
        if trimmed.is_empty() {
            return;
        }
        let normalized = format!("/{trimmed}");
        if !self.execute_slash_command(&normalized, true) {
            let cmd_name = trimmed.split_whitespace().next().unwrap_or(trimmed);
            self.status_notice = Some(format!("Unknown sidebar command: :{cmd_name}"));
        }
    }

    fn execute_sidebar_layout_command(&mut self, rest: &str) {
        let tokens: Vec<&str> = rest.split_whitespace().collect();
        if tokens.is_empty() {
            self.status_notice = Some(self.sidebar_layout.summary());
            return;
        }
        let first = tokens[0].to_ascii_lowercase();
        match first.as_str() {
            "dynamic" | "auto" => {
                self.sidebar_layout.mode = SidebarMode::Dynamic;
                self.pending_actions.push_back(AppAction::SaveUiPreference {
                    key: "sidebar_mode".to_string(),
                    value: "dynamic".to_string(),
                });
                if let Some(min_tok) = tokens.get(1) {
                    if let Ok(v) = min_tok.trim_end_matches('%').parse::<u16>() {
                        let clamped = v.clamp(10, 50);
                        self.sidebar_layout.min_pct = clamped;
                        if self.sidebar_layout.max_pct < clamped {
                            self.sidebar_layout.max_pct = clamped;
                            self.pending_actions.push_back(AppAction::SaveUiPreference {
                                key: "sidebar_max_pct".to_string(),
                                value: self.sidebar_layout.max_pct.to_string(),
                            });
                        }
                        self.pending_actions.push_back(AppAction::SaveUiPreference {
                            key: "sidebar_min_pct".to_string(),
                            value: clamped.to_string(),
                        });
                    }
                }
                if let Some(max_tok) = tokens.get(2) {
                    if let Ok(v) = max_tok.trim_end_matches('%').parse::<u16>() {
                        let clamped = v.clamp(self.sidebar_layout.min_pct, 60);
                        self.sidebar_layout.max_pct = clamped;
                        self.pending_actions.push_back(AppAction::SaveUiPreference {
                            key: "sidebar_max_pct".to_string(),
                            value: clamped.to_string(),
                        });
                    }
                }
            }
            "fixed" => {
                self.sidebar_layout.mode = SidebarMode::Fixed;
                self.pending_actions.push_back(AppAction::SaveUiPreference {
                    key: "sidebar_mode".to_string(),
                    value: "fixed".to_string(),
                });
                if let Some(val_tok) = tokens.get(1) {
                    if let Some(pct_str) = val_tok.strip_suffix('%') {
                        if let Ok(v) = pct_str.parse::<u16>() {
                            let clamped = v.clamp(10, 60);
                            self.sidebar_layout.fixed_pct = Some(clamped);
                            self.pending_actions.push_back(AppAction::SaveUiPreference {
                                key: "sidebar_fixed_pct".to_string(),
                                value: clamped.to_string(),
                            });
                        }
                    } else if let Ok(v) = val_tok.parse::<u16>() {
                        let clamped = v.clamp(16, 80);
                        self.sidebar_layout.fixed_cols = clamped;
                        self.sidebar_layout.fixed_pct = None;
                        self.pending_actions.push_back(AppAction::SaveUiPreference {
                            key: "sidebar_fixed_cols".to_string(),
                            value: clamped.to_string(),
                        });
                        self.pending_actions.push_back(AppAction::SaveUiPreference {
                            key: "sidebar_fixed_pct".to_string(),
                            value: String::new(),
                        });
                    }
                }
            }
            "min" => {
                if let Some(val_tok) = tokens.get(1) {
                    if let Ok(v) = val_tok.trim_end_matches('%').parse::<u16>() {
                        let clamped = v.clamp(10, 50);
                        self.sidebar_layout.min_pct = clamped;
                        self.pending_actions.push_back(AppAction::SaveUiPreference {
                            key: "sidebar_min_pct".to_string(),
                            value: clamped.to_string(),
                        });
                        if self.sidebar_layout.max_pct < clamped {
                            self.sidebar_layout.max_pct = clamped;
                            self.pending_actions.push_back(AppAction::SaveUiPreference {
                                key: "sidebar_max_pct".to_string(),
                                value: clamped.to_string(),
                            });
                        }
                    } else {
                        self.status_notice = Some("Usage: :sidebar min <10..50%>".to_string());
                        return;
                    }
                } else {
                    self.status_notice = Some("Usage: :sidebar min <10..50%>".to_string());
                    return;
                }
            }
            "max" => {
                if let Some(val_tok) = tokens.get(1) {
                    if let Ok(v) = val_tok.trim_end_matches('%').parse::<u16>() {
                        let clamped = v.clamp(self.sidebar_layout.min_pct, 60);
                        self.sidebar_layout.max_pct = clamped;
                        self.pending_actions.push_back(AppAction::SaveUiPreference {
                            key: "sidebar_max_pct".to_string(),
                            value: clamped.to_string(),
                        });
                    } else {
                        self.status_notice = Some("Usage: :sidebar max <min..60%>".to_string());
                        return;
                    }
                } else {
                    self.status_notice = Some("Usage: :sidebar max <min..60%>".to_string());
                    return;
                }
            }
            "overflow" => {
                let next = match tokens.get(1).map(|s| s.to_ascii_lowercase()).as_deref() {
                    Some("on" | "true" | "1" | "yes") => true,
                    Some("off" | "false" | "0" | "no") => false,
                    None => !self.sidebar_layout.overflow,
                    _ => {
                        self.status_notice = Some("Usage: :sidebar overflow <on|off>".to_string());
                        return;
                    }
                };
                self.sidebar_layout.overflow = next;
                self.pending_actions.push_back(AppAction::SaveUiPreference {
                    key: "sidebar_overflow".to_string(),
                    value: if next { "on" } else { "off" }.to_string(),
                });
            }
            other => {
                if let Some(pct_str) = other.strip_suffix('%') {
                    if let Ok(v) = pct_str.parse::<u16>() {
                        let clamped = v.clamp(10, 60);
                        self.sidebar_layout.mode = SidebarMode::Fixed;
                        self.sidebar_layout.fixed_pct = Some(clamped);
                        self.pending_actions.push_back(AppAction::SaveUiPreference {
                            key: "sidebar_mode".to_string(),
                            value: "fixed".to_string(),
                        });
                        self.pending_actions.push_back(AppAction::SaveUiPreference {
                            key: "sidebar_fixed_pct".to_string(),
                            value: clamped.to_string(),
                        });
                    } else {
                        self.status_notice = Some(
                            "Usage: :sidebar [dynamic [min max]|fixed <cols|pct%>|min <%>|max <%>|overflow <on|off>]"
                                .to_string(),
                        );
                        return;
                    }
                } else if let Ok(v) = other.parse::<u16>() {
                    let clamped = v.clamp(16, 80);
                    self.sidebar_layout.mode = SidebarMode::Fixed;
                    self.sidebar_layout.fixed_cols = clamped;
                    self.sidebar_layout.fixed_pct = None;
                    self.pending_actions.push_back(AppAction::SaveUiPreference {
                        key: "sidebar_mode".to_string(),
                        value: "fixed".to_string(),
                    });
                    self.pending_actions.push_back(AppAction::SaveUiPreference {
                        key: "sidebar_fixed_cols".to_string(),
                        value: clamped.to_string(),
                    });
                    self.pending_actions.push_back(AppAction::SaveUiPreference {
                        key: "sidebar_fixed_pct".to_string(),
                        value: String::new(),
                    });
                } else {
                    self.status_notice = Some(
                        "Usage: :sidebar [dynamic [min max]|fixed <cols|pct%>|min <%>|max <%>|overflow <on|off>]"
                            .to_string(),
                    );
                    return;
                }
            }
        }
        self.maybe_queue_terminal_resize();
        self.status_notice = Some(self.sidebar_layout.summary());
    }

    fn execute_bind_command(&mut self, rest: &str) {
        let mut parts = rest.splitn(2, char::is_whitespace);
        let action_str = parts.next().unwrap_or("").trim();
        let chord_str = parts.next().unwrap_or("").trim();
        if action_str.is_empty() {
            self.execute_keybinds_command("");
            return;
        }
        let Some(action) = BindableAction::from_str(action_str) else {
            self.status_notice = Some(format!(
                "Unknown action '{action_str}'. Try: enter_manage_mode, toggle_zoom, toggle_worktrees, scroll_up, scroll_down, jump_terminal, new_agent, fork_thread, worktree, command_palette, help"
            ));
            return;
        };
        if chord_str.is_empty() {
            self.status_notice = Some(format!(
                "{} = {} (usage: :bind {} <chord>)",
                action.as_str(),
                self.keymap.display_for(action),
                action.as_str()
            ));
            return;
        }
        match self.keymap.bind(action, chord_str) {
            Ok(canonical) => {
                self.pending_actions
                    .push_back(AppAction::SaveKeybindingPreference {
                        action: action.as_str().to_string(),
                        binding: canonical,
                    });
                self.status_notice = Some(format!(
                    "Bound {} -> {}",
                    action.as_str(),
                    self.keymap.display_for(action)
                ));
            }
            Err(err) => {
                self.status_notice = Some(format!("Invalid keybind: {err}"));
            }
        }
    }

    fn execute_unbind_command(&mut self, rest: &str) {
        let action_str = rest.trim();
        if action_str.is_empty() {
            self.status_notice = Some("Usage: :unbind <action>".to_string());
            return;
        }
        let Some(action) = BindableAction::from_str(action_str) else {
            self.status_notice = Some(format!("Unknown action '{action_str}'"));
            return;
        };
        self.keymap.unbind(action);
        self.pending_actions
            .push_back(AppAction::RemoveKeybindingPreference {
                action: action.as_str().to_string(),
            });
        self.status_notice = Some(format!(
            "Reset {} -> {}",
            action.as_str(),
            self.keymap.display_for(action)
        ));
    }

    fn execute_keybinds_command(&mut self, rest: &str) {
        if rest.trim().eq_ignore_ascii_case("custom") {
            let custom = self.keymap.custom_entries();
            if custom.is_empty() {
                self.status_notice = Some("No custom keybindings; defaults active".to_string());
            } else {
                let summary = custom
                    .into_iter()
                    .map(|(a, d)| format!("{}={d}", a.as_str()))
                    .collect::<Vec<_>>()
                    .join(" · ");
                self.status_notice = Some(format!("Custom keybinds: {summary}"));
            }
            return;
        }
        let summary = self
            .keymap
            .all_entries()
            .into_iter()
            .map(|(a, d)| format!("{}={d}", a.as_str()))
            .collect::<Vec<_>>()
            .join(" · ");
        self.status_notice = Some(format!("WRITE keybinds: {summary}"));
    }

    fn handle_key_theme_picker(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let no_ctrl_alt = !ctrl && !alt;
        let count = ThemePreset::all().len();
        match key.code {
            KeyCode::Up | KeyCode::Char('k' | 'K') if no_ctrl_alt => {
                if let Some(ref picker) = self.theme_picker {
                    let next = picker.selected_idx.saturating_sub(1);
                    self.preview_theme_at(next);
                }
            }
            KeyCode::Down | KeyCode::Char('j' | 'J') if no_ctrl_alt => {
                if let Some(ref picker) = self.theme_picker {
                    let next = if count == 0 {
                        0
                    } else {
                        (picker.selected_idx + 1).min(count - 1)
                    };
                    self.preview_theme_at(next);
                }
            }
            KeyCode::Home if no_ctrl_alt => {
                self.preview_theme_at(0);
            }
            KeyCode::End if no_ctrl_alt => {
                if count > 0 {
                    self.preview_theme_at(count - 1);
                }
            }
            KeyCode::Enter if no_ctrl_alt => {
                if let Some(picker) = self.theme_picker.take() {
                    let preset = ThemePreset::all()
                        .get(picker.selected_idx)
                        .copied()
                        .unwrap_or(ThemePreset::TokyoNight);
                    self.theme_preset = preset;
                    self.theme_overrides.clear();
                    self.theme_palette = ThemePalette::from_preset(preset);
                    self.pending_actions
                        .push_back(AppAction::SaveThemePreference {
                            theme: preset.as_str().to_string(),
                        });
                    self.status_notice = Some(format!("Theme: {}", preset.display_name()));
                }
            }
            KeyCode::Esc | KeyCode::Char('q' | 'Q') if no_ctrl_alt => {
                if let Some(picker) = self.theme_picker.take() {
                    self.theme_preset = picker.original_preset;
                    self.theme_overrides = picker.original_overrides;
                    self.theme_palette = ThemePalette::from_prefs(
                        Some(self.theme_preset.as_str()),
                        &self.theme_overrides,
                    );
                    self.status_notice = Some("Theme preview cancelled".to_string());
                }
            }
            KeyCode::Char('c' | 'C') if ctrl && !alt => {
                if let Some(picker) = self.theme_picker.take() {
                    self.theme_preset = picker.original_preset;
                    self.theme_overrides = picker.original_overrides;
                    self.theme_palette = ThemePalette::from_prefs(
                        Some(self.theme_preset.as_str()),
                        &self.theme_overrides,
                    );
                    self.status_notice = Some("Theme preview cancelled".to_string());
                }
            }
            _ => {}
        }
    }

    fn handle_key_palette(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let pos = clamp_char_boundary(&self.palette_input, self.palette_cursor);
        let suggestions = self.palette_suggestions();

        match (key.code, ctrl, alt) {
            (KeyCode::Esc, false, false) | (KeyCode::Char('c' | 'C'), true, false) => {
                self.palette_active = false;
                self.palette_input.clear();
                self.palette_cursor = 0;
                self.palette_match_idx = 0;
            }
            (KeyCode::Enter, false, false) => {
                let raw = std::mem::take(&mut self.palette_input);
                self.palette_active = false;
                self.palette_cursor = 0;
                self.palette_match_idx = 0;
                if !raw.trim().is_empty() {
                    self.execute_sidebar_command(&raw);
                }
            }
            (KeyCode::Tab, false, false) => {
                if !suggestions.is_empty() {
                    let idx = self.palette_match_idx.min(suggestions.len() - 1);
                    self.palette_input = suggestions[idx].0.clone();
                    self.palette_cursor = self.palette_input.len();
                    self.palette_match_idx = 0;
                }
            }
            (KeyCode::Up, false, false) | (KeyCode::Char('p' | 'P'), true, false) => {
                if !suggestions.is_empty() {
                    let len = suggestions.len();
                    let cur = self.palette_match_idx.min(len - 1);
                    self.palette_match_idx = (cur + len - 1) % len;
                }
            }
            (KeyCode::Down, false, false) | (KeyCode::Char('n' | 'N'), true, false) => {
                if !suggestions.is_empty() {
                    let len = suggestions.len();
                    let cur = self.palette_match_idx.min(len - 1);
                    self.palette_match_idx = (cur + 1) % len;
                }
            }
            (KeyCode::Char('a' | 'A'), true, false) | (KeyCode::Home, false, false) => {
                self.palette_cursor = 0;
            }
            (KeyCode::Char('e' | 'E'), true, false) | (KeyCode::End, false, false) => {
                self.palette_cursor = self.palette_input.len();
            }
            (KeyCode::Char('u' | 'U'), true, false) => {
                self.palette_input.drain(..pos);
                self.palette_cursor = 0;
                self.palette_match_idx = 0;
            }
            (KeyCode::Char('w' | 'W'), true, false) => {
                let start = prev_word_boundary(&self.palette_input, pos);
                self.palette_input.drain(start..pos);
                self.palette_cursor = start;
                self.palette_match_idx = 0;
            }
            (KeyCode::Left, false, false) if pos > 0 => {
                if let Some((idx, _)) = self.palette_input[..pos].char_indices().next_back() {
                    self.palette_cursor = idx;
                }
            }
            (KeyCode::Right, false, false) if pos < self.palette_input.len() => {
                if let Some(ch) = self.palette_input[pos..].chars().next() {
                    self.palette_cursor = pos + ch.len_utf8();
                }
            }
            (KeyCode::Backspace, false, false) => {
                if self.palette_input.is_empty() {
                    self.palette_active = false;
                    self.palette_cursor = 0;
                    self.palette_match_idx = 0;
                } else if pos > 0 {
                    if let Some((idx, _)) = self.palette_input[..pos].char_indices().next_back() {
                        self.palette_input.remove(idx);
                        self.palette_cursor = idx;
                        self.palette_match_idx = 0;
                    }
                }
            }
            (KeyCode::Delete, false, false) if pos < self.palette_input.len() => {
                self.palette_input.remove(pos);
                self.palette_cursor = pos;
                self.palette_match_idx = 0;
            }
            (KeyCode::Char(c), _, _) if is_altgr_char(c, ctrl, alt) => {
                self.palette_input.insert(pos, c);
                self.palette_cursor = pos + c.len_utf8();
                self.palette_match_idx = 0;
            }
            _ => {}
        }
    }

    fn handle_key_rename_prompt(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let Some(ref mut prompt) = self.rename_prompt else {
            return;
        };
        let pos = clamp_char_boundary(&prompt.buffer, prompt.cursor);
        match (key.code, ctrl, alt) {
            (KeyCode::Esc, false, false) | (KeyCode::Char('c' | 'C'), true, false) => {
                self.rename_prompt = None;
                self.status_notice = Some("Rename cancelled".to_string());
            }
            (KeyCode::Enter, false, false) => {
                let prompt = self.rename_prompt.take().expect("checked");
                let new_title = prompt.buffer.trim().to_string();
                if new_title.is_empty() {
                    self.status_notice = Some("Rename title cannot be empty".to_string());
                } else {
                    self.status_notice = None;
                    self.pending_actions.push_back(AppAction::RenameThread {
                        thread_id: prompt.thread_id,
                        title: new_title,
                    });
                }
            }
            (KeyCode::Char('a' | 'A'), true, false) | (KeyCode::Home, false, false) => {
                prompt.cursor = 0;
            }
            (KeyCode::Char('e' | 'E'), true, false) | (KeyCode::End, false, false) => {
                prompt.cursor = prompt.buffer.len();
            }
            (KeyCode::Char('u' | 'U'), true, false) => {
                prompt.buffer.drain(..pos);
                prompt.cursor = 0;
            }
            (KeyCode::Char('w' | 'W'), true, false) => {
                let start = prev_word_boundary(&prompt.buffer, pos);
                prompt.buffer.drain(start..pos);
                prompt.cursor = start;
            }
            (KeyCode::Left, false, false) if pos > 0 => {
                if let Some((idx, _)) = prompt.buffer[..pos].char_indices().next_back() {
                    prompt.cursor = idx;
                }
            }
            (KeyCode::Right, false, false) if pos < prompt.buffer.len() => {
                if let Some(ch) = prompt.buffer[pos..].chars().next() {
                    prompt.cursor = pos + ch.len_utf8();
                }
            }
            (KeyCode::Backspace, false, false) if pos > 0 => {
                if let Some((idx, _)) = prompt.buffer[..pos].char_indices().next_back() {
                    prompt.buffer.remove(idx);
                    prompt.cursor = idx;
                }
            }
            (KeyCode::Delete, false, false) if pos < prompt.buffer.len() => {
                prompt.buffer.remove(pos);
                prompt.cursor = pos;
            }
            (KeyCode::Char(c), _, _) if is_altgr_char(c, ctrl, alt) => {
                prompt.buffer.insert(pos, c);
                prompt.cursor = pos + c.len_utf8();
            }
            _ => {}
        }
    }

    pub fn active_worktree_path(&self) -> Option<String> {
        let active_id = self.active_thread_id();
        let cwd = self
            .pty_screens
            .get(active_id)
            .and_then(|s| s.cwd.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                self.thread_meta
                    .get(active_id)
                    .and_then(|m| m.cwd.as_deref())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            });
        if let Some(cwd) = cwd {
            let cwd_comps = normalized_path_components(cwd);
            if cwd_comps.is_empty() {
                return None;
            }
            return self
                .worktrees
                .iter()
                .filter_map(|w| {
                    let w_comps = normalized_path_components(&w.path);
                    if !w_comps.is_empty() && cwd_comps.starts_with(&w_comps) {
                        Some((
                            (w_comps.len(), w.path.trim_end_matches('/').len()),
                            w.path.clone(),
                        ))
                    } else {
                        None
                    }
                })
                .max_by_key(|(score, _)| *score)
                .map(|(_, path)| path);
        }
        self.worktrees
            .iter()
            .find(|w| w.attached_thread_ids.iter().any(|id| id == active_id))
            .map(|w| w.path.clone())
    }

    pub fn active_worktree_repo_root(&self) -> Option<String> {
        if let Some(active_path) = self.active_worktree_path() {
            if let Some(entry) = self.worktrees.iter().find(|w| w.path == active_path) {
                return Some(entry.repo_root.clone());
            }
        }
        let active_id = self.active_thread_id();
        self.worktrees
            .iter()
            .find(|w| w.attached_thread_ids.iter().any(|id| id == active_id))
            .map(|w| w.repo_root.clone())
    }

    pub fn visible_worktree_rows(&self) -> Vec<WorktreeRow> {
        let q = self.worktree_filter_query.trim().to_ascii_lowercase();
        let mut groups: Vec<(String, String, Vec<WorktreeEntry>)> = Vec::new();
        for entry in &self.worktrees {
            if let Some(last) = groups.last_mut() {
                if last.0 == entry.repo_root {
                    last.2.push(entry.clone());
                    continue;
                }
            }
            groups.push((
                entry.repo_root.clone(),
                entry.repo_name.clone(),
                vec![entry.clone()],
            ));
        }

        let mut rows = Vec::new();
        for (repo_root, repo_name, entries) in groups {
            let repo_matches = !q.is_empty() && repo_name.to_ascii_lowercase().contains(&q);
            let filtered: Vec<WorktreeEntry> = if q.is_empty() || repo_matches {
                entries
            } else {
                entries
                    .into_iter()
                    .filter(|e| worktree_matches_sidebar_filter(e, &q))
                    .collect()
            };
            if filtered.is_empty() {
                continue;
            }
            let collapsed = self.collapsed_worktree_repos.contains(&repo_root);
            rows.push(WorktreeRow::RepoHeader {
                repo_root,
                repo_name,
                count: filtered.len(),
                collapsed,
            });
            if !collapsed {
                for entry in filtered {
                    rows.push(WorktreeRow::Entry(entry));
                }
            }
        }
        rows
    }

    pub fn clamp_worktree_selection(&mut self) {
        let count = self.visible_worktree_rows().len();
        if count == 0 {
            self.worktree_selected_idx = 0;
            self.worktree_scroll_offset = 0;
        } else {
            self.worktree_selected_idx = self.worktree_selected_idx.min(count - 1);
        }
    }

    pub fn worktree_autocomplete_candidates(&self) -> Vec<WorktreeAutocompleteItem> {
        let query = parse_worktree_autocomplete_query(&self.input_buffer)
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let active_repo = self.active_worktree_repo_root();
        let mut active_entries = Vec::new();
        let mut other_entries = Vec::new();
        for entry in &self.worktrees {
            if !query.is_empty() && !worktree_matches_autocomplete(entry, &query) {
                continue;
            }
            if active_repo.as_deref() == Some(entry.repo_root.as_str()) {
                active_entries.push(WorktreeAutocompleteItem::Existing(entry.clone()));
            } else {
                other_entries.push(WorktreeAutocompleteItem::Existing(entry.clone()));
            }
        }
        let mut out = Vec::with_capacity(active_entries.len() + other_entries.len() + 1);
        if query.is_empty() {
            out.push(WorktreeAutocompleteItem::CreateNew);
            out.extend(active_entries);
            out.extend(other_entries);
        } else {
            out.extend(active_entries);
            out.extend(other_entries);
            out.push(WorktreeAutocompleteItem::CreateNew);
        }
        out
    }

    pub fn sync_worktree_autocomplete(&mut self, buffer_changed: bool) {
        let eligible = self.focused_pane == FocusedPane::Composer
            && !self.has_active_modal()
            && parse_worktree_autocomplete_query(&self.input_buffer).is_some();
        if !eligible {
            self.worktree_autocomplete_open = false;
            self.worktree_autocomplete_idx = 0;
            return;
        }
        if buffer_changed {
            let was_open = self.worktree_autocomplete_open;
            self.worktree_autocomplete_open = true;
            self.worktree_autocomplete_idx = 0;
            if !was_open && !self.worktree_list_in_flight {
                self.worktree_list_in_flight = true;
                self.pending_actions.push_back(AppAction::ListWorktrees);
            }
        }
        let count = self.worktree_autocomplete_candidates().len();
        if count == 0 {
            self.worktree_autocomplete_idx = 0;
        } else {
            self.worktree_autocomplete_idx = self.worktree_autocomplete_idx.min(count - 1);
        }
    }

    pub fn toggle_worktrees_sidebar_view(&mut self) {
        self.pending_rename_thread_id = None;
        self.pending_g_prefix = false;
        self.worktree_filter_active = false;
        if self.sidebar_hidden {
            self.sidebar_hidden = false;
            self.sidebar_view = SidebarView::Worktrees;
            self.focused_pane = FocusedPane::Tree;
            self.clamp_worktree_selection();
            self.worktree_list_in_flight = true;
            self.pending_actions.push_back(AppAction::ListWorktrees);
            self.maybe_fetch_selected_worktree_diff(false);
            self.sync_focus_out();
            self.maybe_queue_terminal_resize();
            return;
        }
        match self.sidebar_view {
            SidebarView::Grein => {
                self.sidebar_view = SidebarView::Worktrees;
                self.focused_pane = FocusedPane::Tree;
                self.clamp_worktree_selection();
                self.worktree_list_in_flight = true;
                self.pending_actions.push_back(AppAction::ListWorktrees);
                self.maybe_fetch_selected_worktree_diff(false);
            }
            SidebarView::Worktrees => {
                self.sidebar_view = SidebarView::Grein;
                self.focused_pane = FocusedPane::Tree;
            }
        }
        self.sync_focus_out();
    }

    pub fn open_worktrees_sidebar_view(&mut self, filter_query: Option<String>) {
        self.pending_rename_thread_id = None;
        self.pending_g_prefix = false;
        self.sidebar_hidden = false;
        self.sidebar_view = SidebarView::Worktrees;
        self.worktree_filter_active = false;
        self.worktree_filter_query = filter_query.unwrap_or_default();
        self.worktree_selected_idx = 0;
        self.worktree_scroll_offset = 0;
        self.clamp_worktree_selection();
        self.focused_pane = FocusedPane::Tree;
        self.worktree_list_in_flight = true;
        self.pending_actions.push_back(AppAction::ListWorktrees);
        self.maybe_fetch_selected_worktree_diff(false);
        self.sync_focus_out();
        self.maybe_queue_terminal_resize();
    }

    pub fn resolve_worktree_query_target(&self, query: &str) -> Option<String> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return None;
        }
        if let Some(exact) = self.worktrees.iter().find(|w| w.path == trimmed) {
            return Some(exact.path.clone());
        }
        let q_lower = trimmed.to_ascii_lowercase();
        let matches: Vec<&WorktreeEntry> = self
            .worktrees
            .iter()
            .filter(|w| worktree_matches_autocomplete(w, &q_lower))
            .collect();
        if matches.len() == 1 {
            Some(matches[0].path.clone())
        } else {
            None
        }
    }

    fn toggle_worktree_repo_fold(&mut self, repo_root: &str) {
        if !self.collapsed_worktree_repos.insert(repo_root.to_string()) {
            self.collapsed_worktree_repos.remove(repo_root);
        }
        self.clamp_worktree_selection();
        let rows = self.visible_worktree_rows();
        if let Some(hdr_idx) = rows.iter().position(|r| {
            matches!(r, WorktreeRow::RepoHeader { repo_root: rr, .. } if rr == repo_root)
        }) {
            if self.collapsed_worktree_repos.contains(repo_root) {
                self.worktree_selected_idx = hdr_idx;
            }
        }
    }

    fn handle_key_worktree_autocomplete(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let candidates = self.worktree_autocomplete_candidates();
        if candidates.is_empty() {
            return false;
        }
        let idx = self.worktree_autocomplete_idx.min(candidates.len() - 1);

        match key.code {
            KeyCode::Down if !ctrl && !alt => {
                self.worktree_autocomplete_idx = (idx + 1) % candidates.len();
                true
            }
            KeyCode::Up if !ctrl && !alt => {
                self.worktree_autocomplete_idx = (idx + candidates.len() - 1) % candidates.len();
                true
            }
            KeyCode::Tab if !ctrl && !alt && !shift => {
                match &candidates[idx] {
                    WorktreeAutocompleteItem::CreateNew => {
                        self.input_buffer = "/worktree".to_string();
                    }
                    WorktreeAutocompleteItem::Existing(entry) => {
                        self.input_buffer = format!("/worktree {}", entry.path);
                    }
                }
                self.cursor_position = self.input_buffer.len();
                self.worktree_autocomplete_open = false;
                self.worktree_autocomplete_idx = 0;
                true
            }
            KeyCode::Enter if !ctrl && !alt && !shift => {
                let selected = candidates[idx].clone();
                self.input_buffer.clear();
                self.cursor_position = 0;
                self.worktree_autocomplete_open = false;
                self.worktree_autocomplete_idx = 0;
                let active_id = self.active_thread_id.clone();
                match selected {
                    WorktreeAutocompleteItem::CreateNew => {
                        self.status_notice = Some("Allocating worktree...".to_string());
                        self.pending_actions
                            .push_back(AppAction::EnsureThreadWorktree {
                                thread_id: active_id,
                            });
                    }
                    WorktreeAutocompleteItem::Existing(entry) => {
                        self.status_notice = Some("Switching worktree...".to_string());
                        self.pending_actions
                            .push_back(AppAction::SwitchThreadWorktree {
                                thread_id: active_id,
                                target_path: entry.path,
                            });
                    }
                }
                true
            }
            KeyCode::Esc if !ctrl && !alt => {
                self.worktree_autocomplete_open = false;
                true
            }
            _ => false,
        }
    }

    fn handle_key_worktrees_tree(&mut self, key: KeyEvent, had_g_prefix: bool) {
        let no_ctrl_alt = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        if !no_ctrl_alt {
            return;
        }

        let rows = self.visible_worktree_rows();
        let selected_row = rows.get(self.worktree_selected_idx).cloned();

        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                if !rows.is_empty() && self.worktree_selected_idx + 1 < rows.len() {
                    self.worktree_selected_idx += 1;
                    self.maybe_fetch_selected_worktree_diff(false);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.worktree_selected_idx = self.worktree_selected_idx.saturating_sub(1);
                self.maybe_fetch_selected_worktree_diff(false);
            }
            KeyCode::Char('g') => {
                if had_g_prefix {
                    self.worktree_selected_idx = 0;
                    self.pending_g_prefix = false;
                    self.maybe_fetch_selected_worktree_diff(false);
                } else {
                    self.pending_g_prefix = true;
                }
            }
            KeyCode::Char('G') => {
                if !rows.is_empty() {
                    self.worktree_selected_idx = rows.len() - 1;
                    self.maybe_fetch_selected_worktree_diff(false);
                }
            }
            KeyCode::Char(' ') => {
                match selected_row {
                    Some(WorktreeRow::RepoHeader { repo_root, .. }) => {
                        self.toggle_worktree_repo_fold(&repo_root);
                    }
                    Some(WorktreeRow::Entry(entry)) => {
                        self.toggle_worktree_repo_fold(&entry.repo_root);
                    }
                    None => {}
                }
                self.maybe_fetch_selected_worktree_diff(false);
            }
            KeyCode::Left | KeyCode::Char('h') => {
                match selected_row {
                    Some(WorktreeRow::RepoHeader {
                        repo_root,
                        collapsed,
                        ..
                    }) => {
                        if !collapsed {
                            self.collapsed_worktree_repos.insert(repo_root);
                            self.clamp_worktree_selection();
                        }
                    }
                    Some(WorktreeRow::Entry(entry)) => {
                        if let Some(hdr_idx) = rows.iter().position(|r| {
                            matches!(r, WorktreeRow::RepoHeader { repo_root, .. } if *repo_root == entry.repo_root)
                        }) {
                            self.worktree_selected_idx = hdr_idx;
                        }
                    }
                    None => {}
                }
                self.maybe_fetch_selected_worktree_diff(false);
            }
            KeyCode::Right | KeyCode::Char('l') => {
                match selected_row {
                    Some(WorktreeRow::RepoHeader {
                        repo_root,
                        collapsed,
                        ..
                    }) => {
                        if collapsed {
                            self.collapsed_worktree_repos.remove(&repo_root);
                            self.clamp_worktree_selection();
                        } else if self.worktree_selected_idx + 1 < rows.len()
                            && matches!(
                                rows.get(self.worktree_selected_idx + 1),
                                Some(WorktreeRow::Entry(_))
                            )
                        {
                            self.worktree_selected_idx += 1;
                        }
                    }
                    Some(WorktreeRow::Entry(entry)) => {
                        let active_id = self.active_thread_id.clone();
                        self.status_notice = Some("Switching worktree...".to_string());
                        self.pending_actions
                            .push_back(AppAction::SwitchThreadWorktree {
                                thread_id: active_id,
                                target_path: entry.path,
                            });
                        self.focused_pane = if self.active_is_terminal() {
                            FocusedPane::Terminal
                        } else {
                            FocusedPane::Composer
                        };
                    }
                    None => {}
                }
                self.maybe_fetch_selected_worktree_diff(false);
            }
            KeyCode::Char('r') => {
                self.status_notice = Some("Refreshing worktrees...".to_string());
                self.worktree_list_in_flight = true;
                self.pending_actions.push_back(AppAction::ListWorktrees);
                self.maybe_fetch_selected_worktree_diff(true);
            }
            KeyCode::Char('p') => {
                self.request_prune_worktrees();
            }
            KeyCode::Char('y') => {
                if let Some(WorktreeRow::Entry(entry)) = selected_row {
                    self.finish_copy_to_clipboard("worktree path", entry.path);
                }
            }
            KeyCode::Enter => match selected_row {
                Some(WorktreeRow::RepoHeader { repo_root, .. }) => {
                    self.toggle_worktree_repo_fold(&repo_root);
                    self.maybe_fetch_selected_worktree_diff(false);
                }
                Some(WorktreeRow::Entry(entry)) => {
                    let active_id = self.active_thread_id.clone();
                    self.status_notice = Some("Switching worktree...".to_string());
                    self.pending_actions
                        .push_back(AppAction::SwitchThreadWorktree {
                            thread_id: active_id,
                            target_path: entry.path,
                        });
                    self.focused_pane = if self.active_is_terminal() {
                        FocusedPane::Terminal
                    } else {
                        FocusedPane::Composer
                    };
                }
                None => {}
            },
            KeyCode::Char('t') => {
                if let Some(WorktreeRow::Entry(entry)) = selected_row {
                    let (rows, cols) = self.current_terminal_dims_or_default();
                    let label = worktree_display_label(&entry);
                    self.status_notice = None;
                    self.pending_actions.push_back(AppAction::CreateTerminal {
                        parent_thread_id: Some(self.active_thread_id.clone()),
                        title: Some(format!("Shell ({label})")),
                        cwd: Some(entry.path),
                        rows,
                        cols,
                    });
                }
            }
            KeyCode::Char('w' | 'a' | 'n') => {
                let thread_id = self.active_thread_id.clone();
                self.status_notice = Some("Allocating worktree...".to_string());
                self.pending_actions
                    .push_back(AppAction::EnsureThreadWorktree { thread_id });
            }
            KeyCode::Char('d' | 'x') => {
                if let Some(WorktreeRow::Entry(entry)) = selected_row {
                    if entry.is_primary {
                        self.status_notice =
                            Some("Cannot delete primary repository worktree".to_string());
                    } else {
                        let label = worktree_display_label(&entry);
                        self.pending_worktree_delete = Some(PendingWorktreeDelete {
                            path: entry.path,
                            label,
                            dirty_count: entry.dirty_count,
                            attached_count: entry.attached_thread_ids.len(),
                            force: entry.dirty_count > 0,
                        });
                    }
                }
            }
            KeyCode::Char('z') => {
                self.toggle_sidebar_zoom();
            }
            _ => {}
        }
    }

    pub fn handle_key_tree(&mut self, key: KeyEvent) -> Option<TreeAction> {
        self.handle_key_tree_with_prefix(key, false)
    }

    fn handle_key_tree_with_prefix(
        &mut self,
        key: KeyEvent,
        had_g_prefix: bool,
    ) -> Option<TreeAction> {
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
            KeyCode::Char('g') if no_ctrl_alt => {
                if had_g_prefix {
                    if !self.tree.nodes.is_empty() {
                        self.tree.select_index(0);
                        let (sel_id, _) = self.selected_tree_thread_info();
                        self.maybe_request_pty_snapshot(&sel_id);
                    }
                    self.pending_g_prefix = false;
                } else {
                    self.pending_g_prefix = true;
                }
                None
            }
            KeyCode::Char('G') if no_ctrl_alt => {
                if !self.tree.nodes.is_empty() {
                    self.tree.select_index(self.tree.nodes.len() - 1);
                    let (sel_id, _) = self.selected_tree_thread_info();
                    self.maybe_request_pty_snapshot(&sel_id);
                }
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
            KeyCode::Char('o') if no_ctrl_alt => {
                self.toggle_transcript_cards();
                None
            }
            KeyCode::Char('y') if no_ctrl_alt => {
                self.copy_transcript_content(CopyMode::LastReply);
                None
            }
            KeyCode::Char('a') if no_ctrl_alt => {
                self.status_notice = None;
                self.pending_actions
                    .push_back(AppAction::CreateRoot { title: None });
                None
            }
            KeyCode::Char('w') if no_ctrl_alt => {
                let (sel_id, _) = self.selected_tree_thread_info();
                let thread_id = if sel_id.is_empty() {
                    self.active_thread_id.clone()
                } else {
                    sel_id
                };
                self.status_notice = Some("Allocating worktree...".to_string());
                self.pending_actions
                    .push_back(AppAction::EnsureThreadWorktree { thread_id });
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
                self.begin_inline_rename();
                None
            }
            KeyCode::Char('m') if no_ctrl_alt => {
                let (source_thread_id, _) = self.selected_tree_thread_info();
                self.request_join_thread(source_thread_id, None);
                None
            }
            KeyCode::Char('d' | 'x') if no_ctrl_alt => {
                if self.threads.len() <= 1 {
                    self.status_notice = Some("Cannot delete the only context".to_string());
                } else {
                    let (thread_id, title) = self.selected_tree_thread_info();
                    self.pending_thread_delete = Some(PendingThreadDelete { thread_id, title });
                }
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
        let prev_buf = self.input_buffer.clone();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let pos = clamp_char_boundary(&self.input_buffer, self.cursor_position);

        let handled = match (key.code, ctrl, alt) {
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
            (KeyCode::Char('f' | 'F'), true, false) => {
                if let Some(ch) = self.input_buffer[pos..].chars().next() {
                    self.cursor_position = pos + ch.len_utf8();
                } else {
                    self.cursor_position = self.input_buffer.len();
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
            (KeyCode::Char(c), _, _) if is_altgr_char(c, ctrl, alt) => {
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
        };
        if handled && self.input_buffer != prev_buf {
            if !matches!(key.code, KeyCode::Enter) {
                self.status_notice = None;
            }
            self.sync_worktree_autocomplete(true);
        }
        handled
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) -> bool {
        if key.kind == KeyEventKind::Release {
            return false;
        }
        self.sync_focus_in();

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let no_ctrl_alt = !ctrl && !alt;

        // 1. Modal priority first (before EnterManageMode & ToggleZoom)
        if self.pending_permission.is_some() {
            self.pending_g_prefix = false;
            match key.code {
                KeyCode::Char('y' | 'Y') if no_ctrl_alt => {
                    self.resolve_active_permission(true);
                }
                KeyCode::Char('n' | 'N') | KeyCode::Esc if no_ctrl_alt => {
                    self.resolve_active_permission(false);
                }
                KeyCode::Char('c' | 'C') if ctrl && !alt => {
                    self.resolve_active_permission(false);
                }
                _ => {}
            }
            self.sync_focus_out();
            return true;
        }

        if let Some(pending) = self.pending_worktree_delete.clone() {
            self.pending_g_prefix = false;
            match key.code {
                KeyCode::Char('y' | 'Y') if no_ctrl_alt => {
                    self.pending_worktree_delete = None;
                    self.status_notice = Some(format!("Deleting worktree {}...", pending.label));
                    self.pending_actions.push_back(AppAction::RemoveWorktree {
                        path: pending.path,
                        force: pending.force,
                    });
                }
                KeyCode::Char('n' | 'N') | KeyCode::Esc if no_ctrl_alt => {
                    self.pending_worktree_delete = None;
                    self.status_notice = Some("Delete worktree cancelled".to_string());
                }
                KeyCode::Char('c' | 'C') if ctrl && !alt => {
                    self.pending_worktree_delete = None;
                    self.status_notice = Some("Delete worktree cancelled".to_string());
                }
                _ => {}
            }
            self.sync_focus_out();
            return true;
        }

        if let Some(pending) = self.pending_thread_delete.clone() {
            self.pending_g_prefix = false;
            match key.code {
                KeyCode::Char('y' | 'Y') if no_ctrl_alt => {
                    self.pending_thread_delete = None;
                    self.request_delete_thread(pending.thread_id);
                }
                KeyCode::Char('n' | 'N') | KeyCode::Esc if no_ctrl_alt => {
                    self.pending_thread_delete = None;
                    self.status_notice = Some("Delete cancelled".to_string());
                }
                KeyCode::Char('c' | 'C') if ctrl && !alt => {
                    self.pending_thread_delete = None;
                    self.status_notice = Some("Delete cancelled".to_string());
                }
                _ => {}
            }
            self.sync_focus_out();
            return true;
        }

        if self.pending_quit_confirm.is_some() {
            self.pending_g_prefix = false;
            match key.code {
                KeyCode::Char('y' | 'Y') if no_ctrl_alt => {
                    self.pending_quit_confirm = None;
                    self.should_quit = true;
                }
                KeyCode::Char('n' | 'N') | KeyCode::Esc if no_ctrl_alt => {
                    self.pending_quit_confirm = None;
                    self.status_notice = Some("Quit cancelled".to_string());
                }
                KeyCode::Char('c' | 'C') if ctrl && !alt => {
                    self.pending_quit_confirm = None;
                    self.status_notice = Some("Quit cancelled".to_string());
                }
                _ => {}
            }
            self.sync_focus_out();
            return true;
        }

        if self.theme_picker.is_some() {
            self.pending_g_prefix = false;
            self.handle_key_theme_picker(key);
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
            let clamp_help = |app: &App, val: u16| -> u16 {
                if app.last_frame_area.is_some() {
                    val.min(app.help_max_scroll)
                } else {
                    val
                }
            };
            match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::F(1) if no_ctrl_alt => {
                    self.show_help_modal = false;
                    self.help_scroll_offset = 0;
                }
                KeyCode::Char('q' | 'Q' | '?') if no_ctrl_alt => {
                    self.show_help_modal = false;
                    self.help_scroll_offset = 0;
                }
                KeyCode::Char('c' | 'C') if ctrl && !alt => {
                    self.show_help_modal = false;
                    self.help_scroll_offset = 0;
                }
                KeyCode::Down | KeyCode::Char('j' | 'J') if no_ctrl_alt => {
                    self.help_scroll_offset =
                        clamp_help(self, self.help_scroll_offset.saturating_add(1));
                }
                KeyCode::Up | KeyCode::Char('k' | 'K') if no_ctrl_alt => {
                    self.help_scroll_offset = self.help_scroll_offset.saturating_sub(1);
                }
                KeyCode::PageDown if no_ctrl_alt => {
                    self.help_scroll_offset =
                        clamp_help(self, self.help_scroll_offset.saturating_add(10));
                }
                KeyCode::Char('d' | 'D') if ctrl && !alt => {
                    self.help_scroll_offset =
                        clamp_help(self, self.help_scroll_offset.saturating_add(10));
                }
                KeyCode::PageUp if no_ctrl_alt => {
                    self.help_scroll_offset = self.help_scroll_offset.saturating_sub(10);
                }
                KeyCode::Char('u' | 'U') if ctrl && !alt => {
                    self.help_scroll_offset = self.help_scroll_offset.saturating_sub(10);
                }
                KeyCode::Home | KeyCode::Char('g') if no_ctrl_alt => {
                    self.help_scroll_offset = 0;
                }
                KeyCode::End | KeyCode::Char('G') if no_ctrl_alt => {
                    self.help_scroll_offset = self.help_max_scroll;
                }
                _ => {}
            }
            self.sync_focus_out();
            return true;
        }

        // 2. WRITE Mode (Terminal & Composer unified minimal intercepts)
        if matches!(
            self.focused_pane,
            FocusedPane::Terminal | FocusedPane::Composer
        ) {
            self.pending_g_prefix = false;
            if let Some(action) = self.keymap.match_write_key(key) {
                self.execute_bindable_action(action);
                self.sync_focus_out();
                return true;
            }

            if self.focused_pane == FocusedPane::Terminal {
                if self.is_terminal_exited(&self.active_thread_id) {
                    if key.code == KeyCode::Esc && no_ctrl_alt {
                        self.focus_tree();
                        return true;
                    }
                    if key.code == KeyCode::Enter && no_ctrl_alt && !shift {
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
                    return true;
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

            // FocusedPane::Composer
            if self.worktree_autocomplete_open && self.handle_key_worktree_autocomplete(key) {
                self.sync_focus_out();
                return true;
            }
            if ctrl && !alt && !shift && matches!(key.code, KeyCode::Char('c' | 'C')) {
                if self.active_thread_is_busy() {
                    self.status_notice = Some("Cancelling active turn...".to_string());
                    self.pending_actions.push_back(AppAction::CancelTurn {
                        thread_id: self.active_thread_id.clone(),
                    });
                } else if !self.input_buffer.is_empty() {
                    self.input_buffer.clear();
                    self.cursor_position = 0;
                    self.worktree_autocomplete_open = false;
                    self.worktree_autocomplete_idx = 0;
                    self.status_notice = Some("Cleared input".to_string());
                } else {
                    self.status_notice =
                        Some("Use Ctrl+G then q (or /quit) to exit".to_string());
                }
                self.sync_focus_out();
                return true;
            }
            if no_ctrl_alt && key.code == KeyCode::Esc {
                if self.worktree_autocomplete_open {
                    self.worktree_autocomplete_open = false;
                } else if self.active_thread_is_busy() {
                    self.status_notice = Some("Cancelling active turn...".to_string());
                    self.pending_actions.push_back(AppAction::CancelTurn {
                        thread_id: self.active_thread_id.clone(),
                    });
                }
                self.sync_focus_out();
                return true;
            }
            let handled = self.handle_key_composer(key);
            self.sync_focus_out();
            return handled;
        }

        let had_g_prefix = std::mem::take(&mut self.pending_g_prefix);

        // 3. MANAGE Mode (FocusedPane::Tree)
        if self.focused_pane == FocusedPane::Tree {
            if alt && !ctrl && matches!(key.code, KeyCode::Char('1'..='9')) {
                self.sync_focus_out();
                return false;
            }
            if self.rename_prompt.is_some() {
                self.handle_key_rename_prompt(key);
                self.sync_focus_out();
                return true;
            }
            if self.palette_active {
                self.handle_key_palette(key);
                self.sync_focus_out();
                return true;
            }
            if self.sidebar_view == SidebarView::Worktrees && self.worktree_filter_active {
                match (key.code, ctrl, alt) {
                    (KeyCode::Esc, false, false) => {
                        self.worktree_filter_active = false;
                        self.worktree_filter_query.clear();
                        self.clamp_worktree_selection();
                        self.maybe_fetch_selected_worktree_diff(false);
                    }
                    (KeyCode::Char('c' | 'C'), true, false) => {
                        self.worktree_filter_active = false;
                        self.worktree_filter_query.clear();
                        self.clamp_worktree_selection();
                        self.maybe_fetch_selected_worktree_diff(false);
                    }
                    (KeyCode::Enter | KeyCode::Down | KeyCode::Up, false, false) => {
                        self.worktree_filter_active = false;
                        self.clamp_worktree_selection();
                        self.maybe_fetch_selected_worktree_diff(false);
                    }
                    (KeyCode::Backspace, false, false) => {
                        self.worktree_filter_query.pop();
                        self.clamp_worktree_selection();
                        self.maybe_fetch_selected_worktree_diff(false);
                    }
                    (KeyCode::Char(c), _, _) if is_altgr_char(c, ctrl, alt) => {
                        self.worktree_filter_query.push(c);
                        self.clamp_worktree_selection();
                        self.maybe_fetch_selected_worktree_diff(false);
                    }
                    _ => {}
                }
                self.sync_focus_out();
                return true;
            }
            if self.transcript_search.editing {
                match (key.code, ctrl, alt) {
                    (KeyCode::Esc, false, false) | (KeyCode::Char('c' | 'C'), true, false) => {
                        self.transcript_search.clear();
                    }
                    (KeyCode::Enter, false, false) => {
                        self.transcript_search.confirm();
                        if !self.transcript_search.query.is_empty() {
                            self.recompute_transcript_search_and_scroll();
                        }
                    }
                    (KeyCode::Backspace, false, false) => {
                        self.transcript_search.query.pop();
                        self.recompute_transcript_search_and_scroll();
                    }
                    (KeyCode::Char(c), _, _) if is_altgr_char(c, ctrl, alt) => {
                        self.transcript_search.query.push(c);
                        self.recompute_transcript_search_and_scroll();
                    }
                    _ => {}
                }
                self.sync_focus_out();
                return true;
            }

            // Tab / Ctrl+T switches between Grein and Worktrees sidebar views
            if (no_ctrl_alt && matches!(key.code, KeyCode::Tab | KeyCode::BackTab))
                || (ctrl && !alt && matches!(key.code, KeyCode::Char('t' | 'T')))
            {
                self.toggle_worktrees_sidebar_view();
                return true;
            }

            // Help via ?, F1, or legacy g?
            if (no_ctrl_alt && matches!(key.code, KeyCode::F(1) | KeyCode::Char('?')))
                || (had_g_prefix && no_ctrl_alt && key.code == KeyCode::Char('?'))
            {
                self.help_scroll_offset = 0;
                self.show_help_modal = true;
                self.sync_focus_out();
                return true;
            }

            // Sidebar Command Palette (:)
            if no_ctrl_alt && key.code == KeyCode::Char(':') {
                self.open_command_palette("");
                self.sync_focus_out();
                return true;
            }

            // Quit (with confirm if busy agents or live PTYs)
            if (no_ctrl_alt && !shift && key.code == KeyCode::Char('q'))
                || (ctrl && !alt && matches!(key.code, KeyCode::Char('c' | 'C')))
            {
                self.request_quit_with_confirm();
                self.sync_focus_out();
                return true;
            }

            // Check WRITE keymap bindings in MANAGE too (Ctrl+G/Alt+G/F6 exits MANAGE -> WRITE;
            // Alt+Z toggles zoom; custom global keybinds work symmetrically; ScrollUp/Down scroll preview)
            if let Some(action) = self.keymap.match_write_key(key) {
                self.execute_bindable_action(action);
                self.sync_focus_out();
                return true;
            }

            // Right-pane preview scrolling (PgUp/PgDn/Ctrl+U/Ctrl+D)
            match (key.code, ctrl, alt) {
                (KeyCode::PageUp, false, false) | (KeyCode::Char('u' | 'U'), true, false) => {
                    let step = self.half_page_scroll_lines();
                    if self.sidebar_view == SidebarView::Worktrees {
                        self.worktree_diff_scroll =
                            self.worktree_diff_scroll.saturating_sub(step as u16);
                    } else {
                        let target_id = self.selected_tree_thread_info().0;
                        if self.is_terminal_thread(&target_id) {
                            self.pending_actions.push_back(AppAction::PtyScroll {
                                thread_id: target_id,
                                delta: 5,
                            });
                        } else {
                            self.scroll_target_transcript(TranscriptScroll::Up(step));
                        }
                    }
                    self.sync_focus_out();
                    return true;
                }
                (KeyCode::PageDown, false, false) | (KeyCode::Char('d' | 'D'), true, false) => {
                    let step = self.half_page_scroll_lines();
                    if self.sidebar_view == SidebarView::Worktrees {
                        self.worktree_diff_scroll = self
                            .worktree_diff_scroll
                            .saturating_add(step as u16)
                            .min(self.worktree_diff_max_scroll);
                    } else {
                        let target_id = self.selected_tree_thread_info().0;
                        if self.is_terminal_thread(&target_id) {
                            self.pending_actions.push_back(AppAction::PtyScroll {
                                thread_id: target_id,
                                delta: -5,
                            });
                        } else {
                            self.scroll_target_transcript(TranscriptScroll::Down(step));
                        }
                    }
                    self.sync_focus_out();
                    return true;
                }
                _ => {}
            }

            // Root switching in MANAGE (Alt+J/K or Alt+Shift+Up/Down)
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

            // View-specific MANAGE controls
            if self.sidebar_view == SidebarView::Worktrees {
                if no_ctrl_alt && key.code == KeyCode::Char('/') {
                    self.worktree_filter_active = true;
                    self.sync_focus_out();
                    return true;
                }
                if no_ctrl_alt && key.code == KeyCode::Esc {
                    if !self.worktree_filter_query.is_empty() {
                        self.worktree_filter_query.clear();
                        self.clamp_worktree_selection();
                        self.maybe_fetch_selected_worktree_diff(false);
                    } else {
                        self.toggle_composer_tree_focus();
                    }
                    self.sync_focus_out();
                    return true;
                }
                self.handle_key_worktrees_tree(key, had_g_prefix);
                self.sync_focus_out();
                return true;
            }

            // Grein view: filter/search & Esc & Grein actions
            if no_ctrl_alt {
                match key.code {
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
                    KeyCode::Esc => {
                        if !self.transcript_search.query.is_empty() {
                            self.transcript_search.clear();
                        } else {
                            let _ = self.activate_selected_tree_node();
                        }
                        self.sync_focus_out();
                        return true;
                    }
                    _ => {}
                }
            }
            if alt && !ctrl && matches!(key.code, KeyCode::Char('t' | 'T')) {
                self.open_or_jump_to_terminal();
                self.sync_focus_out();
                return true;
            }
            let _ = self.handle_key_tree_with_prefix(key, had_g_prefix);
            self.sync_focus_out();
            return true;
        }

        // 4. Legacy Transcript pane fallback (if unit tests explicitly focus Transcript)
        if alt && !ctrl && matches!(key.code, KeyCode::Char('1'..='9')) {
            self.sync_focus_out();
            return false;
        }
        if self.transcript_search.editing {
            match (key.code, ctrl, alt) {
                (KeyCode::Esc, false, false) | (KeyCode::Char('c' | 'C'), true, false) => {
                    self.transcript_search.clear();
                }
                (KeyCode::Enter, false, false) => {
                    self.transcript_search.confirm();
                    if !self.transcript_search.query.is_empty() {
                        self.recompute_transcript_search_and_scroll();
                    }
                }
                (KeyCode::Backspace, false, false) => {
                    self.transcript_search.query.pop();
                    self.recompute_transcript_search_and_scroll();
                }
                (KeyCode::Char(c), _, _) if is_altgr_char(c, ctrl, alt) => {
                    self.transcript_search.query.push(c);
                    self.recompute_transcript_search_and_scroll();
                }
                _ => {}
            }
            self.sync_focus_out();
            return true;
        }
        if let Some(action) = self.keymap.match_write_key(key) {
            self.execute_bindable_action(action);
            self.sync_focus_out();
            return true;
        }
        if no_ctrl_alt && matches!(key.code, KeyCode::F(1) | KeyCode::Char('?')) {
            self.help_scroll_offset = 0;
            self.show_help_modal = true;
            self.sync_focus_out();
            return true;
        }
        match (key.code, ctrl, alt) {
            (KeyCode::Esc, false, false) => {
                if !self.transcript_search.query.is_empty() {
                    self.transcript_search.clear();
                } else {
                    self.focused_pane = FocusedPane::Composer;
                }
                self.sync_focus_out();
                return true;
            }
            (KeyCode::Enter, false, false) => {
                self.focused_pane = FocusedPane::Composer;
                self.sync_focus_out();
                return true;
            }
            (KeyCode::Char('q'), false, false) | (KeyCode::Char('c' | 'C'), true, false) => {
                self.request_quit_with_confirm();
                self.sync_focus_out();
                return true;
            }
            (KeyCode::Char('/'), false, false) => {
                self.open_transcript_search(crate::search::SearchDirection::Forward);
                self.sync_focus_out();
                return true;
            }
            (KeyCode::Char('n'), false, false) if !self.transcript_search.query.is_empty() => {
                self.step_transcript_search(true);
                self.sync_focus_out();
                return true;
            }
            (KeyCode::Char('N'), false, false) if !self.transcript_search.query.is_empty() => {
                self.step_transcript_search(false);
                self.sync_focus_out();
                return true;
            }
            (KeyCode::Char('o'), false, false) => {
                self.toggle_transcript_cards();
                self.sync_focus_out();
                return true;
            }
            (KeyCode::Up | KeyCode::Char('k'), false, false) => {
                self.scroll_target_transcript(TranscriptScroll::Up(1));
                return true;
            }
            (KeyCode::Down | KeyCode::Char('j'), false, false) => {
                self.scroll_target_transcript(TranscriptScroll::Down(1));
                return true;
            }
            (KeyCode::PageUp, false, false) | (KeyCode::Char('u' | 'U'), true, false) => {
                let step = self.half_page_scroll_lines();
                self.scroll_target_transcript(TranscriptScroll::Up(step));
                return true;
            }
            (KeyCode::PageDown, false, false) | (KeyCode::Char('d' | 'D'), true, false) => {
                let step = self.half_page_scroll_lines();
                self.scroll_target_transcript(TranscriptScroll::Down(step));
                return true;
            }
            (KeyCode::Home, false, false) => {
                self.scroll_target_transcript(TranscriptScroll::Top);
                return true;
            }
            (KeyCode::End | KeyCode::Char('G'), false, false) => {
                self.scroll_target_transcript(TranscriptScroll::Bottom);
                return true;
            }
            (KeyCode::Char('g'), false, false) => {
                if had_g_prefix {
                    self.scroll_target_transcript(TranscriptScroll::Top);
                } else {
                    self.pending_g_prefix = true;
                }
                return true;
            }
            _ => {}
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

        // Idle Esc in Composer stays in WRITE mode; Ctrl+G / Alt+G / F6 enter MANAGE,
        // and Esc in MANAGE returns to WRITE mode.
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Composer);

        app.handle_key_event(KeyEvent::new(KeyCode::F(6), KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Tree);
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.focused_pane(), FocusedPane::Composer);

        app.thread_statuses
            .insert(app.active_thread_id.clone(), "streaming".to_string());
        app.handle_key_event(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
        assert_eq!(app.focused_pane(), FocusedPane::Tree);
        assert!(app.pending_actions.is_empty());

        app.handle_key_event(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::ALT));
        assert_eq!(app.focused_pane(), FocusedPane::Composer);
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(
            app.pending_actions.pop_front(),
            Some(AppAction::CancelTurn {
                thread_id: app.active_thread_id.clone(),
            })
        );
    }

    #[test]
    fn test_tab_switches_manage_views_and_cycle_branch_within_active_root() {
        let mut app = multi_root_app();
        assert_eq!(app.active_thread_id(), "r1");

        // In WRITE (Composer), Tab does not hijack branches across threads
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.active_thread_id(), "r1");
        assert_eq!(app.focused_pane(), FocusedPane::Composer);

        // cycle_branch_in_active_root still traverses [r1, b1, g1, b2] within r1 subtree
        app.cycle_branch_in_active_root(true);
        assert_eq!(app.active_thread_id(), "b1");
        app.cycle_branch_in_active_root(true);
        assert_eq!(app.active_thread_id(), "g1");
        app.cycle_branch_in_active_root(true);
        assert_eq!(app.active_thread_id(), "b2");
        app.cycle_branch_in_active_root(true);
        assert_eq!(app.active_thread_id(), "r1");

        app.cycle_branch_in_active_root(false);
        assert_eq!(app.active_thread_id(), "b2");

        // Lone root with no children stays a no-op
        app.switch_thread("r3");
        app.cycle_branch_in_active_root(true);
        assert_eq!(app.active_thread_id(), "r3");

        // In MANAGE (Tree), Tab toggles Grein <-> Worktrees
        app.set_focus(FocusedPane::Tree);
        assert_eq!(app.sidebar_view, SidebarView::Grein);
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.sidebar_view, SidebarView::Worktrees);
        assert_eq!(app.focused_pane(), FocusedPane::Tree);
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.sidebar_view, SidebarView::Grein);
        assert_eq!(app.focused_pane(), FocusedPane::Tree);
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

        // From Composer (WRITE): Alt/Shift+PgUp/PgDn scroll active thread r1; plain Home/End move cursor
        app.set_focus(FocusedPane::Composer);
        app.input_buffer = "draft".to_string();
        app.cursor_position = 2;
        assert!(app.handle_key_event(KeyEvent::new(KeyCode::PageUp, KeyModifiers::ALT)));
        assert_eq!(app.transcript_viewport("r1").scroll_offset(), 90);
        assert!(app.handle_key_event(KeyEvent::new(KeyCode::PageDown, KeyModifiers::SHIFT)));
        assert_eq!(app.transcript_viewport("r1").scroll_offset(), 100);
        assert!(app.handle_key_event(KeyEvent::new(KeyCode::PageUp, KeyModifiers::SHIFT)));
        assert_eq!(app.transcript_viewport("r1").scroll_offset(), 90);
        assert!(app.handle_key_event(KeyEvent::new(KeyCode::PageDown, KeyModifiers::ALT)));
        assert_eq!(app.transcript_viewport("r1").scroll_offset(), 100);

        assert!(app.handle_key_event(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE)));
        assert_eq!(app.cursor_position, 0);
        assert!(app.handle_key_event(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)));
        assert_eq!(app.cursor_position, 5);
        assert_eq!(app.transcript_viewport("r1").scroll_offset(), 100);

        // From Tree previewing b1 (MANAGE): PgUp/Ctrl+U scroll b1 without disturbing r1
        app.set_focus(FocusedPane::Tree);
        app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.active_thread_id(), "r1");
        assert!(app.handle_key_event(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE)));
        assert_eq!(app.transcript_viewport("b1").scroll_offset(), 70);
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
    fn test_ctrl_q_and_ctrl_c_safe_in_composer_and_q_quits_in_manage() {
        let mut app1 = App::new();
        app1.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
        assert!(!app1.should_quit());

        app1.input_buffer = "unsent draft".to_string();
        app1.cursor_position = app1.input_buffer.len();
        app1.handle_key_event(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(!app1.should_quit());
        assert_eq!(app1.composer_content(), "");
        assert_eq!(app1.status_notice.as_deref(), Some("Cleared input"));

        app1.handle_key_event(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(!app1.should_quit());
        assert_eq!(
            app1.status_notice.as_deref(),
            Some("Use Ctrl+G then q (or /quit) to exit")
        );

        app1.handle_key_event(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
        assert_eq!(app1.focused_pane(), FocusedPane::Tree);
        app1.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(app1.should_quit());
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
    fn test_tree_fork_and_enter_switches_thread_to_composer() {
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

        // Ctrl+N no longer hijacks Composer in WRITE mode
        assert!(!app.handle_key_event(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL)));
        assert!(app.pending_actions.is_empty());

        app.set_focus(FocusedPane::Tree);
        app.handle_key_event(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        assert_eq!(
            app.pending_actions.pop_front(),
            Some(AppAction::ForkBranch {
                source_thread_id: "t-1".to_string(),
                title: None,
            })
        );

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
