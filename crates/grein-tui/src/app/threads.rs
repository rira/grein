use crate::engine_port::ThreadMeta;

use super::types::{App, AppAction, ThreadDisplayParts, ThreadItem};

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

pub fn normalize_custom_name(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn is_numbered_placeholder(title: &str, prefix: &str) -> bool {
    title
        .strip_prefix(prefix)
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

pub fn is_generic_agent_placeholder(title: &str) -> bool {
    let t = title.trim();
    t.is_empty()
        || t == "Main"
        || t == "Branch"
        || t == "Root"
        || t == "Untitled"
        || is_numbered_placeholder(t, "Branch ")
        || is_numbered_placeholder(t, "Root ")
}

fn looks_like_uuid(s: &str) -> bool {
    let t = s.trim();
    if t.len() < 8 {
        return false;
    }
    let hex_digits = t.bytes().filter(|b| b.is_ascii_hexdigit()).count();
    t.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
        && hex_digits >= 8
        && (t.len() >= 16 || (t.contains('-') && t.len() >= 12))
}

fn format_worktree_id(slug: &str) -> String {
    let clean = slug.trim();
    let stripped = clean.strip_prefix("worktree-").unwrap_or(clean);
    if looks_like_uuid(stripped) {
        stripped.chars().take(8).collect()
    } else if looks_like_uuid(clean) {
        clean.chars().take(8).collect()
    } else if stripped.is_empty() {
        clean.to_string()
    } else {
        stripped.to_string()
    }
}

fn extract_worktree_cwd_parts(trimmed: &str) -> Option<(String, &str)> {
    let marker = if let Some(idx) = trimmed.find("/.grein/worktrees/") {
        Some(&trimmed[idx + "/.grein/worktrees/".len()..])
    } else {
        trimmed.strip_prefix(".grein/worktrees/")
    };
    if let Some(after) = marker {
        let wt_part = after.trim_matches('/');
        if !wt_part.is_empty() {
            let (slug, sub) = match wt_part.split_once('/') {
                Some((s, rest)) => (s, rest.trim_matches('/')),
                None => (wt_part, ""),
            };
            if !slug.is_empty() {
                return Some((format_worktree_id(slug), sub));
            }
        }
    }
    let clean = trimmed.trim_end_matches('/');
    if let Some(leaf) = std::path::Path::new(clean)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
    {
        if leaf.starts_with("worktree-") || looks_like_uuid(leaf) {
            return Some((format_worktree_id(leaf), ""));
        }
    }
    None
}

fn abbreviate_home_path(trimmed: &str, home_dir: Option<&str>) -> String {
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

fn middle_elide_path(path: &str, max_chars: usize) -> String {
    if path.chars().count() <= max_chars {
        return path.to_string();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        let segs: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
        if segs.len() >= 2 {
            let leaf = segs[segs.len() - 1];
            let candidate = format!("~/.../{leaf}");
            if candidate.chars().count() <= max_chars {
                return candidate;
            }
            let keep_leaf = max_chars.saturating_sub(6).max(3);
            let tail: String = leaf
                .chars()
                .rev()
                .take(keep_leaf)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            return format!("~/.../{tail}");
        }
    } else if path.starts_with('/') {
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if segs.len() >= 3 {
            let leaf = segs[segs.len() - 1];
            let candidate = format!("/{}/.../{leaf}", segs[0]);
            if candidate.chars().count() <= max_chars {
                return candidate;
            }
            let short_candidate = format!("/.../{leaf}");
            if short_candidate.chars().count() <= max_chars {
                return short_candidate;
            }
        }
    }
    let keep_head = max_chars.saturating_sub(4).min(10);
    let keep_tail = max_chars.saturating_sub(keep_head + 3);
    let head: String = path.chars().take(keep_head).collect();
    let tail: String = path
        .chars()
        .rev()
        .take(keep_tail)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{head}...{tail}")
}

pub fn format_terminal_sidebar_cwd_with_home(
    cwd: &str,
    home_dir: Option<&str>,
    elide: bool,
) -> String {
    let trimmed = cwd.trim();
    if trimmed.is_empty() {
        return "Terminal".to_string();
    }
    if let Some((wt_id, sub)) = extract_worktree_cwd_parts(trimmed) {
        if sub.is_empty() {
            return format!("worktree: {wt_id}");
        }
        let full = format!("worktree: {wt_id}/{sub}");
        if !elide || full.chars().count() <= 24 {
            return full;
        }
        let leaf = sub
            .rsplit('/')
            .find(|s| !s.is_empty())
            .unwrap_or(sub);
        let compact = format!("worktree: {wt_id}/.../{leaf}");
        if compact.chars().count() <= 28 {
            return compact;
        }
        return format!("worktree: {wt_id}/{leaf}");
    }
    let abbrev = abbreviate_home_path(trimmed, home_dir);
    if elide {
        middle_elide_path(&abbrev, 24)
    } else {
        abbrev
    }
}

pub fn thread_display_parts_with_home(
    thread: &ThreadItem,
    meta: Option<&ThreadMeta>,
    home_dir: Option<&str>,
    elide_sidebar: bool,
) -> ThreadDisplayParts {
    let custom_name = normalize_custom_name(
        thread
            .custom_name
            .as_deref()
            .or_else(|| meta.and_then(|m| m.custom_name.as_deref())),
    );
    let is_terminal = meta.is_some_and(|m| m.kind == grein_core::ThreadKind::Terminal);
    let auto_label = if is_terminal {
        match meta
            .and_then(|m| m.cwd.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(cwd) => format_terminal_sidebar_cwd_with_home(cwd, home_dir, elide_sidebar),
            None => "Terminal".to_string(),
        }
    } else {
        let trimmed = thread.title.trim();
        if trimmed.is_empty() {
            "Untitled".to_string()
        } else {
            trimmed.to_string()
        }
    };
    ThreadDisplayParts {
        custom_name,
        auto_label,
    }
}

pub fn thread_display_parts(thread: &ThreadItem, meta: Option<&ThreadMeta>) -> ThreadDisplayParts {
    let home = std::env::var("HOME").ok();
    thread_display_parts_with_home(thread, meta, home.as_deref(), true)
}

pub fn display_thread_title(thread: &ThreadItem, meta: Option<&ThreadMeta>) -> String {
    let home = std::env::var("HOME").ok();
    let is_terminal = meta.is_some_and(|m| m.kind == grein_core::ThreadKind::Terminal);
    thread_display_parts_with_home(thread, meta, home.as_deref(), true).combined(is_terminal)
}

pub fn shorten_terminal_cwd_with_home(cwd: &str, home_dir: Option<&str>) -> String {
    let trimmed = cwd.trim();
    if let Some((wt_id, sub)) = extract_worktree_cwd_parts(trimmed) {
        if sub.is_empty() {
            return format!("⎇ {wt_id}");
        }
        return format!("⎇ {wt_id}/{sub}");
    }
    abbreviate_home_path(trimmed, home_dir)
}

pub fn shorten_terminal_cwd(cwd: &str) -> String {
    let home = std::env::var("HOME").ok();
    shorten_terminal_cwd_with_home(cwd, home.as_deref())
}

impl App {
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

    pub(super) fn find_existing_terminal_for(&self, target_id: &str) -> Option<String> {
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

    pub fn switch_thread(&mut self, thread_id: impl Into<String>) {
        let target_thread = thread_id.into();
        self.pending_rename_thread_id = None;
        self.rename_prompt = None;
        self.composer_desired_col = None;
        self.pending_g_prefix = false;
        self.clear_pane_selection();
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
        self.composer_completion_dismissed = false;
        self.sync_worktree_autocomplete(true);
        self.sync_composer_completion();
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

    fn thread_title_or_empty(&self, thread_id: &str) -> String {
        self.threads
            .iter()
            .find(|t| t.id == thread_id)
            .map(|t| t.title.clone())
            .unwrap_or_default()
    }

    pub(super) fn selected_tree_thread_info(&self) -> (String, String) {
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

    pub fn active_thread_is_busy(&self) -> bool {
        matches!(
            self.thread_statuses
                .get(&self.active_thread_id)
                .map(String::as_str),
            Some("streaming" | "executing" | "thinking")
        )
    }

    pub fn selected_or_active_thread_id(&self) -> String {
        if self.focused_pane == super::types::FocusedPane::Tree
            && self.sidebar_view == super::types::SidebarView::Grein
        {
            let sel = self.selected_tree_thread_info().0;
            if !sel.is_empty() {
                return sel;
            }
        }
        self.active_thread_id.clone()
    }

    pub fn display_parts_for_elided(
        &self,
        thread_id: &str,
        fallback_title: &str,
        elide_sidebar: bool,
    ) -> ThreadDisplayParts {
        let home = std::env::var("HOME").ok();
        self.display_parts_for_with_home(thread_id, fallback_title, home.as_deref(), elide_sidebar)
    }

    pub fn display_parts_for_with_home(
        &self,
        thread_id: &str,
        fallback_title: &str,
        home_dir: Option<&str>,
        elide_sidebar: bool,
    ) -> ThreadDisplayParts {
        let item = self
            .threads
            .iter()
            .find(|t| t.id == thread_id)
            .cloned()
            .unwrap_or_else(|| ThreadItem {
                id: thread_id.to_string(),
                title: fallback_title.to_string(),
                custom_name: None,
                anchors: Vec::new(),
            });
        let pty_cwd = self.pty_screens.get(thread_id).and_then(|s| s.cwd.clone());
        let meta = self.thread_meta.get(thread_id);
        match (meta, pty_cwd) {
            (Some(m), Some(cwd)) if m.cwd.as_deref().is_none_or(|c| c.trim().is_empty()) => {
                let mut patched = m.clone();
                patched.cwd = Some(cwd);
                thread_display_parts_with_home(&item, Some(&patched), home_dir, elide_sidebar)
            }
            (Some(m), _) => {
                thread_display_parts_with_home(&item, Some(m), home_dir, elide_sidebar)
            }
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
                    custom_name: None,
                };
                thread_display_parts_with_home(&item, Some(&synth), home_dir, elide_sidebar)
            }
            (None, _) => thread_display_parts_with_home(&item, None, home_dir, elide_sidebar),
        }
    }

    pub fn display_title_for(&self, thread_id: &str, fallback_title: &str) -> String {
        let parts = self.display_parts_for_elided(thread_id, fallback_title, true);
        let is_terminal = self.is_terminal_thread(thread_id);
        parts.combined(is_terminal)
    }
}
