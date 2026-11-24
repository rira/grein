use std::collections::HashSet;

use crate::runner::{compute_ordered_thread_entries, OrderedThreadEntry};

use super::types::{App, FocusedPane, SidebarView, ThreadItem, TreeAction};

fn step_wrapped(pos: usize, len: usize, forward: bool) -> usize {
    if forward {
        (pos + 1) % len
    } else {
        (pos + len - 1) % len
    }
}

impl App {
    pub(super) fn focus_tree(&mut self) {
        self.pending_rename_thread_id = None;
        self.rename_prompt = None;
        self.composer_desired_col = None;
        self.pending_g_prefix = false;
        self.focused_pane = FocusedPane::Tree;
        self.sidebar_hidden = false;
        self.sync_focus_out();
        self.maybe_queue_terminal_resize();
    }

    pub(super) fn open_or_jump_to_terminal(&mut self) {
        self.pending_rename_thread_id = None;
        self.rename_prompt = None;
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

    pub fn unfold_ancestors_for(&mut self, thread_id: &str) -> bool {
        let mut changed = false;
        let mut visited = HashSet::new();
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

    pub fn expand_or_step_child_in_tree(&mut self) -> bool {
        let Some(curr) = self.tree.selected_index() else {
            return false;
        };
        let sel_id = match self.tree.nodes.get(curr) {
            Some(crate::ui::tree::TreeNode::Thread(t)) => t.id.clone(),
            _ => return false,
        };
        if self.collapsed_threads.remove(&sel_id) {
            crate::runner::rebuild_tree_nodes(self);
            return true;
        }
        if let Some(crate::ui::tree::TreeNode::Thread(next_t)) = self.tree.nodes.get(curr + 1) {
            if self
                .thread_meta
                .get(&next_t.id)
                .and_then(|m| m.parent_thread_id.as_deref())
                == Some(sel_id.as_str())
            {
                let next_id = next_t.id.clone();
                self.tree.select_index(curr + 1);
                self.maybe_request_pty_snapshot(&next_id);
                return true;
            }
        }
        false
    }

    pub fn collapse_all_in_tree(&mut self) {
        let (sel_id, _) = self.selected_tree_thread_info();
        let mut cur = sel_id;
        let mut visited = HashSet::new();
        while visited.insert(cur.clone()) {
            if let Some(parent_id) = self
                .thread_meta
                .get(&cur)
                .and_then(|m| m.parent_thread_id.as_deref())
                .filter(|pid| *pid != cur && self.threads.iter().any(|t| t.id == *pid))
            {
                cur = parent_id.to_string();
            } else {
                break;
            }
        }
        let parent_ids: Vec<String> = self
            .threads
            .iter()
            .filter(|t| self.thread_has_children(&t.id))
            .map(|t| t.id.clone())
            .collect();
        for id in parent_ids {
            self.collapsed_threads.insert(id);
        }
        crate::runner::rebuild_tree_nodes(self);
        if let Some(idx) = self.tree.nodes.iter().position(|n| {
            matches!(n, crate::ui::tree::TreeNode::Thread(t) if t.id == cur)
        }) {
            self.tree.select_index(idx);
            self.maybe_request_pty_snapshot(&cur);
        }
    }

    pub fn expand_all_in_tree(&mut self) {
        let (sel_id, _) = self.selected_tree_thread_info();
        self.collapsed_threads.clear();
        crate::runner::rebuild_tree_nodes(self);
        if let Some(idx) = self.tree.nodes.iter().position(|n| {
            matches!(n, crate::ui::tree::TreeNode::Thread(t) if t.id == sel_id)
        }) {
            self.tree.select_index(idx);
            self.maybe_request_pty_snapshot(&sel_id);
        }
    }

    pub(super) fn sync_focus_in(&mut self) {
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

    pub(super) fn sync_focus_out(&mut self) {
        self.focus = self.focused_pane;
        self.active_focus = self.focused_pane;
        if self.focused_pane != FocusedPane::Tree {
            self.rename_prompt = None;
        }
        self.sync_worktree_autocomplete(false);
        if !self.composer_autocomplete_enabled
            || self.composer_completion_dismissed
            || self.worktree_autocomplete_open
            || self.focused_pane != FocusedPane::Composer
            || self.has_active_modal()
        {
            self.composer_completion = None;
        }
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
            self.rename_prompt = None;
            self.focused_pane = pane;
            self.sync_focus_out();
        }
    }

    pub fn set_focused_pane(&mut self, pane: impl Into<FocusedPane>) {
        self.set_focus(pane);
    }

    pub(super) fn activate_selected_tree_node(&mut self) -> Option<TreeAction> {
        self.pending_rename_thread_id = None;
        self.rename_prompt = None;
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

    pub(super) fn toggle_composer_tree_focus(&mut self) {
        self.rename_prompt = None;
        if matches!(
            self.focused_pane,
            FocusedPane::Composer | FocusedPane::Terminal
        ) {
            self.focus_tree();
        } else if self.focused_pane == FocusedPane::Tree {
            if self.sidebar_view == SidebarView::Worktrees {
                self.worktree_filter_active = false;
                self.sidebar_view = SidebarView::Grein;
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

    fn effective_parent_id<'a>(
        &'a self,
        thread_id: &str,
        valid_ids: &HashSet<&str>,
    ) -> Option<&'a str> {
        self.thread_meta
            .get(thread_id)
            .and_then(|m| m.parent_thread_id.as_deref())
            .filter(|pid| *pid != thread_id && valid_ids.contains(pid))
    }

    fn next_sibling_thread_id(&self, forward: bool) -> Option<String> {
        let ordered = compute_ordered_thread_entries(&self.threads, &self.thread_meta);
        if ordered.len() <= 1 {
            return None;
        }
        let valid_ids: HashSet<&str> = self.threads.iter().map(|t| t.id.as_str()).collect();
        if !valid_ids.contains(self.active_thread_id.as_str()) {
            return None;
        }
        let active_parent = self.effective_parent_id(&self.active_thread_id, &valid_ids);
        let siblings: Vec<String> = ordered
            .iter()
            .map(|e| self.threads[e.index].id.clone())
            .filter(|id| self.effective_parent_id(id, &valid_ids) == active_parent)
            .collect();
        if siblings.len() <= 1 {
            return None;
        }
        let pos = siblings.iter().position(|id| id == &self.active_thread_id)?;
        let next_pos = step_wrapped(pos, siblings.len(), forward);
        Some(siblings[next_pos].clone())
    }

    pub fn cycle_sibling_thread(&mut self, forward: bool) -> bool {
        let Some(target_id) = self.next_sibling_thread_id(forward) else {
            return false;
        };
        self.switch_thread(target_id);
        true
    }

    pub fn step_sibling_at_current_depth(&mut self, forward: bool) -> bool {
        let Some(target_id) = self.next_sibling_thread_id(forward) else {
            return false;
        };
        self.sidebar_view = SidebarView::Grein;
        self.switch_thread(target_id);
        true
    }

    pub fn ascend_to_parent_thread(&mut self) -> bool {
        let valid_ids: HashSet<&str> = self.threads.iter().map(|t| t.id.as_str()).collect();
        if !valid_ids.contains(self.active_thread_id.as_str()) {
            return false;
        }
        let Some(parent_id) = self
            .effective_parent_id(&self.active_thread_id, &valid_ids)
            .map(str::to_string)
        else {
            return false;
        };
        self.last_visited_child
            .insert(parent_id.clone(), self.active_thread_id.clone());
        self.sidebar_view = SidebarView::Grein;
        self.switch_thread(parent_id);
        true
    }

    pub fn descend_to_child_thread(&mut self) -> bool {
        let valid_ids: HashSet<&str> = self.threads.iter().map(|t| t.id.as_str()).collect();
        if !valid_ids.contains(self.active_thread_id.as_str()) {
            return false;
        }
        let ordered = compute_ordered_thread_entries(&self.threads, &self.thread_meta);
        let children: Vec<String> = ordered
            .iter()
            .map(|e| self.threads[e.index].id.clone())
            .filter(|id| {
                self.effective_parent_id(id, &valid_ids) == Some(self.active_thread_id.as_str())
            })
            .collect();
        if children.is_empty() {
            return false;
        }
        let target_id = self
            .last_visited_child
            .get(&self.active_thread_id)
            .filter(|last| children.iter().any(|c| c == *last))
            .cloned()
            .unwrap_or_else(|| children[0].clone());
        self.sidebar_view = SidebarView::Grein;
        self.switch_thread(target_id);
        true
    }

    pub fn select_root_by_index(&mut self, one_based: usize) -> bool {
        if !(1..=9).contains(&one_based) {
            return false;
        }
        let ordered = compute_ordered_thread_entries(&self.threads, &self.thread_meta);
        let target_id = ordered
            .into_iter()
            .filter(|e| e.depth == 0)
            .nth(one_based - 1)
            .map(|e| self.threads[e.index].id.clone());
        let Some(id) = target_id else {
            return false;
        };
        if self.focused_pane == FocusedPane::Tree && self.sidebar_view == SidebarView::Worktrees {
            self.sidebar_view = SidebarView::Grein;
        }
        self.switch_thread(id);
        true
    }

    pub(super) fn active_pty_is_alt_screen(&self) -> bool {
        self.focused_pane == FocusedPane::Terminal
            && !self.is_terminal_exited(&self.active_thread_id)
            && self
                .pty_screens
                .get(&self.active_thread_id)
                .is_some_and(|s| s.alternate_screen)
    }
}
