use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{self, IsTerminal};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use grein_core::blackboard::{BlackboardEntry, BlackboardEntryKind};
use grein_core::model::{
    AnchorId, AuthorRole, BlackboardEntryId, ConversationNode, NodeId, ThreadId,
};
use grein_core::storage::sqlite::SqliteStorage;
use ratatui::backend::Backend;
use ratatui::Terminal;
use tokio::sync::mpsc;

use crate::app::{
    find_config_option, match_config_choice, AnchorItem, App, AppAction, CopyMode, FocusedPane,
    PendingPermission, ThreadItem, TreeAction, TuiError,
};
use crate::engine_port::{
    AgentOptionsSnapshot, AgentSlashCommand, HydratedSession, PtyScreenSnapshot, ThreadMeta,
    TuiEngineEvent, TuiEnginePort,
};
use crate::transcript::{ThreadTranscript, TranscriptBlock};

const USER_PREFIX: &str = "# User: ";
const AGENT_HEADER: &str = "## Agent (agy)";
const ASSISTANT_HEADER: &str = "## Assistant";

pub struct TerminalGuard {
    active: bool,
    keyboard_enhanced: bool,
}

impl TerminalGuard {
    pub fn new() -> io::Result<Self> {
        if io::stdout().is_terminal() && io::stdin().is_terminal() {
            enable_raw_mode()?;
            execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
            let keyboard_enhanced =
                if crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false) {
                    execute!(
                        io::stdout(),
                        PushKeyboardEnhancementFlags(
                            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                        )
                    )
                    .is_ok()
                } else {
                    false
                };
            Ok(Self {
                active: true,
                keyboard_enhanced,
            })
        } else {
            Ok(Self {
                active: false,
                keyboard_enhanced: false,
            })
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.active {
            if self.keyboard_enhanced {
                let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
            }
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), DisableBracketedPaste, LeaveAlternateScreen);
        }
    }
}

pub fn draw_frame<B: Backend>(terminal: &mut Terminal<B>, app: &App) -> io::Result<()> {
    terminal.draw(|frame| crate::render::render(frame, app))?;
    Ok(())
}

pub fn draw_frame_mut<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> io::Result<()> {
    terminal.draw(|frame| crate::render::render_mut(frame, app))?;
    Ok(())
}

pub fn handle_event(app: &mut App, event: Event) -> io::Result<bool> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Release => {}
        Event::Key(key) => {
            let _ = handle_key_event(app, key);
        }
        Event::Paste(text) => {
            let _ = app.handle_paste_event(&text);
        }
        Event::Mouse(mouse) => {
            let _ = app.handle_mouse_event(mouse);
        }
        Event::Resize(w, h) => {
            app.last_frame_area = Some(ratatui::layout::Rect::new(0, 0, w, h));
            app.maybe_queue_terminal_resize();
        }
        _ => {}
    }
    Ok(app.should_quit())
}

/// Structured view of the transcript: ordered (role, content) turns.
fn collect_turns(transcript: &str) -> Vec<(AuthorRole, String)> {
    let mut turns: Vec<(AuthorRole, String)> = Vec::new();
    let mut current: Option<(AuthorRole, String)> = None;

    for line in transcript.lines() {
        if let Some(rest) = line.strip_prefix(USER_PREFIX) {
            if let Some((role, content)) = current.take() {
                turns.push((role, content.trim().to_string()));
            }
            current = Some((AuthorRole::User, rest.to_string()));
        } else if line == AGENT_HEADER || line == ASSISTANT_HEADER {
            if let Some((role, content)) = current.take() {
                turns.push((role, content.trim().to_string()));
            }
            current = Some((AuthorRole::Assistant, String::new()));
        } else if let Some((_, content)) = current.as_mut() {
            if !content.is_empty() {
                content.push('\n');
            }
            content.push_str(line);
        }
    }
    if let Some((role, content)) = current.take() {
        turns.push((role, content.trim().to_string()));
    }
    turns
}

fn render_node(role: &AuthorRole, content: &str) -> String {
    match role {
        AuthorRole::User => format!("{USER_PREFIX}{content}\n\n"),
        _ => format!("{AGENT_HEADER}\n{content}\n\n"),
    }
}

fn ensure_paragraph_break(buf: &mut String) {
    if !buf.is_empty() {
        if !buf.ends_with('\n') {
            buf.push('\n');
        }
        if !buf.ends_with("\n\n") {
            buf.push('\n');
        }
    }
}

fn strip_trailing_shell_stub(buf: &mut String, command: &str) {
    let trimmed_end = buf.trim_end();
    let stub = format!("> $ {command}");
    if let Some(prefix) = trimmed_end.strip_suffix(&stub) {
        if prefix.is_empty() || prefix.ends_with('\n') {
            let clean_len = prefix.trim_end_matches(['\r', '\n']).len();
            buf.truncate(clean_len);
        }
    }
}


