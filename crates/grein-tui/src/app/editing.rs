use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::types::{App, AppAction, FocusedPane};

pub(crate) fn is_altgr_char(c: char, ctrl: bool, alt: bool) -> bool {
    (!ctrl && !alt) || (ctrl && alt && !c.is_ascii_control())
}

pub(super) fn clamp_char_boundary(buf: &str, pos: usize) -> usize {
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

impl App {
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
            self.composer_desired_col = None;
            self.composer_completion_dismissed = false;
            self.sync_worktree_autocomplete(true);
            self.sync_composer_completion();
            self.sync_focus_out();
            return true;
        }
        false
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

    fn kill_range(&mut self, start: usize, end: usize) {
        let start = clamp_char_boundary(&self.input_buffer, start);
        let end = clamp_char_boundary(&self.input_buffer, end);
        if end > start {
            self.kill_ring = self.input_buffer.drain(start..end).collect();
        }
        self.cursor_position = start;
    }

    fn move_composer_cursor_vertical(&mut self, delta: isize) -> bool {
        let max_width = (self.last_composer_inner_width > 0)
            .then_some(self.last_composer_inner_width as usize);
        let layout = crate::render::layout_composer_visual_rows(
            &self.input_buffer,
            self.cursor_position,
            max_width,
        );
        if layout.rows.is_empty() {
            return false;
        }
        let curr_row = layout.cursor_row as usize;
        let target_row_isize = curr_row as isize + delta;
        if target_row_isize < 0 || (target_row_isize as usize) >= layout.rows.len() {
            return false;
        }
        let desired_col = *self
            .composer_desired_col
            .get_or_insert(layout.cursor_col as usize);
        let target_row = &layout.rows[target_row_isize as usize];
        let best_byte = target_row
            .stops
            .iter()
            .copied()
            .min_by_key(|&(_, col)| (col.abs_diff(desired_col), usize::from(col > desired_col)))
            .map(|(byte, _)| byte)
            .unwrap_or(self.cursor_position);
        self.cursor_position = clamp_char_boundary(&self.input_buffer, best_byte);
        true
    }

    pub(super) fn handle_key_composer(&mut self, key: KeyEvent) -> bool {
        let prev_buf = self.input_buffer.clone();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let is_vertical = matches!(key.code, KeyCode::Up | KeyCode::Down) && !ctrl && !alt;
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
            (KeyCode::Up, false, false) => self.move_composer_cursor_vertical(-1),
            (KeyCode::Down, false, false) => self.move_composer_cursor_vertical(1),
            _ => false,
        };
        if !is_vertical && (handled || self.input_buffer != prev_buf) {
            self.composer_desired_col = None;
        }
        if handled && self.input_buffer != prev_buf {
            if !matches!(key.code, KeyCode::Enter) {
                self.status_notice = None;
            }
            self.composer_completion_dismissed = false;
            self.sync_worktree_autocomplete(true);
            self.sync_composer_completion();
        } else if handled && !self.composer_completion_dismissed {
            self.sync_composer_completion();
        }
        handled
    }
}
