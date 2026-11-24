use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::text::{Line, Span};

use crate::engine_port::{PtyCell, PtyMouseMode};
use crate::runner::now_epoch_ms;
use crate::selection::{
    extract_lines_selection, extract_pty_selection, word_bounds_at, PaneRects, PaneSelection,
    SelectionPane,
};
use crate::ui::tree::TreeNode;

use super::transcript_ops::TranscriptScroll;
use super::types::{App, AppAction, FocusedPane, SidebarView};

const DOUBLE_CLICK_WINDOW_MS: u64 = 400;

impl App {
    pub fn clear_pane_selection(&mut self) {
        self.pane_selection = None;
        self.frozen_pty_snapshot = None;
        self.last_click_info = None;
    }

    pub fn read_primary_or_clipboard(&self) -> Result<String, String> {
        match self.services.clipboard.read_primary() {
            Ok(s) if !s.is_empty() => Ok(s),
            Ok(_) => self.services.clipboard.read_clipboard().or(Ok(String::new())),
            Err(_) => self.services.clipboard.read_clipboard(),
        }
    }

    fn visible_right_pane_target(&self) -> (SelectionPane, String) {
        let tree_focused = self.focused_pane == FocusedPane::Tree;
        if tree_focused && !self.sidebar_hidden && self.sidebar_view == SidebarView::Worktrees {
            return (SelectionPane::WorktreeDiff, self.active_thread_id.clone());
        }
        let display_thread_id =
            if tree_focused && !self.sidebar_hidden && self.sidebar_view == SidebarView::Grein {
                let sel_id = self.selected_tree_thread_info().0;
                if !sel_id.is_empty() {
                    sel_id
                } else {
                    self.active_thread_id.clone()
                }
            } else {
                self.active_thread_id.clone()
            };
        let pane = if self.is_terminal_thread(&display_thread_id) {
            SelectionPane::Terminal
        } else {
            SelectionPane::Transcript
        };
        (pane, display_thread_id)
    }

    fn effective_pane_rects(&self) -> PaneRects {
        if self.pane_rects.right_inner.is_some() {
            return self.pane_rects;
        }
        let Some(area) = self.last_frame_area else {
            return self.pane_rects;
        };
        if area.width == 0 || area.height == 0 {
            return PaneRects::default();
        }
        let sub_count = self
            .thread_subagents
            .get(&self.active_thread_id)
            .map_or(0, Vec::len);
        let strip_h = crate::layout::subagent_strip_height(sub_count);
        let (pane, _) = self.visible_right_pane_target();
        let show_composer = (!self.active_is_terminal() && pane == SelectionPane::Transcript)
            || self.pending_rename_thread_id.is_some();
        let mut layout = crate::render::compute_app_layout(self, area, strip_h, !show_composer);
        if show_composer && layout.composer_area.height > 0 {
            let composer_w = layout.composer_area.width.saturating_sub(2) as usize;
            let visual_rows = crate::render::layout_composer_visual_rows(
                &self.input_buffer,
                self.cursor_position,
                (composer_w > 0).then_some(composer_w),
            )
            .rows
            .len()
            .max(1) as u16;
            let max_composer_h = layout
                .transcript_area
                .height
                .saturating_add(layout.composer_area.height)
                .saturating_sub(3)
                .max(3);
            let desired_height = (visual_rows + 2).clamp(3, 10).min(max_composer_h);
            if desired_height > layout.composer_area.height {
                let delta = desired_height - layout.composer_area.height;
                layout.transcript_area.height =
                    layout.transcript_area.height.saturating_sub(delta);
                layout.composer_area.y = layout.composer_area.y.saturating_sub(delta);
                layout.composer_area.height = desired_height;
            }
        }
        let tree_inner = if self.sidebar_hidden
            || layout.tree_area.width <= 2
            || layout.tree_area.height <= 2
        {
            None
        } else {
            let inner_x = layout.tree_area.x + 1;
            let mut inner_y = layout.tree_area.y + 1;
            let inner_w = layout.tree_area.width - 2;
            let mut inner_h = layout.tree_area.height - 2;
            if self.sidebar_view == SidebarView::Worktrees
                && (self.worktree_filter_active || !self.worktree_filter_query.is_empty())
                && inner_h >= 2
            {
                inner_y += 1;
                inner_h -= 1;
            }
            if inner_h >= 2 {
                inner_h -= 1;
            }
            (inner_w > 0 && inner_h > 0).then_some(Rect::new(inner_x, inner_y, inner_w, inner_h))
        };
        let right_inner = {
            let rx = if self.sidebar_hidden {
                layout.transcript_area.x
            } else {
                layout.transcript_area.x + 1
            };
            let rw = if self.sidebar_hidden {
                layout.transcript_area.width
            } else {
                layout.transcript_area.width.saturating_sub(2)
            };
            let ry = layout.transcript_area.y + 1;
            let rh = layout.transcript_area.height.saturating_sub(2);
            (rw > 0 && rh > 0).then_some(Rect::new(rx, ry, rw, rh))
        };
        let composer_inner = if show_composer
            && layout.composer_area.width > 2
            && layout.composer_area.height > 2
        {
            Some(Rect::new(
                layout.composer_area.x + 1,
                layout.composer_area.y + 1,
                layout.composer_area.width - 2,
                layout.composer_area.height - 2,
            ))
        } else {
            None
        };
        PaneRects {
            tree_inner,
            right_inner,
            composer_inner,
        }
    }