pub fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn parse_anchors_for_transcript(transcript: &str) -> Vec<AnchorItem> {
    grein_core::anchor::parse_anchors(transcript)
        .into_iter()
        .filter(|a| {
            let raw = transcript
                .get(a.byte_range.clone())
                .map(str::trim)
                .unwrap_or("");
            !raw.starts_with(USER_PREFIX) && raw != AGENT_HEADER && raw != ASSISTANT_HEADER
        })
        .map(|a| AnchorItem {
            id: a.slug,
            label: a.title,
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderedThreadEntry {
    pub index: usize,
    pub depth: usize,
    pub prefix: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldedThreadEntry {
    pub index: usize,
    pub depth: usize,
    pub prefix: String,
    pub has_children: bool,
    pub hidden_descendants: usize,
}

struct FoldedTreeOrderCtx<'a> {
    threads: &'a [ThreadItem],
    children: HashMap<&'a str, Vec<usize>>,
    collapsed: &'a BTreeSet<String>,
    visited: Vec<bool>,
    ordered: Vec<FoldedThreadEntry>,
}

impl<'a> FoldedTreeOrderCtx<'a> {
    fn count_hidden_descendants(&mut self, idx: usize) -> usize {
        let mut count = 0usize;
        let Some(child_indices) = self.children.get(self.threads[idx].id.as_str()).cloned() else {
            return 0;
        };
        for child_idx in child_indices {
            if !self.visited[child_idx] {
                self.visited[child_idx] = true;
                count += 1;
                count += self.count_hidden_descendants(child_idx);
            }
        }
        count
    }

    fn visit(
        &mut self,
        idx: usize,
        depth: usize,
        prefix: String,
        ancestor_continuations: &[bool],
    ) {
        if self.visited[idx] {
            return;
        }
        self.visited[idx] = true;
        let pending_children: Vec<usize> = self
            .children
            .get(self.threads[idx].id.as_str())
            .map(|kids| kids.iter().copied().filter(|&c| !self.visited[c]).collect())
            .unwrap_or_default();
        let has_children = !pending_children.is_empty();
        let is_collapsed = has_children && self.collapsed.contains(&self.threads[idx].id);
        let entry_pos = self.ordered.len();
        self.ordered.push(FoldedThreadEntry {
            index: idx,
            depth,
            prefix,
            has_children,
            hidden_descendants: 0,
        });
        if is_collapsed {
            let hidden = self.count_hidden_descendants(idx);
            self.ordered[entry_pos].hidden_descendants = hidden;
            return;
        }
        let total = pending_children.len();
        for (pos, child_idx) in pending_children.into_iter().enumerate() {
            let is_last = pos + 1 == total;
            let mut child_prefix = String::new();
            for &has_more in ancestor_continuations {
                child_prefix.push_str(if has_more { "│  " } else { "   " });
            }
            child_prefix.push_str(if is_last { "└─ " } else { "├─ " });

            let mut next_continuations = ancestor_continuations.to_vec();
            next_continuations.push(!is_last);
            self.visit(child_idx, depth + 1, child_prefix, &next_continuations);
        }
    }
}

/// Orders threads in parent-child depth-first order while hiding descendants of folded threads.
pub fn compute_ordered_thread_entries_with_folds(
    threads: &[ThreadItem],
    thread_meta: &HashMap<String, ThreadMeta>,
    collapsed: &BTreeSet<String>,
) -> Vec<FoldedThreadEntry> {
    let known_ids: HashSet<&str> = threads.iter().map(|t| t.id.as_str()).collect();
    let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut roots: Vec<usize> = Vec::new();

    for (idx, thread) in threads.iter().enumerate() {
        let parent = thread_meta
            .get(&thread.id)
            .and_then(|m| m.parent_thread_id.as_deref());
        if let Some(pid) = parent {
            if pid != thread.id && known_ids.contains(pid) {
                children.entry(pid).or_default().push(idx);
                continue;
            }
        }
        roots.push(idx);
    }

    let mut ctx = FoldedTreeOrderCtx {
        threads,
        children,
        collapsed,
        visited: vec![false; threads.len()],
        ordered: Vec::with_capacity(threads.len()),
    };
    for root_idx in roots {
        ctx.visit(root_idx, 0, String::new(), &[]);
    }
    for idx in 0..threads.len() {
        if !ctx.visited[idx] {
            ctx.visit(idx, 0, String::new(), &[]);
        }
    }
    ctx.ordered
}

struct TreeOrderCtx<'a> {
    threads: &'a [ThreadItem],
    children: HashMap<&'a str, Vec<usize>>,
    visited: Vec<bool>,
    ordered: Vec<OrderedThreadEntry>,
}

impl<'a> TreeOrderCtx<'a> {
    fn visit(
        &mut self,
        idx: usize,
        depth: usize,
        prefix: String,
        ancestor_continuations: &[bool],
    ) {
        if self.visited[idx] {
            return;
        }
        self.visited[idx] = true;
        self.ordered.push(OrderedThreadEntry {
            index: idx,
            depth,
            prefix,
        });
        let Some(child_indices) = self.children.get(self.threads[idx].id.as_str()) else {
            return;
        };
        let pending_children: Vec<usize> = child_indices
            .iter()
            .copied()
            .filter(|&c| !self.visited[c])
            .collect();
        let total = pending_children.len();
        for (pos, child_idx) in pending_children.into_iter().enumerate() {
            let is_last = pos + 1 == total;
            let mut child_prefix = String::new();
            for &has_more in ancestor_continuations {
                child_prefix.push_str(if has_more { "│  " } else { "   " });
            }
            child_prefix.push_str(if is_last { "└─ " } else { "├─ " });

            let mut next_continuations = ancestor_continuations.to_vec();
            next_continuations.push(!is_last);
            self.visit(child_idx, depth + 1, child_prefix, &next_continuations);
        }
    }
}

/// Orders threads in parent-child depth-first order along with nesting depth and box-drawing prefix.
pub fn compute_ordered_thread_entries(
    threads: &[ThreadItem],
    thread_meta: &HashMap<String, ThreadMeta>,
) -> Vec<OrderedThreadEntry> {
    let known_ids: HashSet<&str> = threads.iter().map(|t| t.id.as_str()).collect();
    let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut roots: Vec<usize> = Vec::new();

    for (idx, thread) in threads.iter().enumerate() {
        let parent = thread_meta
            .get(&thread.id)
            .and_then(|m| m.parent_thread_id.as_deref());
        if let Some(pid) = parent {
            if pid != thread.id && known_ids.contains(pid) {
                children.entry(pid).or_default().push(idx);
                continue;
            }
        }
        roots.push(idx);
    }

    let mut ctx = TreeOrderCtx {
        threads,
        children,
        visited: vec![false; threads.len()],
        ordered: Vec::with_capacity(threads.len()),
    };
    for root_idx in roots {
        ctx.visit(root_idx, 0, String::new(), &[]);
    }
    for idx in 0..threads.len() {
        if !ctx.visited[idx] {
            ctx.visit(idx, 0, String::new(), &[]);
        }
    }
    ctx.ordered
}

/// Orders threads in parent-child depth-first order along with their nesting depth.
pub fn compute_ordered_thread_indices(
    threads: &[ThreadItem],
    thread_meta: &HashMap<String, ThreadMeta>,
) -> Vec<(usize, usize)> {
    compute_ordered_thread_entries(threads, thread_meta)
        .into_iter()
        .map(|entry| (entry.index, entry.depth))
        .collect()
}

pub fn rebuild_tree_nodes(app: &mut App) {
    let ordered = compute_ordered_thread_entries_with_folds(
        &app.threads,
        &app.thread_meta,
        &app.collapsed_threads,
    );
    let mut nodes = Vec::with_capacity(ordered.len());
    let mut depths = BTreeMap::new();
    let mut prefixes = BTreeMap::new();
    let mut has_children_set = BTreeSet::new();
    let mut hidden_counts = BTreeMap::new();

    for entry in ordered {
        let thread = &app.threads[entry.index];
        depths.insert(thread.id.clone(), entry.depth);
        prefixes.insert(thread.id.clone(), entry.prefix);
        if entry.has_children {
            has_children_set.insert(thread.id.clone());
        }
        if entry.hidden_descendants > 0 {
            hidden_counts.insert(thread.id.clone(), entry.hidden_descendants);
        }
        nodes.push(crate::ui::tree::TreeNode::Thread(
            crate::ui::tree::ThreadNode {
                id: thread.id.clone(),
                title: thread.title.clone(),
            },
        ));
    }
    app.tree_has_children = has_children_set;
    app.tree_hidden_counts = hidden_counts;

    let prev_selected_id = app
        .tree
        .selected_index()
        .and_then(|idx| app.tree.nodes.get(idx))
        .map(|node| match node {
            crate::ui::tree::TreeNode::Thread(t) => t.id.clone(),
            crate::ui::tree::TreeNode::Anchor(a) => a.thread_id.clone(),
        });
    let prev_selected = app.tree.selected_index();
    let prev_scroll = app.tree.scroll_offset;
    let prev_active = app.tree.active_anchor_id().map(str::to_string);
    let node_count = nodes.len();

    app.tree = crate::ui::tree::TreeState::new(nodes);
    app.tree.set_depths(depths);
    app.tree.set_prefixes(prefixes);
    app.tree.set_scroll_offset(prev_scroll);

    if node_count > 0 {
        let restored_idx = prev_selected_id
            .and_then(|id| {
                app.tree.nodes.iter().position(|n| {
                    matches!(n, crate::ui::tree::TreeNode::Thread(t) if t.id == id)
                })
            })
            .or_else(|| prev_selected.map(|i| i.min(node_count - 1)));
        if let Some(idx) = restored_idx {
            app.tree.select_index(idx);
        }
    }
    if let Some(active_id) = prev_active {
        app.tree.set_active_anchor_id(Some(active_id));
    }
}

fn with_thread_transcript_mut<F>(app: &mut App, thread_id: &str, f: F)
where
    F: FnOnce(&mut String),
{
    if thread_id == app.active_thread_id {
        f(&mut app.active_transcript_md);
        app.thread_transcripts
            .insert(thread_id.to_string(), app.active_transcript_md.clone());
    } else {
        let entry = app
            .thread_transcripts
            .entry(thread_id.to_string())
            .or_default();
        f(entry);
    }
}

fn sync_thread_blocks_md(app: &mut App, thread_id: &str) {
    let md = if thread_id == app.active_thread_id {
        app.active_transcript_md.clone()
    } else {
        app.thread_transcripts
            .get(thread_id)
            .cloned()
            .unwrap_or_default()
    };
    app.thread_blocks_synced_md
        .insert(thread_id.to_string(), md);
}

fn build_blocks_from_turns(turns: &[crate::engine_port::HydratedTurn]) -> ThreadTranscript {
    let mut blocks = ThreadTranscript::new();
    for turn in turns {
        if turn.role.eq_ignore_ascii_case("user") {
            blocks.push_user(&turn.content);
        } else {
            blocks.blocks.push(TranscriptBlock::AssistantMarkdown {
                content: turn.content.clone(),
            });
        }
    }
    blocks
}

fn parse_config_arg(rest: &str) -> (bool, String) {
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

fn parse_term_cwd(rest: &str) -> Option<String> {
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

fn format_tool_result(tool_name: &str, output: &str) -> String {
    let trimmed = output.trim_end_matches(['\r', '\n']);
    if trimmed.trim().is_empty() {
        format!("> ✓ `{tool_name}`\n")
    } else if !trimmed.contains('\n') {
        format!("> ✓ `{tool_name}`: {}\n", trimmed.trim())
    } else {
        format!("> ✓ `{tool_name}`:\n```text\n{trimmed}\n```\n")
    }
}

impl App {
    pub(crate) fn record_agent_commands_note(
        &mut self,
        thread_id: &str,
        commands: &[AgentSlashCommand],
    ) {
        if commands.is_empty() {
            self.status_notice = Some("No agent commands advertised".to_string());
            return;
        }
        let mut names = Vec::with_capacity(commands.len());
        let mut lines = Vec::with_capacity(commands.len());
        for cmd in commands {
            let cmd_name = if cmd.name.starts_with('/') {
                cmd.name.clone()
            } else {
                format!("/{}", cmd.name)
            };
            names.push(cmd_name.clone());
            if cmd.description.trim().is_empty() {
                lines.push(cmd_name);
            } else {
                lines.push(format!("{cmd_name} — {}", cmd.description.trim()));
            }
        }
        let note = format!("Agent commands:\n{}", lines.join("\n"));
        let md_note = lines
            .iter()
            .map(|l| format!("> ↳ {l}"))
            .collect::<Vec<_>>()
            .join("\n");
        with_thread_transcript_mut(self, thread_id, |buf| {
            ensure_paragraph_break(buf);
            buf.push_str(&md_note);
            buf.push_str("\n\n");
        });
        if thread_id == self.active_thread_id {
            self.sync_anchors();
        }
        let resolved_id = if thread_id == "trunk" && self.active_thread_id != "trunk" {
            self.active_thread_id.clone()
        } else {
            thread_id.to_string()
        };
        self.thread_blocks
            .entry(resolved_id.clone())
            .or_default()
            .push_join_note(note);
        sync_thread_blocks_md(self, &resolved_id);
        self.status_notice = Some(format!("Commands: {}", names.join(", ")));
    }

    pub fn submit_composer_prompt(&mut self, prompt: impl Into<String>) {
        let prompt = prompt.into();
        let trimmed = prompt.trim();
        if trimmed.is_empty() {
            return;
        }

        if let Some(rest) = trimmed.strip_prefix('!') {
            let command = rest.trim();
            if !command.is_empty() {
                ensure_paragraph_break(&mut self.active_transcript_md);
                self.active_transcript_md
                    .push_str(&format!("> $ {command}\n\n"));
                self.sync_anchors();
                let active_id = self.active_thread_id.clone();
                self.thread_blocks
                    .entry(active_id.clone())
                    .or_default()
                    .push_shell_pending(command);
                sync_thread_blocks_md(self, &active_id);
                self.transcript_viewport_mut(&active_id).scroll_to_bottom();
                self.status_notice = None;
                self.thread_active_tool.remove(&active_id);
                self.thread_turn_started_ms
                    .insert(active_id.clone(), now_epoch_ms());
                self.thread_statuses
                    .insert(active_id.clone(), "executing".to_string());
                self.pending_actions.push_back(AppAction::RunShell {
                    thread_id: active_id,
                    command: command.to_string(),
                });
            }
            return;
        }

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
                return;
            }
            "/zoom" => {
                self.toggle_sidebar_zoom();
                return;
            }
            "/copy" => {
                match rest {
                    "" => self.copy_transcript_content(CopyMode::LastReply),
                    "code" => self.copy_transcript_content(CopyMode::CodeBlock),
                    "all" => self.copy_transcript_content(CopyMode::All),
                    _ => {
                        self.status_notice = Some("Usage: /copy [code|all]".to_string());
                    }
                }
                return;
            }
            "/commands" => {
                let active_id = self.active_thread_id.clone();
                if let Some(cmds) = self.thread_commands.get(&active_id).cloned() {
                    if !cmds.is_empty() {
                        self.record_agent_commands_note(&active_id, &cmds);
                        return;
                    }
                }
                self.status_notice = Some("Fetching agent commands...".to_string());
                self.pending_actions
                    .push_back(AppAction::FetchAgentCommands {
                        thread_id: active_id,
                    });
                return;
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
                    return;
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
                return;
            }
            "/delete" | "/close" => {
                let target_id = if rest.is_empty() {
                    Some(self.active_thread_id.clone())
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
                return;
            }
            "/branch" | "/fork" | "/grein" => {
                let title = if rest.is_empty() {
                    None
                } else {
                    Some(rest.to_string())
                };
                self.pending_actions.push_back(AppAction::ForkBranch {
                    source_thread_id: self.active_thread_id.clone(),
                    title,
                });
                return;
            }
            "/root" | "/new" => {
                let title = (!rest.is_empty()).then(|| rest.to_string());
                self.status_notice = None;
                self.pending_actions
                    .push_back(AppAction::CreateRoot { title });
                return;
            }
            "/rename" => {
                if rest.is_empty() {
                    self.status_notice = Some("Usage: /rename <new title>".to_string());
                } else {
                    let thread_id = self
                        .pending_rename_thread_id
                        .take()
                        .unwrap_or_else(|| self.active_thread_id.clone());
                    self.status_notice = None;
                    self.pending_actions.push_back(AppAction::RenameThread {
                        thread_id,
                        title: rest.to_string(),
                    });
                    self.sync_focus_for_active_thread();
                }
                return;
            }
            "/join" | "/merge" => {
                let source_thread_id = self.active_thread_id.clone();
                let summary = (!rest.is_empty()).then(|| rest.to_string());
                self.request_join_thread(source_thread_id, summary);
                return;
            }
            "/term" | "/terminal" | "/zsh" | "/sh" | "/shell" | "/bash" => {
                let cwd = parse_term_cwd(rest);
                let active_id = self.active_thread_id.clone();
                self.dispatch_create_terminal(Some(active_id), cwd);
                return;
            }
            "/worktree" => {
                let active_id = self.active_thread_id.clone();
                self.status_notice = Some("Allocating worktree...".to_string());
                self.pending_actions
                    .push_back(AppAction::EnsureThreadWorktree {
                        thread_id: active_id,
                    });
                return;
            }
            "/quit" => {
                self.should_quit = true;
                return;
            }
            _ => {}
        }

        ensure_paragraph_break(&mut self.active_transcript_md);
        self.active_transcript_md
            .push_str(&render_node(&AuthorRole::User, trimmed));
        self.last_submitted_prompt = Some(trimmed.to_string());
        self.sync_anchors();
        let active_id = self.active_thread_id.clone();
        self.thread_blocks
            .entry(active_id.clone())
            .or_default()
            .push_user(trimmed);
        sync_thread_blocks_md(self, &active_id);
        self.transcript_viewport_mut(&active_id).scroll_to_bottom();
        self.status_notice = None;
        self.thread_active_tool.remove(&active_id);
        self.thread_turn_started_ms
            .insert(active_id.clone(), now_epoch_ms());
        self.thread_statuses
            .insert(active_id.clone(), "streaming".to_string());
        self.pending_actions.push_back(AppAction::SendPrompt {
            thread_id: active_id,
            prompt: trimmed.to_string(),
        });
    }

    pub fn apply_agent_response(&mut self, response: &str) {
        let clean = response.trim_end_matches('\n');
        let last_user = self.active_transcript_md.rfind(USER_PREFIX);
        let agent_marker = format!("{AGENT_HEADER}\n");
        let last_agent = self.active_transcript_md.rfind(agent_marker.as_str());
        let has_open_agent_turn = match (last_user, last_agent) {
            (Some(u), Some(a)) => a > u,
            (None, Some(_)) => true,
            _ => false,
        };
        if has_open_agent_turn {
            let trimmed_len = self.active_transcript_md.trim_end_matches('\n').len();
            self.active_transcript_md.truncate(trimmed_len);
            self.active_transcript_md.push('\n');
            self.active_transcript_md.push_str(clean);
            self.active_transcript_md.push_str("\n\n");
        } else {
            ensure_paragraph_break(&mut self.active_transcript_md);
            self.active_transcript_md
                .push_str(&render_node(&AuthorRole::Assistant, clean));
        }
        self.sync_anchors();
        let active_id = self.active_thread_id.clone();
        let blocks = self.thread_blocks.entry(active_id.clone()).or_default();
        if has_open_agent_turn {
            if let Some(TranscriptBlock::AssistantMarkdown { content }) = blocks.blocks.last_mut() {
                if !content.is_empty() && !clean.is_empty() {
                    content.push('\n');
                }
                content.push_str(clean);
            } else {
                blocks.append_assistant_chunk(clean);
            }
        } else {
            blocks.append_assistant_chunk(clean);
        }
        sync_thread_blocks_md(self, &active_id);
    }

    pub fn sync_anchors(&mut self) {
        let turns = collect_turns(&self.active_transcript_md);
        if self.threads.is_empty() && !turns.is_empty() {
            let title = turns
                .iter()
                .find(|(role, _)| *role == AuthorRole::User)
                .map(|(_, content)| {
                    content
                        .lines()
                        .next()
                        .unwrap_or("Active Session")
                        .to_string()
                })
                .unwrap_or_else(|| "Active Session".to_string());
            let old_id = self.active_thread_id.clone();
            let new_id = if self.active_thread_id.is_empty() || self.active_thread_id == "trunk" {
                let id = ThreadId::now_v7().to_string();
                self.active_thread_id = id.clone();
                id
            } else {
                self.active_thread_id.clone()
            };
            if old_id != new_id {
                if let Some(prev_blocks) = self.thread_blocks.remove(&old_id) {
                    self.thread_blocks.insert(new_id.clone(), prev_blocks);
                }
                if let Some(prev_synced) = self.thread_blocks_synced_md.remove(&old_id) {
                    self.thread_blocks_synced_md
                        .insert(new_id.clone(), prev_synced);
                }
                if let Some(prev_vp) = self.thread_viewports.remove(&old_id) {
                    self.thread_viewports.insert(new_id.clone(), prev_vp);
                }
                self.thread_transcripts.remove(&old_id);
            }
            self.threads.push(ThreadItem {
                id: new_id,
                title,
                anchors: Vec::new(),
            });
        }

        self.thread_transcripts.insert(
            self.active_thread_id.clone(),
            self.active_transcript_md.clone(),
        );

        let anchor_items = parse_anchors_for_transcript(&self.active_transcript_md);
        if let Some(thread) = self
            .threads
            .iter_mut()
            .find(|t| t.id == self.active_thread_id)
        {
            thread.anchors = anchor_items;
        } else if let Some(thread) = self.threads.first_mut() {
            thread.anchors = anchor_items;
        }

        rebuild_tree_nodes(self);
    }

    pub fn persist_state_to_sqlite(&self, db_path: &Path) {
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let Ok(storage) = SqliteStorage::open(db_path) else {
            return;
        };
        let turns = collect_turns(&self.active_transcript_md);
        if turns.is_empty() {
            return;
        }

        let thread_id = self
            .threads
            .first()
            .and_then(|t| t.id.parse().ok().map(ThreadId))
            .or_else(|| {
                storage
                    .get_blackboard_entry("active_thread_id")
                    .ok()
                    .flatten()
                    .and_then(|e| e.value.parse().ok().map(ThreadId))
            });

        let existing_thread = thread_id.and_then(|tid| storage.get_thread(tid).ok().flatten());

        let existing_trunk = existing_thread
            .as_ref()
            .and_then(|t| storage.get_trunk_path(t.active_leaf_id).ok())
            .unwrap_or_default();

        let common_len = existing_trunk
            .iter()
            .zip(turns.iter())
            .take_while(|(node, (role, content))| node.role == *role && node.content == *content)
            .count();

        let mut parent_id = if common_len > 0 {
            existing_trunk.get(common_len - 1).map(|n| n.id)
        } else {
            None
        };
        let mut root_id = if common_len > 0 {
            existing_trunk.first().map(|n| n.id)
        } else {
            None
        };
        let mut last_node_id = parent_id;

        for (role, content) in turns.into_iter().skip(common_len) {
            let node_id = NodeId::now_v7();
            let node = ConversationNode::new(node_id, parent_id, role, content);
            if storage.insert_node(&node).is_ok() {
                if root_id.is_none() {
                    root_id = Some(node_id);
                }
                parent_id = Some(node_id);
                last_node_id = Some(node_id);
            }
        }

        let (Some(r_id), Some(leaf_id)) = (root_id, last_node_id) else {
            return;
        };

        let real_tid = if let Some(existing) = existing_thread {
            let _ = storage.update_active_leaf(existing.id, leaf_id);
            existing.id
        } else {
            match storage.create_thread(r_id, AnchorId::now_v7()) {
                Ok(tid) => {
                    let _ = storage.update_active_leaf(tid, leaf_id);
                    tid
                }
                Err(_) => return,
            }
        };

        let title = self
            .threads
            .first()
            .map(|t| t.title.clone())
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| "Active Session".to_string());

        let entries = [
            ("active_thread_id".to_string(), real_tid.to_string()),
            ("active_thread_title".to_string(), title),
        ];

        for (key, value) in entries {
            let _ = storage.delete_blackboard_entry(&key);
            let _ = storage.insert_blackboard_entry(&BlackboardEntry {
                id: BlackboardEntryId::now_v7(),
                key,
                value,
                kind: BlackboardEntryKind::Note,
                created_at: 1,
            });
        }
    }

    pub fn load_state_from_sqlite(&mut self, db_path: &Path) {
        if !db_path.exists() {
            return;
        }
        let Ok(storage) = SqliteStorage::open(db_path) else {
            return;
        };
        let Some(tid) = storage
            .get_blackboard_entry("active_thread_id")
            .ok()
            .flatten()
            .and_then(|e| e.value.parse().ok().map(ThreadId))
        else {
            return;
        };
        let Some(thread) = storage.get_thread(tid).ok().flatten() else {
            return;
        };
        let Ok(trunk) = storage.get_trunk_path(thread.active_leaf_id) else {
            return;
        };

        let mut reconstructed = String::new();
        let mut blocks = ThreadTranscript::new();
        let mut first_user_content: Option<String> = None;
        for node in &trunk {
            if node.role == AuthorRole::User {
                if first_user_content.is_none() {
                    first_user_content = Some(node.content.clone());
                }
                blocks.push_user(&node.content);
            } else {
                blocks.blocks.push(TranscriptBlock::AssistantMarkdown {
                    content: node.content.clone(),
                });
            }
            reconstructed.push_str(&render_node(&node.role, &node.content));
        }
        self.active_transcript_md = reconstructed;

        let title = storage
            .get_blackboard_entry("active_thread_title")
            .ok()
            .flatten()
            .map(|e| e.value)
            .filter(|v| !v.trim().is_empty())
            .or(first_user_content)
            .unwrap_or_else(|| "Active Session".to_string());

        let tid_str = thread.id.to_string();
        self.threads = vec![ThreadItem {
            id: tid_str.clone(),
            title,
            anchors: Vec::new(),
        }];
        self.active_thread_id = tid_str.clone();
        self.thread_blocks.clear();
        self.thread_blocks_synced_md.clear();

        self.sync_anchors();
        self.thread_blocks.insert(tid_str.clone(), blocks);
        sync_thread_blocks_md(self, &tid_str);
    }
}

pub fn handle_key_event(app: &mut App, key: KeyEvent) -> Result<Option<TreeAction>, TuiError> {
    if key.kind == KeyEventKind::Release {
        return Ok(None);
    }
    let was_composer = app.focused_pane() == FocusedPane::Composer
        || app.focus() == FocusedPane::Composer
        || app.active_focus() == FocusedPane::Composer;
    let is_enter = key.code == KeyCode::Enter
        && !key
            .modifiers
            .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT);
    if was_composer && is_enter {
        app.last_submitted_prompt = None;
    }
    app.handle_key_event(key);
    if key.code == KeyCode::Enter && app.focused_pane() == FocusedPane::Tree {
        if let Some(idx) = app.tree.selected_index() {
            if let Some(crate::ui::tree::TreeNode::Anchor(anchor)) = app.tree.nodes.get(idx) {
                return Ok(Some(TreeAction::ActivateAnchor(anchor.id.clone())));
            }
        }
    }
    Ok(None)
}

fn render_turns_md(turns: &[crate::engine_port::HydratedTurn]) -> String {
    let mut transcript = String::new();
    for turn in turns {
        let role = if turn.role.eq_ignore_ascii_case("user") {
            AuthorRole::User
        } else {
            AuthorRole::Assistant
        };
        transcript.push_str(&render_node(&role, &turn.content));
    }
    transcript
}

/// Seeds the app (threads, transcript nodes) from a hydrated session.
pub fn apply_hydrated_session(app: &mut App, session: &HydratedSession) {
    app.threads.clear();
    app.thread_transcripts.clear();
    app.thread_blocks.clear();
    app.thread_blocks_synced_md.clear();
    app.thread_statuses.clear();
    app.streaming_assistant_threads.clear();
    app.thread_thoughts.clear();
    app.thread_meta.clear();
    app.thread_subagents.clear();
    app.thread_agent_meta.clear();
    app.thread_config_options.clear();
    app.thread_commands.clear();
    app.thread_active_tool.clear();
    app.thread_turn_started_ms.clear();
    app.pty_screens.clear();
    app.pty_exit_codes.clear();
    app.last_pty_sizes.clear();
    app.config_picker = None;
    app.help_scroll_offset = 0;
    app.status_notice = None;
    app.pending_rename_thread_id = None;
    app.transcript_search.clear();
    app.pending_g_prefix = false;
    app.collapsed_threads.clear();
    app.tree_has_children.clear();
    app.tree_hidden_counts.clear();

    for t in &session.threads {
        let transcript = render_turns_md(&t.turns);
        let blocks = build_blocks_from_turns(&t.turns);
        let anchors = parse_anchors_for_transcript(&transcript);
        app.thread_blocks_synced_md
            .insert(t.id.clone(), transcript.clone());
        app.thread_blocks.insert(t.id.clone(), blocks);
        app.thread_transcripts.insert(t.id.clone(), transcript);
        app.thread_statuses
            .entry(t.id.clone())
            .or_insert_with(|| "idle".to_string());
        app.threads.push(ThreadItem {
            id: t.id.clone(),
            title: t.title.clone(),
            anchors,
        });
    }

    let active_id = if !session.active_thread_id.is_empty()
        && session.threads.iter().any(|t| t.id == session.active_thread_id)
    {
        session.active_thread_id.clone()
    } else if let Some(first) = session.threads.first() {
        first.id.clone()
    } else {
        session.active_thread_id.clone()
    };

    if !active_id.is_empty() {
        app.active_thread_id = active_id;
    }
    app.active_transcript_md = app
        .thread_transcripts
        .get(&app.active_thread_id)
        .cloned()
        .unwrap_or_default();

    app.sync_anchors();
    app.select_active_thread_in_tree();
}

/// Folds a streamed engine event into the app state.
pub fn apply_engine_event(app: &mut App, event: TuiEngineEvent) {
    match event {
        TuiEngineEvent::StreamChunk { thread_id, content } => {
            app.thread_thoughts.remove(&thread_id);
            app.thread_turn_started_ms
                .entry(thread_id.clone())
                .or_insert_with(now_epoch_ms);
            app.thread_statuses
                .insert(thread_id.clone(), "streaming".to_string());
            let is_first = app.streaming_assistant_threads.insert(thread_id.clone());
            with_thread_transcript_mut(app, &thread_id, |buf| {
                if is_first {
                    ensure_paragraph_break(buf);
                    buf.push_str(ASSISTANT_HEADER);
                    buf.push('\n');
                }
                buf.push_str(&content);
            });
            app.thread_blocks
                .entry(thread_id.clone())
                .or_default()
                .append_assistant_chunk(&content);
            sync_thread_blocks_md(app, &thread_id);
        }
        TuiEngineEvent::ThoughtChunk { thread_id, content } => {
            app.thread_turn_started_ms
                .entry(thread_id.clone())
                .or_insert_with(now_epoch_ms);
            let is_executing = app
                .thread_statuses
                .get(&thread_id)
                .map(String::as_str)
                == Some("executing");
            if !is_executing {
                app.thread_statuses
                    .insert(thread_id.clone(), "thinking".to_string());
            }
            app.thread_thoughts
                .entry(thread_id)
                .or_default()
                .push_str(&content);
        }
        TuiEngineEvent::ToolInvoked {
            thread_id,
            tool_name,
            tool_call_id,
            summary,
        } => {
            app.thread_thoughts.remove(&thread_id);
            app.thread_turn_started_ms
                .entry(thread_id.clone())
                .or_insert_with(now_epoch_ms);
            app.thread_active_tool
                .insert(thread_id.clone(), Some(tool_name.clone()));
            app.thread_statuses
                .insert(thread_id.clone(), "executing".to_string());
            let is_first = app.streaming_assistant_threads.insert(thread_id.clone());
            with_thread_transcript_mut(app, &thread_id, |buf| {
                if is_first {
                    ensure_paragraph_break(buf);
                    buf.push_str(ASSISTANT_HEADER);
                    buf.push('\n');
                } else if !buf.is_empty() && !buf.ends_with('\n') {
                    buf.push('\n');
                }
                buf.push_str(&format!("> ⚙ tool: `{tool_name}`\n"));
            });
            app.thread_blocks
                .entry(thread_id.clone())
                .or_default()
                .start_tool_call(tool_call_id, tool_name, summary);
            sync_thread_blocks_md(app, &thread_id);
        }
        TuiEngineEvent::ToolResult {
            thread_id,
            tool_name,
            output,
            tool_call_id,
            is_error,
        } => {
            app.thread_thoughts.remove(&thread_id);
            app.thread_active_tool.remove(&thread_id);
            if app
                .thread_statuses
                .get(&thread_id)
                .map(String::as_str)
                == Some("executing")
            {
                app.thread_statuses
                    .insert(thread_id.clone(), "streaming".to_string());
            }
            let is_first = app.streaming_assistant_threads.insert(thread_id.clone());
            let formatted = format_tool_result(&tool_name, &output);
            with_thread_transcript_mut(app, &thread_id, |buf| {
                if is_first {
                    ensure_paragraph_break(buf);
                    buf.push_str(ASSISTANT_HEADER);
                    buf.push('\n');
                } else if !buf.is_empty() && !buf.ends_with('\n') {
                    buf.push('\n');
                }
                buf.push_str(&formatted);
            });
            app.thread_blocks
                .entry(thread_id.clone())
                .or_default()
                .complete_tool_call(tool_call_id.as_deref(), &tool_name, output, is_error);
            sync_thread_blocks_md(app, &thread_id);
        }
        TuiEngineEvent::ToolActivityUpdated {
            thread_id,
            tool_name,
        } => {
            app.thread_thoughts.remove(&thread_id);
            if let Some(name) = tool_name {
                app.thread_turn_started_ms
                    .entry(thread_id.clone())
                    .or_insert_with(now_epoch_ms);
                app.thread_active_tool.insert(thread_id.clone(), Some(name));
                app.thread_statuses
                    .insert(thread_id, "executing".to_string());
            } else {
                app.thread_active_tool.remove(&thread_id);
                if app
                    .thread_statuses
                    .get(&thread_id)
                    .map(String::as_str)
                    == Some("executing")
                {
                    app.thread_statuses
                        .insert(thread_id, "streaming".to_string());
                }
            }
        }
        TuiEngineEvent::ShellOutput {
            thread_id,
            command,
            stdout,
            stderr,
            exit_code,
        } => {
            let mut body = String::new();
            if !stdout.is_empty() {
                body.push_str(stdout.trim_end_matches(['\r', '\n']));
            }
            if !stderr.is_empty() {
                let clean_stderr = stderr.trim_end_matches(['\r', '\n']);
                if !clean_stderr.is_empty() {
                    if !body.is_empty() {
                        body.push('\n');
                    }
                    body.push_str("[stderr]\n");
                    body.push_str(clean_stderr);
                }
            }
            let block = if body.is_empty() {
                format!("$ {command} (exit {exit_code})\n\n")
            } else {
                format!("$ {command} (exit {exit_code})\n```ansi\n{body}\n```\n\n")
            };
            with_thread_transcript_mut(app, &thread_id, |buf| {
                strip_trailing_shell_stub(buf, &command);
                ensure_paragraph_break(buf);
                buf.push_str(&block);
            });
            app.thread_blocks
                .entry(thread_id.clone())
                .or_default()
                .complete_shell(&command, stdout, stderr, exit_code);
            sync_thread_blocks_md(app, &thread_id);
            app.thread_active_tool.remove(&thread_id);
            app.thread_turn_started_ms.remove(&thread_id);
            app.thread_statuses.insert(thread_id.clone(), "idle".to_string());
            if thread_id == app.active_thread_id {
                app.sync_anchors();
            }
        }
        TuiEngineEvent::ThreadCreated { thread } => {
            if let Some(existing) = app.threads.iter_mut().find(|t| t.id == thread.id) {
                if !thread.title.trim().is_empty() {
                    existing.title = thread.title;
                }
                rebuild_tree_nodes(app);
                return;
            }
            let transcript = render_turns_md(&thread.turns);
            let blocks = build_blocks_from_turns(&thread.turns);
            let anchors = parse_anchors_for_transcript(&transcript);
            let new_id = thread.id.clone();
            app.thread_blocks_synced_md
                .insert(new_id.clone(), transcript.clone());
            app.thread_blocks.insert(new_id.clone(), blocks);
            app.thread_transcripts.insert(new_id.clone(), transcript);
            app.thread_statuses
                .entry(new_id.clone())
                .or_insert_with(|| "idle".to_string());
            app.threads.push(ThreadItem {
                id: new_id.clone(),
                title: thread.title,
                anchors,
            });
            app.switch_thread(&new_id);
            if app.is_terminal_thread(&new_id) {
                app.set_focus(FocusedPane::Terminal);
                app.maybe_request_pty_snapshot(&new_id);
            } else {
                app.set_focus(FocusedPane::Composer);
            }
        }
        TuiEngineEvent::ThreadTitleUpdated { thread_id, title } => {
            if let Some(thread) = app.threads.iter_mut().find(|t| t.id == thread_id) {
                thread.title = title;
            }
            rebuild_tree_nodes(app);
        }
        TuiEngineEvent::ThreadMerged {
            thread_id,
            target_thread_id,
            summary,
        } => {
            let has_target = !target_thread_id.is_empty() && target_thread_id != thread_id;
            app.thread_meta
                .entry(thread_id.clone())
                .and_modify(|m| m.merged = true)
                .or_insert_with(|| ThreadMeta {
                    thread_id: thread_id.clone(),
                    parent_thread_id: has_target.then(|| target_thread_id.clone()),
                    inherited_turn_count: 0,
                    merged: true,
                    kind: grein_core::ThreadKind::Agent,
                    cwd: None,
                    git_branch: None,
                    git_dirty: 0,
                });
            let source_title = app
                .threads
                .iter()
                .find(|t| t.id == thread_id)
                .map(|t| t.title.clone())
                .unwrap_or_else(|| thread_id.clone());
            if has_target {
                let clean_summary = summary.trim();
                let join_note = if clean_summary.is_empty() {
                    format!("Joined from {source_title}")
                } else {
                    format!("Joined from {source_title}: {clean_summary}")
                };
                let note = format!("> ↳ {join_note}\n\n");
                with_thread_transcript_mut(app, &target_thread_id, |buf| {
                    ensure_paragraph_break(buf);
                    buf.push_str(&note);
                });
                app.thread_blocks
                    .entry(target_thread_id.clone())
                    .or_default()
                    .push_join_note(join_note);
                sync_thread_blocks_md(app, &target_thread_id);
                if target_thread_id == app.active_thread_id {
                    app.sync_anchors();
                }
                let target_title = app
                    .threads
                    .iter()
                    .find(|t| t.id == target_thread_id)
                    .map(|t| t.title.as_str())
                    .unwrap_or(target_thread_id.as_str());
                app.status_notice = Some(format!("Joined {source_title} → {target_title}"));
            } else {
                app.status_notice = Some(format!("Joined {source_title} to Shared Context"));
            }
            rebuild_tree_nodes(app);
        }
        TuiEngineEvent::ThreadMetadataUpdated { meta } => {
            app.thread_meta.clear();
            for m in meta {
                app.thread_meta.insert(m.thread_id.clone(), m);
            }
            let active = app.active_thread_id.clone();
            app.unfold_ancestors_for(&active);
            rebuild_tree_nodes(app);
            app.sync_focus_for_active_thread();
            app.maybe_request_pty_snapshot(&active);
            app.maybe_queue_terminal_resize();
        }
        TuiEngineEvent::ThreadDeleted {
            thread_id,
            next_active_thread_id,
        } => {
            app.threads.retain(|t| t.id != thread_id);
            app.thread_transcripts.remove(&thread_id);
            app.thread_blocks.remove(&thread_id);
            app.thread_blocks_synced_md.remove(&thread_id);
            app.thread_statuses.remove(&thread_id);
            app.streaming_assistant_threads.remove(&thread_id);
            app.thread_thoughts.remove(&thread_id);
            app.thread_viewports.remove(&thread_id);
            app.thread_drafts.remove(&thread_id);
            app.thread_meta.remove(&thread_id);
            app.thread_subagents.remove(&thread_id);
            app.thread_agent_meta.remove(&thread_id);
            app.thread_config_options.remove(&thread_id);
            app.thread_commands.remove(&thread_id);
            app.thread_active_tool.remove(&thread_id);
            app.thread_turn_started_ms.remove(&thread_id);
            app.pty_screens.remove(&thread_id);
            app.pty_exit_codes.remove(&thread_id);
            app.last_pty_sizes.remove(&thread_id);
            app.collapsed_threads.remove(&thread_id);
            app.tree_has_children.remove(&thread_id);
            app.tree_hidden_counts.remove(&thread_id);
            if app
                .config_picker
                .as_ref()
                .is_some_and(|p| p.thread_id == thread_id)
            {
                app.config_picker = None;
            }
            app.status_notice = None;
            app.pending_rename_thread_id = None;

            if app.active_thread_id == thread_id
                || !app.threads.iter().any(|t| t.id == app.active_thread_id)
            {
                let target = if !next_active_thread_id.is_empty()
                    && app.threads.iter().any(|t| t.id == next_active_thread_id)
                {
                    next_active_thread_id
                } else {
                    app.threads
                        .first()
                        .map(|t| t.id.clone())
                        .unwrap_or_default()
                };
                app.active_thread_id = target;
                app.input_buffer = app
                    .thread_drafts
                    .get(&app.active_thread_id)
                    .cloned()
                    .unwrap_or_default();
                app.cursor_position = app.input_buffer.len();
                app.active_transcript_md = app
                    .thread_transcripts
                    .get(&app.active_thread_id)
                    .cloned()
                    .unwrap_or_default();
                app.thread_viewports
                    .entry(app.active_thread_id.clone())
                    .or_default();
            }

            let active_for_unfold = app.active_thread_id.clone();
            app.unfold_ancestors_for(&active_for_unfold);
            rebuild_tree_nodes(app);
            app.select_active_thread_in_tree();
            app.sync_focus_for_active_thread();
            let active = app.active_thread_id.clone();
            app.maybe_request_pty_snapshot(&active);
        }
        TuiEngineEvent::AgentMetaUpdated {
            thread_id,
            model,
            mode,
        } => {
            let entry = app.thread_agent_meta.entry(thread_id).or_default();
            if model.is_some() {
                entry.0 = model;
            }
            if mode.is_some() {
                entry.1 = mode;
            }
        }
        TuiEngineEvent::AgentConfigUpdated {
            thread_id,
            config_options,
        } => {
            app.sync_agent_meta_from_config_options(&thread_id, &config_options);
            if let Some(ref mut picker) = app.config_picker {
                if picker.thread_id == thread_id {
                    if let Some(opt) = find_config_option(&config_options, &picker.config_id) {
                        picker.current_value = opt.current_value.clone();
                        if !opt.options.is_empty() {
                            picker.options = opt.options.clone();
                            picker.selected_index =
                                picker.selected_index.min(picker.options.len() - 1);
                        }
                    }
                }
            }
            app.thread_config_options.insert(thread_id, config_options);
        }
        TuiEngineEvent::AgentCommandsUpdated {
            thread_id,
            commands,
        } => {
            app.thread_commands.insert(thread_id, commands);
        }
        TuiEngineEvent::SubagentUpdated {
            thread_id,
            subagent,
        } => {
            let list = app.thread_subagents.entry(thread_id).or_default();
            if let Some(existing) = list.iter_mut().find(|s| s.id == subagent.id) {
                *existing = subagent;
            } else {
                list.push(subagent);
            }
        }
        TuiEngineEvent::SubagentRemoved {
            thread_id,
            subagent_id,
        } => {
            if let Some(list) = app.thread_subagents.get_mut(&thread_id) {
                list.retain(|s| s.id != subagent_id);
                if list.is_empty() {
                    app.thread_subagents.remove(&thread_id);
                }
            }
        }
        TuiEngineEvent::TurnCompleted { thread_id } => {
            app.streaming_assistant_threads.remove(&thread_id);
            app.thread_thoughts.remove(&thread_id);
            app.thread_active_tool.remove(&thread_id);
            app.thread_turn_started_ms.remove(&thread_id);
            app.thread_statuses
                .insert(thread_id.clone(), "idle".to_string());
            with_thread_transcript_mut(app, &thread_id, ensure_paragraph_break);
            sync_thread_blocks_md(app, &thread_id);
            if thread_id == app.active_thread_id {
                app.sync_anchors();
            } else {
                let anchors = app
                    .thread_transcripts
                    .get(&thread_id)
                    .map(|t| parse_anchors_for_transcript(t))
                    .unwrap_or_default();
                if let Some(thread) = app.threads.iter_mut().find(|t| t.id == thread_id) {
                    thread.anchors = anchors;
                }
                rebuild_tree_nodes(app);
            }
        }
        TuiEngineEvent::PermissionRequested {
            request_id,
            description,
        } => {
            app.push_permission(PendingPermission {
                request_id,
                description,
            });
        }
        TuiEngineEvent::ThreadStatusChanged { thread_id, status } => {
            if matches!(status.as_str(), "idle" | "error") {
                app.thread_active_tool.remove(&thread_id);
                app.thread_turn_started_ms.remove(&thread_id);
                app.thread_thoughts.remove(&thread_id);
            } else if matches!(status.as_str(), "streaming" | "executing" | "thinking") {
                app.thread_turn_started_ms
                    .entry(thread_id.clone())
                    .or_insert_with(now_epoch_ms);
            }
            app.thread_statuses.insert(thread_id, status);
        }
        TuiEngineEvent::PtyScreenUpdated { thread_id, snapshot } => {
            if snapshot.exited {
                app.pty_exit_codes
                    .insert(thread_id.clone(), snapshot.exit_code);
                let status = if snapshot.exit_code.is_some_and(|c| c != 0) {
                    "error"
                } else {
                    "idle"
                };
                app.thread_statuses
                    .insert(thread_id.clone(), status.to_string());
            } else {
                let was_exited = app.pty_exit_codes.remove(&thread_id).is_some()
                    || app.pty_screens.get(&thread_id).is_some_and(|s| s.exited);
                if was_exited {
                    app.thread_statuses
                        .insert(thread_id.clone(), "idle".to_string());
                }
            }
            app.pty_screens.insert(thread_id, snapshot);
        }
        TuiEngineEvent::PtyExited {
            thread_id,
            exit_code,
        } => {
            app.pty_exit_codes.insert(thread_id.clone(), exit_code);
            if let Some(snap) = app.pty_screens.get_mut(&thread_id) {
                snap.exited = true;
                snap.exit_code = exit_code;
            }
            let status = if exit_code.is_some_and(|c| c != 0) {
                "error"
            } else {
                "idle"
            };
            app.thread_statuses.insert(thread_id, status.to_string());
        }
        TuiEngineEvent::Error { message } => {
            let active = app.active_thread_id.clone();
            app.streaming_assistant_threads.remove(&active);
            app.thread_thoughts.remove(&active);
            app.thread_active_tool.remove(&active);
            app.thread_turn_started_ms.remove(&active);
            app.thread_statuses
                .insert(active.clone(), "error".to_string());
            with_thread_transcript_mut(app, &active, |buf| {
                ensure_paragraph_break(buf);
                buf.push_str(&format!("Error: {message}\n\n"));
            });
            app.thread_blocks
                .entry(active.clone())
                .or_default()
                .push_error(message);
            sync_thread_blocks_md(app, &active);
            app.sync_anchors();
        }
    }
}

enum RunnerLocalEvent {
    Engine(TuiEngineEvent),
    OptimisticThreadMeta(ThreadMeta),
    StatusNotice(String),
    OpenConfigPicker {
        snapshot: AgentOptionsSnapshot,
        config_id: String,
    },
    ResolveConfigQuery {
        snapshot: AgentOptionsSnapshot,
        config_id: String,
        query: String,
        set_default: bool,
    },
    ConfigApplied {
        snapshot: AgentOptionsSnapshot,
        config_id: String,
        value: String,
        set_default: bool,
    },
    CommandsFetched {
        snapshot: AgentOptionsSnapshot,
    },
    MergeFallback {
        thread_id: String,
        target_thread_id: String,
        summary: Option<String>,
    },
    MergeAborted(String),
    PtySnapshotLoaded {
        thread_id: String,
        snapshot: PtyScreenSnapshot,
    },
    WorktreeReady {
        thread_id: String,
        path: String,
    },
}

async fn poll_metadata_to_local(
    engine: &dyn TuiEnginePort,
    local_ev_tx: &mpsc::UnboundedSender<RunnerLocalEvent>,
) {
    match engine.thread_metadata().await {
        Ok(meta) => {
            if !meta.is_empty() {
                let _ = local_ev_tx.send(RunnerLocalEvent::Engine(
                    TuiEngineEvent::ThreadMetadataUpdated { meta },
                ));
            }
        }
        Err(err) => {
            let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                "Metadata sync failed: {err}"
            )));
        }
    }
}

