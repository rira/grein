use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::engine_port::{AgentConfigChoice, AgentConfigOption, UserPreferencesSnapshot};
use crate::keymap::Keymap;
use crate::theme::{ThemePalette, ThemePickerModal, ThemePreset};

use super::editing::{clamp_char_boundary, is_altgr_char, prev_word_boundary};
use super::threads::normalize_custom_name;
use super::types::{
    App, AppAction, ConfigPickerModal, FocusedPane, PendingPermission, RenamePromptState,
    SidebarView,
};

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

impl App {
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

    pub(super) fn handle_key_config_picker(&mut self, key: KeyEvent) {
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

    pub fn begin_inline_rename(&mut self) {
        let target_id =
            if self.focused_pane == FocusedPane::Tree && self.sidebar_view == SidebarView::Grein {
                self.selected_tree_thread_info().0
            } else {
                self.active_thread_id.clone()
            };
        if target_id.is_empty() {
            return;
        }
        let existing_custom = self
            .threads
            .iter()
            .find(|t| t.id == target_id)
            .and_then(|t| normalize_custom_name(t.custom_name.as_deref()))
            .or_else(|| {
                self.thread_meta
                    .get(&target_id)
                    .and_then(|m| normalize_custom_name(m.custom_name.as_deref()))
            })
            .unwrap_or_default();
        self.pending_rename_thread_id = None;
        self.palette_active = false;
        self.worktree_filter_active = false;
        self.rename_prompt = Some(RenamePromptState {
            thread_id: target_id,
            cursor: existing_custom.len(),
            buffer: existing_custom,
        });
        self.sync_focus_out();
    }

    pub fn apply_preferences(&mut self, prefs: &UserPreferencesSnapshot) {
        let preset = ThemePalette::resolve_preset(prefs.theme.as_deref(), &prefs.theme_overrides);
        self.theme_preset = preset;
        self.theme_overrides = prefs.theme_overrides.clone();
        self.theme_palette =
            ThemePalette::from_prefs(prefs.theme.as_deref(), &prefs.theme_overrides);
        self.keymap = Keymap::from_prefs(&prefs.keybindings);
        self.sidebar_layout.apply_ui_prefs(&prefs.ui);
        if let Some(val) = prefs.ui.get("composer_autocomplete") {
            match val.trim().to_ascii_lowercase().as_str() {
                "off" | "false" | "0" | "no" => {
                    self.composer_autocomplete_enabled = false;
                    self.composer_completion = None;
                }
                "on" | "true" | "1" | "yes" => {
                    self.composer_autocomplete_enabled = true;
                }
                _ => {}
            }
        }
        if let Some(def_provider) = prefs
            .ui
            .get("default_agent_provider")
            .map(String::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            self.default_agent_provider = Some(def_provider.to_string());
            if self
                .agent_providers
                .iter()
                .any(|p| p.id == def_provider && p.available)
            {
                for p in &mut self.agent_providers {
                    p.is_default = p.id == def_provider;
                }
            }
        }
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
            let names: Vec<&str> = ThemePreset::all().iter().map(ThemePreset::as_str).collect();
            self.status_notice = Some(format!(
                "Unknown theme '{trimmed}'. Available: {}",
                names.join(", ")
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

    pub(super) fn handle_key_theme_picker(&mut self, key: KeyEvent) {
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

    pub(super) fn handle_key_rename_prompt(&mut self, key: KeyEvent) {
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
                self.status_notice = None;
                self.pending_actions.push_back(AppAction::RenameThread {
                    thread_id: prompt.thread_id,
                    title: new_title,
                });
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
            (KeyCode::Char('v' | 'V'), true, false) => {
                self.paste_from_clipboard();
            }
            (KeyCode::Char(c), _, _) if is_altgr_char(c, ctrl, alt) => {
                prompt.buffer.insert(pos, c);
                prompt.cursor = pos + c.len_utf8();
            }
            _ => {}
        }
    }

    pub(super) fn handle_key_modals(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let no_ctrl_alt = !ctrl && !alt;

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

        if self.agent_picker.is_some() {
            self.pending_g_prefix = false;
            self.handle_key_agent_picker(key);
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
            if self.help_filter_active {
                match (key.code, ctrl, alt) {
                    (KeyCode::Esc, false, false) | (KeyCode::Char('c' | 'C'), true, false) => {
                        self.help_filter_active = false;
                        self.help_filter_query.clear();
                        self.help_scroll_offset = 0;
                    }
                    (KeyCode::Enter, false, false) => {
                        self.help_filter_active = false;
                    }
                    (KeyCode::Backspace, false, false) => {
                        self.help_filter_query.pop();
                        self.help_scroll_offset = 0;
                    }
                    (KeyCode::Char('u' | 'U'), true, false) => {
                        self.help_filter_query.clear();
                        self.help_scroll_offset = 0;
                    }
                    (KeyCode::Char('w' | 'W'), true, false) => {
                        let trimmed_len = self.help_filter_query.trim_end().len();
                        self.help_filter_query.truncate(trimmed_len);
                        let start = prev_word_boundary(
                            &self.help_filter_query,
                            self.help_filter_query.len(),
                        );
                        self.help_filter_query.truncate(start);
                        self.help_scroll_offset = 0;
                    }
                    (KeyCode::Up, false, false) | (KeyCode::Char('p' | 'P'), true, false) => {
                        self.help_scroll_offset = self.help_scroll_offset.saturating_sub(1);
                    }
                    (KeyCode::Down, false, false) | (KeyCode::Char('n' | 'N'), true, false) => {
                        self.help_scroll_offset =
                            clamp_help(self, self.help_scroll_offset.saturating_add(1));
                    }
                    (KeyCode::PageUp, false, false) => {
                        self.help_scroll_offset = self.help_scroll_offset.saturating_sub(10);
                    }
                    (KeyCode::PageDown, false, false) => {
                        self.help_scroll_offset =
                            clamp_help(self, self.help_scroll_offset.saturating_add(10));
                    }
                    (KeyCode::Home, false, false) => {
                        self.help_scroll_offset = 0;
                    }
                    (KeyCode::End, false, false) => {
                        self.help_scroll_offset = self.help_max_scroll;
                    }
                    (KeyCode::Char(c), _, _) if is_altgr_char(c, ctrl, alt) => {
                        self.help_filter_query.push(c);
                        self.help_scroll_offset = 0;
                    }
                    _ => {}
                }
            } else {
                match key.code {
                    KeyCode::Char('/') if no_ctrl_alt => {
                        self.help_filter_active = true;
                    }
                    KeyCode::Esc if no_ctrl_alt => {
                        if !self.help_filter_query.is_empty() {
                            self.help_filter_query.clear();
                            self.help_scroll_offset = 0;
                        } else {
                            self.show_help_modal = false;
                            self.help_scroll_offset = 0;
                            self.help_filter_active = false;
                        }
                    }
                    KeyCode::Enter | KeyCode::Char('q' | 'Q' | '?') if no_ctrl_alt => {
                        self.show_help_modal = false;
                        self.help_scroll_offset = 0;
                        self.help_filter_active = false;
                        self.help_filter_query.clear();
                    }
                    KeyCode::Char('c' | 'C') if ctrl && !alt => {
                        self.show_help_modal = false;
                        self.help_scroll_offset = 0;
                        self.help_filter_active = false;
                        self.help_filter_query.clear();
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
                    KeyCode::Char('f' | 'F') if ctrl && !alt => {
                        self.help_scroll_offset =
                            clamp_help(self, self.help_scroll_offset.saturating_add(20));
                    }
                    KeyCode::PageUp if no_ctrl_alt => {
                        self.help_scroll_offset = self.help_scroll_offset.saturating_sub(10);
                    }
                    KeyCode::Char('u' | 'U') if ctrl && !alt => {
                        self.help_scroll_offset = self.help_scroll_offset.saturating_sub(10);
                    }
                    KeyCode::Char('b' | 'B') if ctrl && !alt => {
                        self.help_scroll_offset = self.help_scroll_offset.saturating_sub(20);
                    }
                    KeyCode::Home | KeyCode::Char('g') if no_ctrl_alt => {
                        self.help_scroll_offset = 0;
                    }
                    KeyCode::End | KeyCode::Char('G') if no_ctrl_alt => {
                        self.help_scroll_offset = self.help_max_scroll;
                    }
                    _ => {}
                }
            }
            self.sync_focus_out();
            return true;
        }

        false
    }
}
