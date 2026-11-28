use crate::engine_port::AgentProviderInfo;

use super::types::{AgentPickerModal, App, AppAction};

pub(crate) fn unavailable_provider_notice(provider: &AgentProviderInfo) -> String {
    match provider
        .install_hint
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty())
    {
        Some(hint) => format!("{} not found — install: {hint}", provider.display_name),
        None => format!("{} is not available on PATH", provider.display_name),
    }
}

fn match_provider_exact<'a>(
    providers: &'a [AgentProviderInfo],
    needle: &str,
) -> Option<&'a AgentProviderInfo> {
    let q = needle.trim();
    if q.is_empty() {
        return None;
    }
    providers
        .iter()
        .find(|p| p.id.eq_ignore_ascii_case(q))
        .or_else(|| providers.iter().find(|p| p.plugin_id.eq_ignore_ascii_case(q)))
        .or_else(|| {
            providers
                .iter()
                .find(|p| p.display_name.eq_ignore_ascii_case(q))
        })
}

pub(crate) fn resolve_provider_and_title<'a>(
    providers: &'a [AgentProviderInfo],
    raw: &str,
) -> Option<(&'a AgentProviderInfo, Option<String>)> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || providers.is_empty() {
        return None;
    }

    if let Some(quote) = trimmed.chars().next().filter(|&c| c == '"' || c == '\'') {
        if let Some(rel_end) = trimmed[1..].find(quote) {
            let quoted = &trimmed[1..1 + rel_end];
            let after = trimmed[2 + rel_end..].trim();
            if let Some(provider) = match_provider_exact(providers, quoted) {
                let title = (!after.is_empty()).then(|| after.to_string());
                return Some((provider, title));
            }
        }
    }

    let mut by_display_len: Vec<&AgentProviderInfo> = providers
        .iter()
        .filter(|p| !p.display_name.trim().is_empty())
        .collect();
    by_display_len.sort_by_key(|a| std::cmp::Reverse(a.display_name.len()));

    for provider in by_display_len {
        let name = provider.display_name.trim();
        let name_len = name.len();
        if let Some(prefix) = trimmed.get(..name_len) {
            if prefix.eq_ignore_ascii_case(name) {
                let rest = &trimmed[name_len..];
                if rest.is_empty() || rest.starts_with(char::is_whitespace) {
                    let clean_title = rest.trim();
                    let title = (!clean_title.is_empty()).then(|| clean_title.to_string());
                    return Some((provider, title));
                }
            }
        }
    }

    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let head = parts.next().unwrap_or("");
    let tail = parts.next().unwrap_or("").trim();
    if let Some(provider) = match_provider_exact(providers, head) {
        let title = (!tail.is_empty()).then(|| tail.to_string());
        return Some((provider, title));
    }

    None
}

impl App {
    pub fn set_agent_providers(&mut self, mut providers: Vec<AgentProviderInfo>) {
        if let Some(ref preferred) = self.default_agent_provider {
            if providers.iter().any(|p| p.id == *preferred && p.available) {
                for p in &mut providers {
                    p.is_default = p.id == *preferred;
                }
            }
        }
        self.agent_providers = providers;
        if let Some(ref mut picker) = self.agent_picker {
            if self.agent_providers.is_empty() {
                picker.selected_idx = 0;
            } else {
                picker.selected_idx = picker.selected_idx.min(self.agent_providers.len() - 1);
            }
        }
    }

    pub(crate) fn effective_parent_id_for(&self, thread_id: &str) -> Option<String> {
        self.thread_meta
            .get(thread_id)
            .and_then(|m| m.parent_thread_id.as_deref())
            .filter(|&pid| pid != thread_id && self.threads.iter().any(|t| t.id == pid))
            .map(str::to_string)
    }

    pub(crate) fn inherited_cwd_for(&self, thread_id: &str) -> Option<String> {
        self.thread_meta
            .get(thread_id)
            .and_then(|m| m.cwd.as_deref())
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(str::to_string)
    }