fn dispatch_action(
    engine: Arc<dyn TuiEnginePort>,
    action: AppAction,
    local_ev_tx: mpsc::UnboundedSender<RunnerLocalEvent>,
    err_tx: mpsc::UnboundedSender<(Option<String>, String)>,
    pty_write_tx: &mpsc::UnboundedSender<(String, Vec<u8>)>,
) {
    match action {
        AppAction::SendPrompt { thread_id, prompt } => {
            tokio::spawn(async move {
                if let Err(err) = engine.send_turn(&thread_id, &prompt).await {
                    let _ = err_tx.send((Some(thread_id), err.to_string()));
                }
            });
        }
        AppAction::RunShell { thread_id, command } => {
            tokio::spawn(async move {
                if let Err(err) = engine.run_shell_command(&thread_id, &command).await {
                    let _ = err_tx.send((Some(thread_id), err.to_string()));
                }
            });
        }
        AppAction::ForkBranch {
            source_thread_id,
            title,
        } => {
            tokio::spawn(async move {
                match engine.fork_branch(&source_thread_id, title.as_deref()).await {
                    Ok(thread) => {
                        let optimistic = ThreadMeta {
                            thread_id: thread.id.clone(),
                            parent_thread_id: Some(source_thread_id),
                            inherited_turn_count: 0,
                            merged: false,
                            kind: grein_core::ThreadKind::Agent,
                            cwd: None,
                            git_branch: None,
                            git_dirty: 0,
                        };
                        let _ = local_ev_tx.send(RunnerLocalEvent::Engine(
                            TuiEngineEvent::ThreadCreated { thread },
                        ));
                        let _ =
                            local_ev_tx.send(RunnerLocalEvent::OptimisticThreadMeta(optimistic));
                        poll_metadata_to_local(engine.as_ref(), &local_ev_tx).await;
                    }
                    Err(err) => {
                        let _ = err_tx.send((Some(source_thread_id), err.to_string()));
                    }
                }
            });
        }
        AppAction::CreateRoot { title } => {
            tokio::spawn(async move {
                match engine.create_root(title.as_deref()).await {
                    Ok(thread) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::Engine(
                            TuiEngineEvent::ThreadCreated { thread },
                        ));
                        poll_metadata_to_local(engine.as_ref(), &local_ev_tx).await;
                    }
                    Err(err) => {
                        let _ = err_tx.send((None, err.to_string()));
                    }
                }
            });
        }
        AppAction::RenameThread { thread_id, title } => {
            tokio::spawn(async move {
                match engine.rename_thread(&thread_id, &title).await {
                    Ok(()) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::Engine(
                            TuiEngineEvent::ThreadTitleUpdated { thread_id, title },
                        ));
                    }
                    Err(err) => {
                        let _ = err_tx.send((Some(thread_id), err.to_string()));
                    }
                }
            });
        }
        AppAction::MergeBranch {
            source_thread_id,
            target_thread_id,
            summary,
        } => {
            tokio::spawn(async move {
                match engine
                    .merge_thread_with_summary(
                        &source_thread_id,
                        &target_thread_id,
                        summary.as_deref(),
                    )
                    .await
                {
                    Ok(()) => {
                        tokio::task::yield_now().await;
                        let _ = local_ev_tx.send(RunnerLocalEvent::MergeFallback {
                            thread_id: source_thread_id,
                            target_thread_id,
                            summary,
                        });
                        poll_metadata_to_local(engine.as_ref(), &local_ev_tx).await;
                    }
                    Err(err) => {
                        let _ = local_ev_tx
                            .send(RunnerLocalEvent::MergeAborted(source_thread_id.clone()));
                        let _ = err_tx.send((Some(source_thread_id), err.to_string()));
                    }
                }
            });
        }
        AppAction::DeleteThread(thread_id) => {
            tokio::spawn(async move {
                match engine.delete_thread(&thread_id).await {
                    Ok(next_active_thread_id) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::Engine(
                            TuiEngineEvent::ThreadDeleted {
                                thread_id,
                                next_active_thread_id,
                            },
                        ));
                        poll_metadata_to_local(engine.as_ref(), &local_ev_tx).await;
                    }
                    Err(err) => {
                        let _ = err_tx.send((Some(thread_id), err));
                    }
                }
            });
        }
        AppAction::CancelTurn { thread_id } => {
            tokio::spawn(async move {
                if let Err(err) = engine.cancel_turn(&thread_id).await {
                    let _ = err_tx.send((Some(thread_id), err.to_string()));
                }
            });
        }
        AppAction::ResolvePermission {
            request_id,
            approved,
        } => {
            tokio::spawn(async move {
                if let Err(err) = engine.resolve_permission(&request_id, approved).await {
                    let _ = err_tx.send((None, err.to_string()));
                }
            });
        }
        AppAction::OpenConfigPicker {
            thread_id,
            config_id,
        } => {
            tokio::spawn(async move {
                match engine.agent_options(&thread_id).await {
                    Ok(snapshot) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::OpenConfigPicker {
                            snapshot,
                            config_id,
                        });
                    }
                    Err(err) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                            "Failed to load options: {err}"
                        )));
                    }
                }
            });
        }
        AppAction::SetAgentConfig {
            thread_id,
            config_id,
            value,
            set_default,
        } => {
            tokio::spawn(async move {
                match engine
                    .set_agent_config(&thread_id, &config_id, &value, set_default)
                    .await
                {
                    Ok(snapshot) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::ConfigApplied {
                            snapshot,
                            config_id,
                            value,
                            set_default,
                        });
                    }
                    Err(err) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                            "Config update failed: {err}"
                        )));
                    }
                }
            });
        }
        AppAction::ApplyAgentConfigQuery {
            thread_id,
            config_id,
            query,
            set_default,
        } => {
            tokio::spawn(async move {
                match engine.agent_options(&thread_id).await {
                    Ok(snapshot) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::ResolveConfigQuery {
                            snapshot,
                            config_id,
                            query,
                            set_default,
                        });
                    }
                    Err(err) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                            "Failed to load options: {err}"
                        )));
                    }
                }
            });
        }
        AppAction::FetchAgentCommands { thread_id } => {
            tokio::spawn(async move {
                match engine.agent_options(&thread_id).await {
                    Ok(snapshot) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::CommandsFetched { snapshot });
                    }
                    Err(err) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                            "Failed to load commands: {err}"
                        )));
                    }
                }
            });
        }
        AppAction::CreateTerminal {
            parent_thread_id,
            title,
            cwd,
            rows,
            cols,
        } => {
            tokio::spawn(async move {
                match engine
                    .create_terminal(
                        parent_thread_id.as_deref(),
                        title.as_deref(),
                        cwd.as_deref(),
                        rows,
                        cols,
                    )
                    .await
                {
                    Ok(thread) => {
                        let optimistic = ThreadMeta {
                            thread_id: thread.id.clone(),
                            parent_thread_id,
                            inherited_turn_count: 0,
                            merged: false,
                            kind: grein_core::ThreadKind::Terminal,
                            cwd,
                            git_branch: None,
                            git_dirty: 0,
                        };
                        let _ =
                            local_ev_tx.send(RunnerLocalEvent::OptimisticThreadMeta(optimistic));
                        let _ = local_ev_tx.send(RunnerLocalEvent::Engine(
                            TuiEngineEvent::ThreadCreated { thread },
                        ));
                        poll_metadata_to_local(engine.as_ref(), &local_ev_tx).await;
                    }
                    Err(err) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                            "Create terminal failed: {err}"
                        )));
                    }
                }
            });
        }
        AppAction::PtyWrite { thread_id, bytes } => {
            let _ = pty_write_tx.send((thread_id, bytes));
        }
        AppAction::PtyResize {
            thread_id,
            rows,
            cols,
        } => {
            tokio::spawn(async move {
                if let Err(err) = engine.pty_resize(&thread_id, rows, cols).await {
                    let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                        "Terminal resize failed: {err}"
                    )));
                }
            });
        }
        AppAction::PtySnapshot {
            thread_id,
            rows,
            cols,
        } => {
            tokio::spawn(async move {
                match engine.pty_snapshot(&thread_id, rows, cols).await {
                    Ok(snapshot) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::PtySnapshotLoaded {
                            thread_id,
                            snapshot,
                        });
                    }
                    Err(err) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                            "Terminal snapshot failed: {err}"
                        )));
                    }
                }
            });
        }
        AppAction::PtyScroll { thread_id, delta } => {
            tokio::spawn(async move {
                match engine.pty_scroll(&thread_id, delta).await {
                    Ok(snapshot) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::PtySnapshotLoaded {
                            thread_id,
                            snapshot,
                        });
                    }
                    Err(err) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                            "Terminal scroll failed: {err}"
                        )));
                    }
                }
            });
        }
        AppAction::PtyRestart {
            thread_id,
            rows,
            cols,
        } => {
            tokio::spawn(async move {
                match engine.pty_restart(&thread_id, rows, cols).await {
                    Ok(snapshot) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::PtySnapshotLoaded {
                            thread_id,
                            snapshot,
                        });
                    }
                    Err(err) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                            "Terminal restart failed: {err}"
                        )));
                    }
                }
            });
        }
        AppAction::EnsureThreadWorktree { thread_id } => {
            tokio::spawn(async move {
                match engine.ensure_thread_worktree(&thread_id).await {
                    Ok(path) => {
                        let _ = local_ev_tx
                            .send(RunnerLocalEvent::WorktreeReady { thread_id, path });
                        poll_metadata_to_local(engine.as_ref(), &local_ev_tx).await;
                    }
                    Err(err) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                            "Worktree failed: {err}"
                        )));
                    }
                }
            });
        }
        AppAction::Scrolled { .. } | AppAction::None => {}
    }
}

