use crate::autocomplete::{
    compute_completions, extract_history_words, extract_transcript_identifiers, AutocompleteContext,
};

use super::editing::clamp_char_boundary;
use super::types::{App, AppAction, FocusedPane};

const STATIC_SLASH_COMMANDS: &[&str] = &[
    "/theme",
    "/agent",
    "/sidebar",
    "/keymap",
    "/bind",
    "/unbind",
    "/keybinds",
    "/zoom",
    "/help",
    "/quit",
    "/root",
    "/fork",
    "/rename",
    "/join",
    "/delete",
    "/term",
    "/worktree",
    "/wt",
    "/worktrees",
    "/prune",
    "/autocomplete",
    "/model",
    "/mode",
    "/thinking",
    "/commands",
    "/copy",
];

impl App {
    pub(crate) fn build_autocomplete_context(&self) -> AutocompleteContext {
        let mut slash_commands: Vec<String> = STATIC_SLASH_COMMANDS
            .iter()
            .map(|&s| s.to_string())
            .collect();
        if let Some(cmds) = self.thread_commands.get(&self.active_thread_id) {
            for cmd in cmds {
                if !cmd.name.trim().is_empty() {
                    slash_commands.push(cmd.name.clone());
                }
            }
        }

        let mut git_branches: Vec<String> = self
            .worktrees
            .iter()
            .filter_map(|w| w.branch.clone())
            .filter(|b| !b.trim().is_empty())
            .collect();
        if let Some(branch) = self
            .thread_meta
            .get(&self.active_thread_id)
            .and_then(|m| m.git_branch.as_deref())
            .map(str::trim)
            .filter(|b| !b.is_empty())
        {
            if !git_branches.iter().any(|b| b == branch) {
                git_branches.push(branch.to_string());
            }
        }

        let transcript_text = if !self.active_transcript_md.is_empty() {
            self.active_transcript_md.as_str()
        } else {
            self.thread_transcripts
                .get(&self.active_thread_id)
                .map(String::as_str)
                .unwrap_or("")
        };
        let transcript_slice = if transcript_text.len() > 256 * 1024 {
            let start = clamp_char_boundary(
                transcript_text,
                transcript_text.len().saturating_sub(256 * 1024),
            );
            &transcript_text[start..]
        } else {
            transcript_text
        };
        let transcript_tokens = if transcript_slice.is_empty() {
            Vec::new()
        } else {
            extract_transcript_identifiers(&[transcript_slice], 200)
        };

        let chronological_prompts: Vec<String> = self
            .autocomplete_recent_prompts
            .iter()
            .rev()
            .cloned()
            .collect();
        let history_words = extract_history_words(&chronological_prompts, 200);

        AutocompleteContext {
            slash_commands,
            workspace_files: self.autocomplete_workspace_files.clone(),
            git_branches,
            transcript_tokens,
            recent_prompts: self.autocomplete_recent_prompts.clone(),
            history_words,
        }
    }

    pub fn sync_composer_completion(&mut self) {
        if !self.composer_autocomplete_enabled
            || self.composer_completion_dismissed
            || self.worktree_autocomplete_open
            || self.focused_pane != FocusedPane::Composer
            || self.has_active_modal()
        {
            self.composer_completion = None;
            return;
        }
        let ctx = self.build_autocomplete_context();
        self.composer_completion =
            compute_completions(&self.input_buffer, self.cursor_position, &ctx);
    }

    pub fn accept_composer_completion(&mut self) -> bool {
        let Some(suffix) = self
            .composer_completion
            .as_ref()
            .and_then(|ac| ac.active())
            .map(|s| s.insert_suffix.clone())
        else {
            return false;
        };
        let pos = clamp_char_boundary(&self.input_buffer, self.cursor_position);
        self.input_buffer.insert_str(pos, &suffix);
        self.cursor_position = pos + suffix.len();
        self.composer_desired_col = None;
        self.composer_completion_dismissed = false;
        self.status_notice = None;
        self.sync_worktree_autocomplete(true);
        self.sync_composer_completion();
        true
    }

    pub(super) fn execute_autocomplete_command(&mut self, rest: &str) {
        let arg = rest.trim().to_ascii_lowercase();
        let target = match arg.as_str() {
            "" => Some(!self.composer_autocomplete_enabled),
            "status" => {
                let state = if self.composer_autocomplete_enabled {
                    "on"
                } else {
                    "off"
                };
                self.status_notice = Some(format!("Composer autocomplete: {state}"));
                None
            }
            "on" | "true" | "1" | "yes" => Some(true),
            "off" | "false" | "0" | "no" => Some(false),
            _ => {
                self.status_notice = Some("Usage: /autocomplete [on|off|status]".to_string());
                None
            }
        };
        if let Some(enabled) = target {
            self.composer_autocomplete_enabled = enabled;
            if enabled {
                self.composer_completion_dismissed = false;
                self.sync_composer_completion();
            } else {
                self.composer_completion = None;
            }
            let val = if enabled { "on" } else { "off" };
            self.pending_actions.push_back(AppAction::SaveUiPreference {
                key: "composer_autocomplete".to_string(),
                value: val.to_string(),
            });
            self.status_notice = Some(format!("Composer autocomplete: {val}"));
        }
    }
}