    fn scroll_y_for_pane(&self, pane: SelectionPane) -> usize {
        match pane {
            SelectionPane::Terminal => 0,
            SelectionPane::Transcript => self.last_transcript_scroll_y,
            SelectionPane::WorktreeDiff => self.worktree_diff_scroll as usize,
        }
    }

    fn line_plain_text(line: &Line<'_>) -> String {
        let mut text = String::new();
        for span in &line.spans {
            text.push_str(span.content.as_ref());
        }
        text
    }

    fn line_trimmed_end_col(line: &Line<'_>, is_hard_ended: bool) -> Option<usize> {
        let raw = Self::line_plain_text(line);
        let effective = if is_hard_ended {
            raw.trim_end_matches(' ')
        } else {
            raw.as_str()
        };
        let width = Span::raw(effective).width();
        (width > 0).then_some(width - 1)
    }

    fn pty_row_effective_len(row: &[PtyCell], is_wrapped: bool) -> usize {
        if is_wrapped {
            return row.len();
        }
        let Some(last_content) = row.iter().rposition(|cell| {
            !cell.is_wide_continuation
                && !cell.contents.is_empty()
                && !cell.contents.chars().all(|c| c == ' ')
        }) else {
            return 0;
        };
        let mut len = last_content + 1;
        while row.get(len).is_some_and(|cell| cell.is_wide_continuation) {
            len += 1;
        }
        len
    }

    fn pty_row_text_for_word_bounds(row: &[PtyCell], is_wrapped: bool) -> String {
        let effective_len = Self::pty_row_effective_len(row, is_wrapped);
        let mut out = String::new();
        for cell in row.iter().take(effective_len) {
            if cell.is_wide_continuation {
                continue;
            }
            if cell.contents.is_empty() || Span::raw(cell.contents.as_str()).width() == 0 {
                out.push(' ');
            } else {
                out.push_str(&cell.contents);
            }
        }
        out
    }

    fn extract_selection_text(&self, sel: &PaneSelection, display_thread_id: &str) -> String {
        let (start, end) = sel.ordered();
        match sel.pane {
            SelectionPane::Terminal => {
                let snap_opt = self
                    .frozen_pty_snapshot
                    .as_ref()
                    .filter(|(tid, _)| tid == display_thread_id)
                    .map(|(_, s)| s)
                    .or_else(|| self.pty_screens.get(display_thread_id));
                let Some(snap) = snap_opt else {
                    return String::new();
                };
                extract_pty_selection(&snap.cells, &snap.row_wrapped, start, end)
            }
            SelectionPane::Transcript | SelectionPane::WorktreeDiff => {
                extract_lines_selection(&self.last_rendered_lines, &self.last_rendered_meta, start, end)
            }
        }
    }

    fn clamp_to_right_content(&self, right_inner: Rect, pane: SelectionPane, pos: Position) -> (usize, usize) {
        let clamped_col = pos
            .x
            .clamp(right_inner.left(), right_inner.right().saturating_sub(1));
        let clamped_row = pos
            .y
            .clamp(right_inner.top(), right_inner.bottom().saturating_sub(1));
        let local_col = usize::from(clamped_col - right_inner.x);
        let local_row = usize::from(clamped_row - right_inner.y);
        let scroll_y = self.scroll_y_for_pane(pane);
        (scroll_y + local_row, local_col)
    }

    fn update_drag_selection(&mut self, right_inner: Option<Rect>, pos: Position) -> bool {
        let Some(right_inner) = right_inner.filter(|r| r.width > 0 && r.height > 0) else {
            self.clear_pane_selection();
            return false;
        };
        let Some(pane) = self.pane_selection.as_ref().map(|s| s.pane) else {
            return false;
        };
        let focus = self.clamp_to_right_content(right_inner, pane, pos);
        if let Some(ref mut sel) = self.pane_selection {
            sel.focus = focus;
        }
        true
    }