fn drain_pending_actions(
    app: &mut App,
    engine: &Arc<dyn TuiEnginePort>,
    local_ev_tx: &mpsc::UnboundedSender<RunnerLocalEvent>,
    err_tx: &mpsc::UnboundedSender<(Option<String>, String)>,
    pty_write_tx: &mpsc::UnboundedSender<(String, Vec<u8>)>,
    pending_local_merges: &mut HashMap<String, usize>,
) {
    while let Some(action) = app.pending_actions.pop_front() {
        if let AppAction::MergeBranch {
            ref source_thread_id,
            ..
        } = action
        {
            *pending_local_merges
                .entry(source_thread_id.clone())
                .or_default() += 1;
        }
        dispatch_action(
            Arc::clone(engine),
            action,
            local_ev_tx.clone(),
            err_tx.clone(),
            pty_write_tx,
        );
    }
}

fn cache_snapshot_options(app: &mut App, snapshot: &AgentOptionsSnapshot) {
    if !snapshot.config_options.is_empty() {
        apply_engine_event(
            app,
            TuiEngineEvent::AgentConfigUpdated {
                thread_id: snapshot.thread_id.clone(),
                config_options: snapshot.config_options.clone(),
            },
        );
    }
    if !snapshot.available_commands.is_empty() {
        apply_engine_event(
            app,
            TuiEngineEvent::AgentCommandsUpdated {
                thread_id: snapshot.thread_id.clone(),
                commands: snapshot.available_commands.clone(),
            },
        );
    }
}