    fn default_agent_picker_index(&self, target_thread_id: &str) -> usize {
        if let Some(active_provider) = self
            .thread_meta
            .get(target_thread_id)
            .and_then(|m| m.agent_provider.as_deref())
        {
            if let Some(idx) = self
                .agent_providers
                .iter()
                .position(|p| p.id.eq_ignore_ascii_case(active_provider))
            {
                return idx;
            }
        }
        if let Some(idx) = self.agent_providers.iter().position(|p| p.is_default) {
            return idx;
        }
        self.agent_providers
            .iter()
            .position(|p| p.available)
            .unwrap_or(0)
    }

    pub fn open_agent_picker(&mut self) {
        self.palette_active = false;
        self.rename_prompt = None;
        let target_thread_id = self.selected_or_active_thread_id();
        let selected_idx = self.default_agent_picker_index(&target_thread_id);
        self.agent_picker = Some(AgentPickerModal {
            selected_idx,
            target_thread_id,
        });
    }

    fn spawn_sibling_with_provider(
        &mut self,
        target_thread_id: &str,
        provider: &AgentProviderInfo,
        title: Option<String>,
    ) -> bool {
        if !provider.available {
            self.status_notice = Some(unavailable_provider_notice(provider));
            return false;
        }
        let parent_thread_id = self.effective_parent_id_for(target_thread_id);
        let cwd = self.inherited_cwd_for(target_thread_id);
        self.status_notice = None;
        self.pending_actions
            .push_back(AppAction::CreateAgentThread {
                title,
                cwd,
                parent_thread_id,
                agent_provider: Some(provider.id.clone()),
            });
        true
    }

    pub(super) fn execute_agent_command(&mut self, rest: &str, from_palette: bool) {
        let trimmed = rest.trim();
        if trimmed.is_empty() {
            self.open_agent_picker();
            return;
        }
        let target_id = if from_palette {
            self.selected_or_active_thread_id()
        } else {
            self.active_thread_id.clone()
        };
        if let Some((provider, title)) = resolve_provider_and_title(&self.agent_providers, trimmed)
        {
            let provider = provider.clone();
            self.spawn_sibling_with_provider(&target_id, &provider, title);
        } else if self.agent_providers.is_empty() {
            self.status_notice = Some("No agent providers available".to_string());
        } else {
            let head = trimmed.split_whitespace().next().unwrap_or(trimmed);
            self.status_notice = Some(format!("Unknown agent provider '{head}'"));
        }
    }

    pub(super) fn try_execute_provider_palette_command(&mut self, trimmed: &str) -> bool {
        let Some((provider, title)) = resolve_provider_and_title(&self.agent_providers, trimmed)
        else {
            return false;
        };
        let provider = provider.clone();
        let target_id = self.selected_or_active_thread_id();
        self.spawn_sibling_with_provider(&target_id, &provider, title);
        true
    }

    pub(super) fn append_provider_palette_suggestions(
        &self,
        needle: &str,
        prefix_matches: &mut Vec<(String, String)>,
        substring_matches: &mut Vec<(String, String)>,
    ) {
        for provider in &self.agent_providers {
            let cmd = provider.id.clone();
            if prefix_matches.iter().any(|(c, _)| c == &cmd)
                || substring_matches.iter().any(|(c, _)| c == &cmd)
            {
                continue;
            }
            let desc = if provider.available {
                format!("New {} sibling thread", provider.display_name)
            } else {
                format!("New {} sibling thread [not found]", provider.display_name)
            };
            let cmd_lower = cmd.to_ascii_lowercase();
            let name_lower = provider.display_name.to_ascii_lowercase();
            if needle.is_empty() || cmd_lower.starts_with(needle) {
                prefix_matches.push((cmd, desc));
            } else if cmd_lower.contains(needle)
                || name_lower.contains(needle)
                || desc.to_ascii_lowercase().contains(needle)
            {
                substring_matches.push((cmd, desc));
            }
        }
    }