    fn finish_drag_selection(&mut self, right_inner: Option<Rect>, pos: Position) -> bool {
        let Some(mut sel) = self.pane_selection.take() else {
            self.frozen_pty_snapshot = None;
            return false;
        };
        if let Some(right_inner) = right_inner.filter(|r| r.width > 0 && r.height > 0) {
            sel.focus = self.clamp_to_right_content(right_inner, sel.pane, pos);
        }
        sel.dragging = false;
        if sel.anchor == sel.focus {
            self.frozen_pty_snapshot = None;
            return true;
        }
        self.last_click_info = None;
        let (_, display_thread_id) = self.visible_right_pane_target();
        let text = self.extract_selection_text(&sel, &display_thread_id);
        self.frozen_pty_snapshot = None;
        if text.is_empty() {
            self.pane_selection = None;
        } else {
            self.pane_selection = Some(sel);
            self.finish_copy_to_clipboard("selection", text);
        }
        true
    }

    fn select_word_at(
        &mut self,
        pane: SelectionPane,
        display_thread_id: &str,
        content_row: usize,
        local_col: usize,
    ) {
        let bounds = match pane {
            SelectionPane::Terminal => self
                .frozen_pty_snapshot
                .as_ref()
                .filter(|(tid, _)| tid == display_thread_id)
                .map(|(_, s)| s)
                .or_else(|| self.pty_screens.get(display_thread_id))
                .and_then(|snap| {
                    let row = snap.cells.get(content_row)?;
                    let is_wrapped = snap.row_wrapped.get(content_row).copied().unwrap_or(false);
                    let text = Self::pty_row_text_for_word_bounds(row, is_wrapped);
                    word_bounds_at(&text, local_col)
                }),
            SelectionPane::Transcript | SelectionPane::WorktreeDiff => self
                .last_rendered_lines
                .get(content_row)
                .and_then(|line| {
                    let is_hard_ended = !self
                        .last_rendered_meta
                        .get(content_row + 1)
                        .is_some_and(|m| m.soft_wrapped);
                    let raw = Self::line_plain_text(line);
                    let effective = if is_hard_ended {
                        raw.trim_end_matches(' ')
                    } else {
                        raw.as_str()
                    };
                    word_bounds_at(effective, local_col)
                }),
        };

        let Some((ws, we)) = bounds else {
            self.pane_selection = None;
            self.frozen_pty_snapshot = None;
            return;
        };

        let sel = PaneSelection {
            pane,
            anchor: (content_row, ws),
            focus: (content_row, we),
            dragging: false,
        };
        let text = self.extract_selection_text(&sel, display_thread_id);
        self.frozen_pty_snapshot = None;
        if text.is_empty() {
            self.pane_selection = None;
        } else {
            self.pane_selection = Some(sel);
            self.finish_copy_to_clipboard("selection", text);
        }
    }

    fn select_line_at(&mut self, pane: SelectionPane, display_thread_id: &str, content_row: usize) {
        let end_col = match pane {
            SelectionPane::Terminal => self
                .frozen_pty_snapshot
                .as_ref()
                .filter(|(tid, _)| tid == display_thread_id)
                .map(|(_, s)| s)
                .or_else(|| self.pty_screens.get(display_thread_id))
                .and_then(|snap| {
                    let row = snap.cells.get(content_row)?;
                    let is_wrapped = snap.row_wrapped.get(content_row).copied().unwrap_or(false);
                    let len = Self::pty_row_effective_len(row, is_wrapped);
                    (len > 0).then_some(len - 1)
                }),
            SelectionPane::Transcript | SelectionPane::WorktreeDiff => self
                .last_rendered_lines
                .get(content_row)
                .and_then(|line| {
                    let is_hard_ended = !self
                        .last_rendered_meta
                        .get(content_row + 1)
                        .is_some_and(|m| m.soft_wrapped);
                    Self::line_trimmed_end_col(line, is_hard_ended)
                }),
        };

        let Some(line_end) = end_col else {
            self.pane_selection = None;
            self.frozen_pty_snapshot = None;
            return;
        };

        let sel = PaneSelection {
            pane,
            anchor: (content_row, 0),
            focus: (content_row, line_end),
            dragging: false,
        };
        let text = self.extract_selection_text(&sel, display_thread_id);
        self.frozen_pty_snapshot = None;
        if text.is_empty() {
            self.pane_selection = None;
        } else {
            self.pane_selection = Some(sel);
            self.finish_copy_to_clipboard("selection", text);
        }
    }