fn handle_streamed_engine_event(
    app: &mut App,
    event: TuiEngineEvent,
    pending_local_merges: &mut HashMap<String, usize>,
) {
    if let TuiEngineEvent::ThreadMerged { ref thread_id, .. } = event {
        if let Some(cnt) = pending_local_merges.get_mut(thread_id) {
            if *cnt > 0 {
                *cnt -= 1;
            }
        }
    }
    apply_engine_event(app, event);
}

fn spawn_key_reader(
    stop: Arc<AtomicBool>,
    tx: mpsc::UnboundedSender<Event>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            match event::poll(Duration::from_millis(25)) {
                Ok(true) => match event::read() {
                    Ok(ev) => {
                        if tx.send(ev).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                },
                Ok(false) => {}
                Err(_) => break,
            }
        }
    })
}

pub async fn run_with_engine<B: Backend>(
    terminal: &mut Terminal<B>,
    app: App,
    engine: Arc<dyn TuiEnginePort>,
) -> io::Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    let (key_tx, key_rx) = mpsc::unbounded_channel::<Event>();
    let reader = spawn_key_reader(Arc::clone(&stop), key_tx);
    let result = run_with_engine_events(terminal, app, engine, key_rx).await;
    stop.store(true, Ordering::Relaxed);
    let _ = reader.join();
    result
}

