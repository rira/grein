use crate::engine_port::PtyScreenSnapshot;
use crate::search::SearchDirection;
use crate::transcript::{
    extract_last_fenced_code_block, ThreadTranscript, TranscriptBlock, TranscriptViewport,
};

use super::types::{App, AppAction, FocusedPane, SidebarView};

#[derive(Debug, Clone, Copy)]
pub(super) enum TranscriptScroll {
    Up(usize),
    Down(usize),
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CopyMode {
    LastReply,
    CodeBlock,
    All,
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

    pub(super) fn transcript_scroll_target_id(&self) -> String {
        if self.focused_pane == FocusedPane::Tree && self.sidebar_view == SidebarView::Grein {
            self.selected_tree_thread_info().0
        } else {
            self.active_thread_id.clone()
        }
    }

    pub(super) fn half_page_scroll_lines(&self) -> usize {
        if self.transcript_viewport_height > 0 {
            (self.transcript_viewport_height as usize / 2).max(1)
        } else {
            10
        }
    }

    pub(super) fn full_page_scroll_lines(&self) -> usize {
        if self.transcript_viewport_height > 0 {
            self.transcript_viewport_height as usize
        } else {
            20
        }
    }

    pub(super) fn scroll_manage_preview(&mut self, forward: bool, lines: usize) {
        if self.sidebar_view == SidebarView::Worktrees {
            if forward {
                self.worktree_diff_scroll = self
                    .worktree_diff_scroll
                    .saturating_add(lines as u16)
                    .min(self.worktree_diff_max_scroll);
            } else {
                self.worktree_diff_scroll = self.worktree_diff_scroll.saturating_sub(lines as u16);
            }
            return;
        }
        let target_id = self.transcript_scroll_target_id();
        if self.is_terminal_thread(&target_id) {
            let pty_step = if lines >= 15 { 10 } else { 5 };
            let delta = if forward { -pty_step } else { pty_step };
            self.pending_actions.push_back(AppAction::PtyScroll {
                thread_id: target_id,
                delta,
            });
        } else {
            let action = if forward {
                TranscriptScroll::Down(lines)
            } else {
                TranscriptScroll::Up(lines)
            };
            self.scroll_target_transcript(action);
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

    pub(super) fn finish_copy_to_clipboard(&mut self, kind: &str, text: String) {
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
            SearchDirection::Forward => Some(vp.scroll_offset()),
            SearchDirection::Reverse => {
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

    pub(super) fn recompute_transcript_search_and_scroll(&mut self) {
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

    pub(super) fn step_transcript_search(&mut self, next: bool) {
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

    pub(super) fn open_transcript_search(&mut self, direction: SearchDirection) {
        self.transcript_search.open(direction);
        self.pending_g_prefix = false;
    }

    pub(super) fn scroll_target_transcript(&mut self, action: TranscriptScroll) {
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
}