    fn handle_right_pane_press(&mut self, pos: Position, right_inner: Rect) -> bool {
        let (pane, display_thread_id) = self.visible_right_pane_target();
        if pane == SelectionPane::Terminal && display_thread_id == self.active_thread_id {
            self.pending_rename_thread_id = None;
            self.set_focus(FocusedPane::Terminal);
        }

        let local_col = usize::from(pos.x - right_inner.x);
        let local_row = usize::from(pos.y - right_inner.y);
        let content_row = self.scroll_y_for_pane(pane) + local_row;

        let now_ms = now_epoch_ms();
        let click_count = match self.last_click_info {
            Some((prev_ms, prev_col, prev_row, prev_count))
                if prev_col == pos.x
                    && prev_row == pos.y
                    && now_ms >= prev_ms
                    && now_ms - prev_ms <= DOUBLE_CLICK_WINDOW_MS =>
            {
                (prev_count % 3) + 1
            }
            _ => 1,
        };
        self.last_click_info = Some((now_ms, pos.x, pos.y, click_count));

        if pane == SelectionPane::Terminal {
            self.frozen_pty_snapshot = self
                .pty_screens
                .get(&display_thread_id)
                .cloned()
                .map(|s| (display_thread_id.clone(), s));
        } else {
            self.frozen_pty_snapshot = None;
        }

        match click_count {
            2 => {
                self.select_word_at(pane, &display_thread_id, content_row, local_col);
            }
            3 => {
                self.select_line_at(pane, &display_thread_id, content_row);
            }
            _ => {
                self.pane_selection = Some(PaneSelection {
                    pane,
                    anchor: (content_row, local_col),
                    focus: (content_row, local_col),
                    dragging: true,
                });
            }
        }
        true
    }

    fn handle_left_press(&mut self, pos: Position, rects: PaneRects) -> bool {
        if let Some(tree_inner) = rects.tree_inner {
            if tree_inner.contains(pos) {
                self.clear_pane_selection();
                self.focus_tree();
                let rel_row = usize::from(pos.y - tree_inner.y);
                if self.sidebar_view == SidebarView::Worktrees {
                    let row_idx = self.worktree_scroll_offset + rel_row;
                    let row_count = self.visible_worktree_rows().len();
                    if row_idx < row_count {
                        self.worktree_selected_idx = row_idx;
                        self.maybe_fetch_selected_worktree_diff(false);
                    }
                } else {
                    if self.tree.nodes.is_empty() && !self.threads.is_empty() {
                        crate::runner::rebuild_tree_nodes(self);
                    }
                    let row_idx = (self.tree.scroll_offset as usize) + rel_row;
                    if let Some(node_idx) = self
                        .tree
                        .nodes
                        .iter()
                        .enumerate()
                        .filter(|(_, n)| matches!(n, TreeNode::Thread(_)))
                        .nth(row_idx)
                        .map(|(idx, _)| idx)
                    {
                        self.tree.select_index(node_idx);
                        let (sel_id, _) = self.selected_tree_thread_info();
                        self.maybe_request_pty_snapshot(&sel_id);
                    }
                }
                return true;
            }
        }

        if let Some(composer_inner) = rects.composer_inner {
            if composer_inner.contains(pos) {
                self.clear_pane_selection();
                if !self.active_is_terminal() {
                    self.pending_rename_thread_id = None;
                    self.set_focus(FocusedPane::Composer);
                }
                return true;
            }
        }

        if let Some(right_inner) = rects.right_inner {
            if right_inner.contains(pos) {
                return self.handle_right_pane_press(pos, right_inner);
            }
        }

        self.clear_pane_selection();
        false
    }

    fn handle_middle_press(&mut self, pos: Position, rects: PaneRects) -> bool {
        if rects.composer_inner.is_some_and(|r| r.contains(pos)) && !self.active_is_terminal() {
            self.pending_rename_thread_id = None;
            self.set_focus(FocusedPane::Composer);
        } else if rects.right_inner.is_some_and(|r| r.contains(pos)) {
            let (pane, display_thread_id) = self.visible_right_pane_target();
            if pane == SelectionPane::Terminal && display_thread_id == self.active_thread_id {
                self.pending_rename_thread_id = None;
                self.set_focus(FocusedPane::Terminal);
            }
        }

        match self.read_primary_or_clipboard() {
            Ok(text) if !text.is_empty() => self.handle_paste_event(&text),
            Ok(_) => true,
            Err(err) => {
                self.status_notice = Some(format!("Paste failed: {err}"));
                false
            }
        }
    }

