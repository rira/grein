use crossterm::event::KeyEvent;
use ratatui::text::Line;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use thiserror::Error;

use crate::clipboard::{ChainedClipboardDriver, ClipboardPort};
use crate::engine_port::{
    AgentConfigChoice, AgentConfigOption, AgentSlashCommand, PtyScreenSnapshot, SubagentSnapshot,
    ThreadMeta, WorktreeEntry,
};
use crate::keymap::Keymap;
use crate::markdown::LineMeta;
use crate::pager::{DefaultPagerRunner, PagerRunner};
use crate::selection::{PaneRects, PaneSelection};
use crate::theme::{ThemePalette, ThemePickerModal, ThemePreset};
use crate::transcript::{ThreadTranscript, TranscriptViewport};

use super::threads::is_generic_agent_placeholder;

#[derive(Clone)]
pub struct TuiServices {
    pub clipboard: Arc<dyn ClipboardPort>,
    pub pager: Arc<dyn PagerRunner>,
}

impl TuiServices {
    pub fn new(clipboard: Arc<dyn ClipboardPort>, pager: Arc<dyn PagerRunner>) -> Self {
        Self { clipboard, pager }
    }

    pub fn with_clipboard(mut self, clipboard: Arc<dyn ClipboardPort>) -> Self {
        self.clipboard = clipboard;
        self
    }

    pub fn with_pager(mut self, pager: Arc<dyn PagerRunner>) -> Self {
        self.pager = pager;
        self
    }
}

impl Default for TuiServices {
    fn default() -> Self {
        Self {
            clipboard: Arc::new(ChainedClipboardDriver::default()),
            pager: Arc::new(DefaultPagerRunner::new()),
        }
    }
}

impl std::fmt::Debug for TuiServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TuiServices")
            .field("clipboard", &"<dyn ClipboardPort>")
            .field("pager", &"<dyn PagerRunner>")
            .finish()
    }
}

impl PartialEq for TuiServices {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Eq for TuiServices {}

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
        cwd: Option<String>,
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
    FetchAutocompleteContext {
        cwd: Option<String>,
    },
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
    pub custom_name: Option<String>,
    pub anchors: Vec<AnchorItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadDisplayParts {
    pub custom_name: Option<String>,
    pub auto_label: String,
}

impl ThreadDisplayParts {
    pub fn combined(&self, is_terminal: bool) -> String {
        match self.custom_name.as_deref() {
            Some(name) if is_terminal || !is_generic_agent_placeholder(&self.auto_label) => {
                format!("[{name}] {}", self.auto_label)
            }
            Some(name) => format!("[{name}]"),
            None => self.auto_label.clone(),
        }
    }
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
    pub services: TuiServices,
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
    pub help_filter_active: bool,
    pub help_filter_query: String,
    pub config_picker: Option<ConfigPickerModal>,
    pub sidebar_hidden: bool,
    pub composer_scroll_y: u16,
    pub last_composer_inner_width: u16,
    pub composer_desired_col: Option<usize>,
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
    pub last_visited_child: HashMap<String, String>,
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
    pub composer_autocomplete_enabled: bool,
    pub autocomplete_workspace_files: Vec<String>,
    pub autocomplete_recent_prompts: Vec<String>,
    pub composer_completion: Option<crate::autocomplete::ActiveCompletion>,
    pub composer_completion_dismissed: bool,
    pub pane_rects: PaneRects,
    pub last_transcript_scroll_y: usize,
    pub last_rendered_lines: Vec<Line<'static>>,
    pub last_rendered_meta: Vec<LineMeta>,
    pub pane_selection: Option<PaneSelection>,
    pub last_click_info: Option<(u64, u16, u16, u8)>,
    pub frozen_pty_snapshot: Option<(String, PtyScreenSnapshot)>,
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
            services: TuiServices::default(),
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
            help_filter_active: false,
            help_filter_query: String::new(),
            config_picker: None,
            sidebar_hidden: false,
            composer_scroll_y: 0,
            last_composer_inner_width: 0,
            composer_desired_col: None,
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
            last_visited_child: HashMap::new(),
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
            composer_autocomplete_enabled: true,
            autocomplete_workspace_files: Vec::new(),
            autocomplete_recent_prompts: Vec::new(),
            composer_completion: None,
            composer_completion_dismissed: false,
            pane_rects: PaneRects::default(),
            last_transcript_scroll_y: 0,
            last_rendered_lines: Vec::new(),
            last_rendered_meta: Vec::new(),
            pane_selection: None,
            last_click_info: None,
            frozen_pty_snapshot: None,
        }
    }
}