    pub(super) fn handle_key_agent_picker(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::{KeyCode, KeyModifiers};
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let no_ctrl_alt = !ctrl && !alt;
        let count = self.agent_providers.len();

        match (key.code, shift) {
            (KeyCode::Up, false) | (KeyCode::Char('k' | 'K'), _) if no_ctrl_alt => {
                if let Some(ref mut picker) = self.agent_picker {
                    picker.selected_idx = picker.selected_idx.saturating_sub(1);
                }
            }
            (KeyCode::Down, false) | (KeyCode::Char('j' | 'J'), _) if no_ctrl_alt => {
                if let Some(ref mut picker) = self.agent_picker {
                    if count > 0 {
                        picker.selected_idx = (picker.selected_idx + 1).min(count - 1);
                    }
                }
            }
            (KeyCode::Home, _) if no_ctrl_alt => {
                if let Some(ref mut picker) = self.agent_picker {
                    picker.selected_idx = 0;
                }
            }
            (KeyCode::End, _) if no_ctrl_alt => {
                if let Some(ref mut picker) = self.agent_picker {
                    if count > 0 {
                        picker.selected_idx = count - 1;
                    }
                }
            }
            (KeyCode::Enter, _) if no_ctrl_alt => {
                let Some(idx) = self.agent_picker.as_ref().map(|p| p.selected_idx) else {
                    return;
                };
                let Some(provider) = self
                    .agent_providers
                    .get(idx)
                    .or_else(|| self.agent_providers.first())
                    .cloned()
                else {
                    self.status_notice = Some("No agent providers available".to_string());
                    return;
                };
                if !provider.available {
                    self.status_notice = Some(unavailable_provider_notice(&provider));
                    return;
                }
                let picker = self.agent_picker.take().expect("checked");
                let target_id = if picker.target_thread_id.is_empty() {
                    self.active_thread_id.clone()
                } else {
                    picker.target_thread_id
                };
                self.spawn_sibling_with_provider(&target_id, &provider, None);
            }
            (KeyCode::Char('s' | 'S'), _) if no_ctrl_alt => {
                let Some(idx) = self.agent_picker.as_ref().map(|p| p.selected_idx) else {
                    return;
                };
                let Some(provider) = self
                    .agent_providers
                    .get(idx)
                    .or_else(|| self.agent_providers.first())
                    .cloned()
                else {
                    self.status_notice = Some("No agent providers available".to_string());
                    return;
                };
                if !provider.available {
                    self.status_notice = Some(unavailable_provider_notice(&provider));
                    return;
                }
                let target_id = self
                    .agent_picker
                    .as_ref()
                    .map(|p| p.target_thread_id.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&self.active_thread_id)
                    .to_string();
                if self.is_terminal_thread(&target_id) {
                    self.status_notice =
                        Some("Cannot switch agent provider on a shell thread".to_string());
                    return;
                }
                self.agent_picker = None;
                self.status_notice = Some(format!("Switching agent to {}...", provider.display_name));
                self.pending_actions
                    .push_back(AppAction::SwitchThreadProvider {
                        thread_id: target_id,
                        provider: provider.id,
                    });
            }
            (KeyCode::Char('d' | 'D'), _) if no_ctrl_alt => {
                let Some(idx) = self.agent_picker.as_ref().map(|p| p.selected_idx) else {
                    return;
                };
                let Some(provider) = self
                    .agent_providers
                    .get(idx)
                    .or_else(|| self.agent_providers.first())
                    .cloned()
                else {
                    self.status_notice = Some("No agent providers available".to_string());
                    return;
                };
                if !provider.available {
                    self.status_notice = Some(unavailable_provider_notice(&provider));
                    return;
                }
                let id = provider.id.clone();
                let name = provider.display_name.clone();
                self.default_agent_provider = Some(id.clone());
                for p in &mut self.agent_providers {
                    p.is_default = p.id == id;
                }
                self.status_notice = Some(format!("Default agent provider: {name}"));
                self.pending_actions
                    .push_back(AppAction::SaveDefaultAgentProvider { provider: id });
            }
            (KeyCode::Esc | KeyCode::Char('q' | 'Q'), _) if no_ctrl_alt => {
                self.agent_picker = None;
            }
            (KeyCode::Char('c' | 'C'), _) if ctrl && !alt => {
                self.agent_picker = None;
            }
            _ => {}
        }
    }
}
