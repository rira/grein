use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind};

use crate::keymap::BindableAction;
use crate::search::SearchDirection;

use super::editing::is_altgr_char;
use super::transcript_ops::{CopyMode, TranscriptScroll};
use super::types::{App, AppAction, FocusedPane, PendingThreadDelete, SidebarView, TreeAction};

impl App {
    pub fn handle_mouse_event(&mut self, mouse: MouseEvent) -> bool {
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
            KeyCode::Char('n') if no_ctrl_alt => {
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
            KeyCode::Char('s') if no_ctrl_alt => {
                let (parent_thread_id, _) = self.selected_tree_thread_info();
                self.dispatch_create_terminal(Some(parent_thread_id), None);
                None
            }
            KeyCode::Char('S') if no_ctrl_alt => {
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
            KeyCode::Char('d') if no_ctrl_alt => {
                if self.threads.len() <= 1 {
                    self.status_notice = Some("Cannot delete the only context".to_string());
                } else {
                    let (thread_id, title) = self.selected_tree_thread_info();
                    self.pending_thread_delete = Some(PendingThreadDelete { thread_id, title });
                }
                None
            }
            KeyCode::Char('H') if no_ctrl_alt => {
                self.collapse_all_in_tree();
                None
            }
            KeyCode::Char('L') if no_ctrl_alt => {
                self.expand_all_in_tree();
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
                let _ = self.expand_or_step_child_in_tree();
                None
            }
            KeyCode::Enter => self.activate_selected_tree_node(),
            _ => None,
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
        let no_ctrl_alt = !ctrl && !alt;

        // 1. Modal priority first (before EnterManageMode & ToggleZoom)
        if self.handle_key_modals(key) {
            return true;
        }

        // 2. WRITE Mode (Terminal & Composer unified minimal intercepts)
        if matches!(
            self.focused_pane,
            FocusedPane::Terminal | FocusedPane::Composer
        ) {
            self.pending_g_prefix = false;
            if let Some(action) = self.keymap.match_write_key(key) {
                let pty_alt_passthrough = self.active_pty_is_alt_screen()
                    && matches!(
                        action,
                        BindableAction::ScrollUp
                            | BindableAction::ScrollDown
                            | BindableAction::PrevSibling
                            | BindableAction::NextSibling
                    );
                if !pty_alt_passthrough {
                    self.execute_bindable_action(action);
                    self.sync_focus_out();
                    return true;
                }
            }
            if alt && !ctrl {
                if let KeyCode::Char(c @ '1'..='9') = key.code {
                    self.select_root_by_index((c as u8 - b'0') as usize);
                    self.sync_focus_out();
                    return true;
                }
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
                    self.composer_desired_col = None;
                    self.worktree_autocomplete_open = false;
                    self.worktree_autocomplete_idx = 0;
                    self.status_notice = Some("Cleared input".to_string());
                } else {
                    let manage_hint = self.keymap.display_for(BindableAction::EnterManageMode);
                    self.status_notice =
                        Some(format!("Use {manage_hint} then q (or /quit) to exit"));
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

            // Tab switches between Grein and Worktrees sidebar views
            if no_ctrl_alt && key.code == KeyCode::Tab {
                self.toggle_worktrees_sidebar_view();
                return true;
            }

            // Help via ?
            if no_ctrl_alt && key.code == KeyCode::Char('?') {
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

            // Alt+1..9 jumps directly to depth-0 root 1..9 (AltGr excluded)
            if alt && !ctrl {
                if let KeyCode::Char(c @ '1'..='9') = key.code {
                    self.select_root_by_index((c as u8 - b'0') as usize);
                    self.sync_focus_out();
                    return true;
                }
            }

            // Check keymap bindings in MANAGE too (Ctrl+G exits MANAGE -> WRITE;
            // Alt+Z toggles zoom; Alt+H/L cycle siblings; Alt+K/J scroll preview target)
            if let Some(action) = self.keymap.match_write_key(key) {
                self.execute_bindable_action(action);
                self.sync_focus_out();
                return true;
            }

            // Right-pane preview scrolling (Ctrl+U/D half-page, Ctrl+B/F full-page)
            match (key.code, ctrl, alt) {
                (KeyCode::Char('u' | 'U'), true, false) => {
                    let step = self.half_page_scroll_lines();
                    self.scroll_manage_preview(false, step);
                    self.sync_focus_out();
                    return true;
                }
                (KeyCode::Char('d' | 'D'), true, false) => {
                    let step = self.half_page_scroll_lines();
                    self.scroll_manage_preview(true, step);
                    self.sync_focus_out();
                    return true;
                }
                (KeyCode::Char('b' | 'B'), true, false) => {
                    let step = self.full_page_scroll_lines();
                    self.scroll_manage_preview(false, step);
                    self.sync_focus_out();
                    return true;
                }
                (KeyCode::Char('f' | 'F'), true, false) => {
                    let step = self.full_page_scroll_lines();
                    self.scroll_manage_preview(true, step);
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
                        self.open_transcript_search(SearchDirection::Forward);
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
            let _ = self.handle_key_tree_with_prefix(key, had_g_prefix);
            self.sync_focus_out();
            return true;
        }

        // 4. Legacy Transcript pane fallback (if unit tests explicitly focus Transcript)
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
        if alt && !ctrl {
            if let KeyCode::Char(c @ '1'..='9') = key.code {
                self.select_root_by_index((c as u8 - b'0') as usize);
                self.sync_focus_out();
                return true;
            }
        }
        if let Some(action) = self.keymap.match_write_key(key) {
            self.execute_bindable_action(action);
            self.sync_focus_out();
            return true;
        }
        if no_ctrl_alt && key.code == KeyCode::Char('?') {
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
                self.open_transcript_search(SearchDirection::Forward);
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