pub async fn run_with_engine_events<B: Backend>(
    terminal: &mut Terminal<B>,
    mut app: App,
    engine: Arc<dyn TuiEnginePort>,
    mut key_rx: mpsc::UnboundedReceiver<Event>,
) -> io::Result<()> {
    let mut engine_events = engine.subscribe_events().await?;
    let session = engine.hydrate_session().await?;
    apply_hydrated_session(&mut app, &session);
    match engine.thread_metadata().await {
        Ok(meta) => {
            if !meta.is_empty() {
                apply_engine_event(&mut app, TuiEngineEvent::ThreadMetadataUpdated { meta });
            }
        }
        Err(err) => {
            app.status_notice = Some(format!("Metadata sync failed: {err}"));
        }
    }

    let (local_ev_tx, mut local_ev_rx) = mpsc::unbounded_channel::<RunnerLocalEvent>();
    let (err_tx, mut err_rx) = mpsc::unbounded_channel::<(Option<String>, String)>();
    let (pty_write_tx, mut pty_write_rx) = mpsc::unbounded_channel::<(String, Vec<u8>)>();
    let pty_engine = Arc::clone(&engine);
    let pty_local_ev_tx = local_ev_tx.clone();
    let pty_writer_handle = tokio::spawn(async move {
        while let Some((thread_id, bytes)) = pty_write_rx.recv().await {
            if let Err(err) = pty_engine.pty_write(&thread_id, bytes).await {
                let _ = pty_local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                    "Terminal input failed: {err}"
                )));
            }
        }
    });
    let mut pending_local_merges: HashMap<String, usize> = HashMap::new();
    while let Ok(event) = engine_events.try_recv() {
        handle_streamed_engine_event(&mut app, event, &mut pending_local_merges);
    }
    let mut tick = tokio::time::interval(Duration::from_millis(50));
    drain_pending_actions(
        &mut app,
        &engine,
        &local_ev_tx,
        &err_tx,
        &pty_write_tx,
        &mut pending_local_merges,
    );

    loop {
        draw_frame_mut(terminal, &mut app)?;
        drain_pending_actions(
            &mut app,
            &engine,
            &local_ev_tx,
            &err_tx,
            &pty_write_tx,
            &mut pending_local_merges,
        );
        if app.should_quit() {
            break;
        }

        tokio::select! {
            ev_opt = key_rx.recv() => {
                let Some(ev) = ev_opt else {
                    break;
                };
                handle_event(&mut app, ev)?;
                drain_pending_actions(
                    &mut app,
                    &engine,
                    &local_ev_tx,
                    &err_tx,
                    &pty_write_tx,
                    &mut pending_local_merges,
                );
            }
            Some(event) = engine_events.recv() => {
                handle_streamed_engine_event(&mut app, event, &mut pending_local_merges);
            }
            Some(local_event) = local_ev_rx.recv() => {
                match local_event {
                    RunnerLocalEvent::Engine(event) => apply_engine_event(&mut app, event),
                    RunnerLocalEvent::OptimisticThreadMeta(meta) => {
                        if !app.thread_meta.contains_key(&meta.thread_id) {
                            app.thread_meta.insert(meta.thread_id.clone(), meta);
                            let active = app.active_thread_id.clone();
                            app.unfold_ancestors_for(&active);
                            rebuild_tree_nodes(&mut app);
                            app.sync_focus_for_active_thread();
                            app.maybe_request_pty_snapshot(&active);
                        }
                    }
                    RunnerLocalEvent::StatusNotice(notice) => {
                        app.status_notice = Some(notice);
                    }
                    RunnerLocalEvent::PtySnapshotLoaded { thread_id, snapshot } => {
                        apply_engine_event(
                            &mut app,
                            TuiEngineEvent::PtyScreenUpdated { thread_id, snapshot },
                        );
                    }
                    RunnerLocalEvent::WorktreeReady { thread_id, path } => {
                        if let Some(meta) = app.thread_meta.get_mut(&thread_id) {
                            meta.cwd = Some(path.clone());
                        }
                        app.status_notice = Some(format!("Worktree ready: {path}"));
                    }
                    RunnerLocalEvent::OpenConfigPicker {
                        snapshot,
                        config_id,
                    } => {
                        cache_snapshot_options(&mut app, &snapshot);
                        app.open_config_picker_from_options(&snapshot.thread_id, &config_id);
                    }
                    RunnerLocalEvent::ResolveConfigQuery {
                        snapshot,
                        config_id,
                        query,
                        set_default,
                    } => {
                        cache_snapshot_options(&mut app, &snapshot);
                        let tid = snapshot.thread_id.clone();
                        if let Some(opt) = app
                            .thread_config_options
                            .get(&tid)
                            .and_then(|opts| find_config_option(opts, &config_id))
                            .cloned()
                        {
                            if opt.options.is_empty() {
                                app.pending_actions.push_back(AppAction::SetAgentConfig {
                                    thread_id: tid,
                                    config_id: opt.id,
                                    value: query,
                                    set_default,
                                });
                            } else if let Some(choice) = match_config_choice(&opt.options, &query) {
                                app.pending_actions.push_back(AppAction::SetAgentConfig {
                                    thread_id: tid,
                                    config_id: opt.id,
                                    value: choice.value.clone(),
                                    set_default,
                                });
                            } else {
                                app.status_notice =
                                    Some(format!("No {} option matching '{query}'", opt.name));
                            }
                        } else {
                            app.status_notice =
                                Some(format!("Option '{config_id}' not exposed by agent"));
                        }
                    }
                    RunnerLocalEvent::ConfigApplied {
                        snapshot,
                        config_id,
                        value,
                        set_default,
                    } => {
                        cache_snapshot_options(&mut app, &snapshot);
                        let tid = if snapshot.thread_id.is_empty() {
                            app.active_thread_id.clone()
                        } else {
                            snapshot.thread_id
                        };
                        if snapshot.config_options.is_empty() {
                            if let Some(opts) = app.thread_config_options.get_mut(&tid) {
                                if let Some(opt) = opts.iter_mut().find(|o| o.id == config_id) {
                                    opt.current_value = value.clone();
                                }
                                let cloned = opts.clone();
                                app.sync_agent_meta_from_config_options(&tid, &cloned);
                            }
                        }
                        let scope = if set_default { "default" } else { "branch" };
                        app.status_notice = Some(format!("Set {config_id} = {value} ({scope})"));
                    }
                    RunnerLocalEvent::CommandsFetched { snapshot } => {
                        cache_snapshot_options(&mut app, &snapshot);
                        let tid = snapshot.thread_id.clone();
                        app.record_agent_commands_note(&tid, &snapshot.available_commands);
                    }
                    RunnerLocalEvent::MergeFallback {
                        thread_id,
                        target_thread_id,
                        summary,
                    } => {
                        while let Ok(ev) = engine_events.try_recv() {
                            handle_streamed_engine_event(&mut app, ev, &mut pending_local_merges);
                        }
                        if let Some(cnt) = pending_local_merges.get_mut(&thread_id) {
                            if *cnt > 0 {
                                *cnt -= 1;
                                apply_engine_event(
                                    &mut app,
                                    TuiEngineEvent::ThreadMerged {
                                        thread_id,
                                        target_thread_id,
                                        summary: summary.unwrap_or_default(),
                                    },
                                );
                            }
                        }
                    }
                    RunnerLocalEvent::MergeAborted(thread_id) => {
                        if let Some(cnt) = pending_local_merges.get_mut(&thread_id) {
                            if *cnt > 0 {
                                *cnt -= 1;
                            }
                        }
                    }
                }
                drain_pending_actions(
                    &mut app,
                    &engine,
                    &local_ev_tx,
                    &err_tx,
                    &pty_write_tx,
                    &mut pending_local_merges,
                );
            }
            Some((thread_id_opt, message)) = err_rx.recv() => {
                let target_thread = thread_id_opt.unwrap_or_else(|| app.active_thread_id.clone());
                app.streaming_assistant_threads.remove(&target_thread);
                app.thread_thoughts.remove(&target_thread);
                app.thread_active_tool.remove(&target_thread);
                app.thread_turn_started_ms.remove(&target_thread);
                app.thread_statuses
                    .insert(target_thread.clone(), "error".to_string());
                with_thread_transcript_mut(&mut app, &target_thread, |buf| {
                    ensure_paragraph_break(buf);
                    buf.push_str(&format!("Error: {message}\n\n"));
                });
                app.thread_blocks
                    .entry(target_thread.clone())
                    .or_default()
                    .push_error(message);
                sync_thread_blocks_md(&mut app, &target_thread);
                if target_thread == app.active_thread_id {
                    app.sync_anchors();
                }
            }
            _ = tick.tick() => {
                app.tick_count = app.tick_count.wrapping_add(1);
            }
        }
    }

    drop(pty_write_tx);
    let _ = pty_writer_handle.await;

    Ok(())
}