    fn handle_mouse_scroll(&mut self, pos: Position, rects: PaneRects, is_up: bool) -> bool {
        let delta: isize = if is_up { 3 } else { -3 };
        let over_tree_pane = self.focused_pane == FocusedPane::Tree
            && if let Some(right_inner) = rects.right_inner {
                !right_inner.contains(pos)
            } else {
                self.last_frame_area.is_none_or(|area| {
                    if self.sidebar_hidden || area.width == 0 || area.height == 0 {
                        false
                    } else {
                        let sub_count = self
                            .thread_subagents
                            .get(&self.active_thread_id)
                            .map_or(0, Vec::len);
                        let strip_h = crate::layout::subagent_strip_height(sub_count);
                        let composer_hidden = self.active_is_terminal()
                            || (self.focused_pane == FocusedPane::Tree
                                && !self.sidebar_hidden
                                && self.sidebar_view == SidebarView::Worktrees)
                            || self.pending_rename_thread_id.is_some();
                        let layout =
                            crate::render::compute_app_layout(self, area, strip_h, composer_hidden);
                        !layout.transcript_area.contains(pos)
                    }
                })
            };

        if self.focused_pane == FocusedPane::Tree
            && !self.sidebar_hidden
            && self.sidebar_view == SidebarView::Worktrees
            && !over_tree_pane
        {
            if is_up {
                self.worktree_diff_scroll = self.worktree_diff_scroll.saturating_sub(3);
            } else {
                self.worktree_diff_scroll = self
                    .worktree_diff_scroll
                    .saturating_add(3)
                    .min(self.worktree_diff_max_scroll);
            }
            return true;
        }

        if self.active_is_terminal() && !over_tree_pane {
            self.pending_actions.push_back(AppAction::PtyScroll {
                thread_id: self.active_thread_id.clone(),
                delta,
            });
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
        true
    }

    pub fn handle_mouse_event(&mut self, mouse: MouseEvent) -> bool {
        self.sync_focus_in();
        if self.show_help_modal {
            let handled = match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.help_scroll_offset = self.help_scroll_offset.saturating_sub(3);
                    true
                }
                MouseEventKind::ScrollDown => {
                    self.help_scroll_offset = self.help_scroll_offset.saturating_add(3);
                    true
                }
                _ => false,
            };
            self.sync_focus_out();
            return handled;
        }
        if self.has_active_modal() {
            self.sync_focus_out();
            return false;
        }

        let rects = self.effective_pane_rects();
        let pos = Position::new(mouse.column, mouse.row);
        let is_dragging = self.pane_selection.as_ref().is_some_and(|s| s.dragging);

        if is_dragging {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    let handled = self.update_drag_selection(rects.right_inner, pos);
                    self.sync_focus_out();
                    return handled;
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    let handled = self.finish_drag_selection(rects.right_inner, pos);
                    self.sync_focus_out();
                    return handled;
                }
                _ => {}
            }
        }

        if !is_dragging
            && self.focused_pane == FocusedPane::Terminal
            && self.active_is_terminal()
            && !self.is_terminal_exited(&self.active_thread_id)
            && !mouse.modifiers.contains(KeyModifiers::ALT)
        {
            if let Some(right_inner) = rects.right_inner {
                if right_inner.contains(pos) {
                    if let Some(snap) = self.pty_screens.get(&self.active_thread_id) {
                        if snap.mouse_protocol_mode != PtyMouseMode::None
                            && snap.scrollback_offset == 0
                        {
                            let local_col = mouse.column - right_inner.x;
                            let local_row = mouse.row - right_inner.y;
                            if let Some(bytes) = crate::vt_input::encode_mouse_event(
                                mouse,
                                local_col,
                                local_row,
                                snap.mouse_protocol_mode,
                                snap.mouse_protocol_encoding,
                            ) {
                                self.pending_actions.push_back(AppAction::PtyWrite {
                                    thread_id: self.active_thread_id.clone(),
                                    bytes,
                                });
                                self.sync_focus_out();
                                return true;
                            }
                            if !matches!(
                                mouse.kind,
                                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                            ) {
                                self.sync_focus_out();
                                return false;
                            }
                        }
                    }
                }
            }
        }

        if matches!(
            mouse.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        ) {
            let is_up = matches!(mouse.kind, MouseEventKind::ScrollUp);
            let handled = self.handle_mouse_scroll(pos, rects, is_up);
            self.sync_focus_out();
            return handled;
        }

        let handled = match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => self.handle_left_press(pos, rects),
            MouseEventKind::Down(MouseButton::Middle) => self.handle_middle_press(pos, rects),
            _ => false,
        };
        self.sync_focus_out();
        handled
    }
}
