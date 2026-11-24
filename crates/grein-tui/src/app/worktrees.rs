use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::engine_port::WorktreeEntry;

use super::types::{
    App, AppAction, FocusedPane, PendingWorktreeDelete, SidebarView, WorktreeAutocompleteItem,
    WorktreeRow,
};

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

    fn sync_active_thread_from_grein_selection(&mut self) {
        if self.sidebar_view != SidebarView::Grein {
            return;
        }
        let (sel_id, _) = self.selected_tree_thread_info();
        if !sel_id.is_empty()
            && sel_id != self.active_thread_id
            && self.threads.iter().any(|t| t.id == sel_id)
        {
            self.switch_thread(&sel_id);
        }
    }

    pub fn toggle_worktrees_sidebar_view(&mut self) {
        self.pending_rename_thread_id = None;
        self.rename_prompt = None;
        self.pending_g_prefix = false;
        self.worktree_filter_active = false;
        if self.sidebar_hidden {
            self.sync_active_thread_from_grein_selection();
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
                self.sync_active_thread_from_grein_selection();
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
        self.rename_prompt = None;
        self.pending_g_prefix = false;
        self.sync_active_thread_from_grein_selection();
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

    pub(super) fn handle_key_worktree_autocomplete(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let candidates = self.worktree_autocomplete_candidates();
        if candidates.is_empty() {
            return false;
        }
        let idx = self.worktree_autocomplete_idx.min(candidates.len() - 1);

        match key.code {
            KeyCode::Down if !ctrl && !alt && !shift => {
                self.composer_desired_col = None;
                self.worktree_autocomplete_idx = (idx + 1) % candidates.len();
                true
            }
            KeyCode::Up if !ctrl && !alt && !shift => {
                self.composer_desired_col = None;
                self.worktree_autocomplete_idx = (idx + candidates.len() - 1) % candidates.len();
                true
            }
            KeyCode::BackTab if !ctrl && !alt => {
                self.composer_desired_col = None;
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

    pub(super) fn handle_key_worktrees_tree(&mut self, key: KeyEvent, had_g_prefix: bool) {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let no_ctrl_alt = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        if !no_ctrl_alt {
            return;
        }

        let rows = self.visible_worktree_rows();
        let selected_row = rows.get(self.worktree_selected_idx).cloned();

        match (key.code, shift) {
            (KeyCode::Down, false) | (KeyCode::Char('j'), _) => {
                if !rows.is_empty() && self.worktree_selected_idx + 1 < rows.len() {
                    self.worktree_selected_idx += 1;
                    self.maybe_fetch_selected_worktree_diff(false);
                }
            }
            (KeyCode::Up, false) | (KeyCode::Char('k'), _) => {
                self.worktree_selected_idx = self.worktree_selected_idx.saturating_sub(1);
                self.maybe_fetch_selected_worktree_diff(false);
            }
            (KeyCode::Char('g'), _) => {
                if had_g_prefix {
                    self.worktree_selected_idx = 0;
                    self.pending_g_prefix = false;
                    self.maybe_fetch_selected_worktree_diff(false);
                } else {
                    self.pending_g_prefix = true;
                }
            }
            (KeyCode::Char('G'), _) => {
                if !rows.is_empty() {
                    self.worktree_selected_idx = rows.len() - 1;
                    self.maybe_fetch_selected_worktree_diff(false);
                }
            }
            (KeyCode::Char(' '), _) => {
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
            (KeyCode::Left, false) | (KeyCode::Char('h'), _) => {
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
            (KeyCode::Char('H'), _) => {
                for wt in &self.worktrees {
                    self.collapsed_worktree_repos.insert(wt.repo_root.clone());
                }
                self.clamp_worktree_selection();
                self.maybe_fetch_selected_worktree_diff(false);
            }
            (KeyCode::Char('L'), _) => {
                self.collapsed_worktree_repos.clear();
                self.clamp_worktree_selection();
                self.maybe_fetch_selected_worktree_diff(false);
            }
            (KeyCode::Right, false) | (KeyCode::Char('l'), _) => {
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
                    Some(WorktreeRow::Entry(_)) | None => {}
                }
                self.maybe_fetch_selected_worktree_diff(false);
            }
            (KeyCode::Char('r'), _) => {
                self.status_notice = Some("Refreshing worktrees...".to_string());
                self.worktree_list_in_flight = true;
                self.pending_actions.push_back(AppAction::ListWorktrees);
                self.maybe_fetch_selected_worktree_diff(true);
            }
            (KeyCode::Char('p'), _) => {
                self.request_prune_worktrees();
            }
            (KeyCode::Char('y'), _) => {
                if let Some(WorktreeRow::Entry(entry)) = selected_row {
                    self.finish_copy_to_clipboard("worktree path", entry.path);
                }
            }
            (KeyCode::Enter, _) => match selected_row {
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
                    self.sidebar_view = SidebarView::Grein;
                    self.focused_pane = if self.active_is_terminal() {
                        FocusedPane::Terminal
                    } else {
                        FocusedPane::Composer
                    };
                }
                None => {}
            },
            (KeyCode::Char('n'), _) => {
                if let Some(WorktreeRow::Entry(entry)) = selected_row {
                    self.status_notice = None;
                    self.sidebar_view = SidebarView::Grein;
                    self.focused_pane = FocusedPane::Composer;
                    self.pending_actions.push_back(AppAction::CreateRoot {
                        title: None,
                        cwd: Some(entry.path),
                    });
                }
            }
            (KeyCode::Char('s'), _) => {
                if let Some(WorktreeRow::Entry(entry)) = selected_row {
                    self.dispatch_create_terminal(
                        Some(self.active_thread_id.clone()),
                        Some(entry.path),
                    );
                    self.sidebar_view = SidebarView::Grein;
                }
            }
            (KeyCode::Char('w'), _) => {
                let thread_id = self.active_thread_id.clone();
                self.status_notice = Some("Allocating worktree...".to_string());
                self.pending_actions
                    .push_back(AppAction::EnsureThreadWorktree { thread_id });
            }
            (KeyCode::Char('d'), _) => {
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
            (KeyCode::Char('z'), _) => {
                self.toggle_sidebar_zoom();
            }
            _ => {}
        }
    }
}
