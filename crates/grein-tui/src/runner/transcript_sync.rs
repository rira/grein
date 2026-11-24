use std::time::{SystemTime, UNIX_EPOCH};

use grein_core::model::{AuthorRole, ThreadId};

use crate::app::{AnchorItem, App, AppAction, ThreadItem};
use crate::engine_port::{AgentSlashCommand, HydratedTurn};
use crate::transcript::{ThreadTranscript, TranscriptBlock};

use super::tree_order::rebuild_tree_nodes;

pub(super) const USER_PREFIX: &str = "# User: ";
pub(super) const AGENT_HEADER: &str = "## Agent (agy)";
pub(super) const ASSISTANT_HEADER: &str = "## Assistant";

/// Structured view of the transcript: ordered (role, content) turns.
pub(super) fn collect_turns(transcript: &str) -> Vec<(AuthorRole, String)> {
    let mut turns: Vec<(AuthorRole, String)> = Vec::new();
    let mut current: Option<(AuthorRole, String)> = None;

    for line in transcript.lines() {
        if let Some(rest) = line.strip_prefix(USER_PREFIX) {
            if let Some((role, content)) = current.take() {
                turns.push((role, content.trim().to_string()));
            }
            current = Some((AuthorRole::User, rest.to_string()));
        } else if line == AGENT_HEADER || line == ASSISTANT_HEADER {
            if let Some((role, content)) = current.take() {
                turns.push((role, content.trim().to_string()));
            }
            current = Some((AuthorRole::Assistant, String::new()));
        } else if let Some((_, content)) = current.as_mut() {
            if !content.is_empty() {
                content.push('\n');
            }
            content.push_str(line);
        }
    }
    if let Some((role, content)) = current.take() {
        turns.push((role, content.trim().to_string()));
    }
    turns
}

pub(super) fn render_node(role: &AuthorRole, content: &str) -> String {
    match role {
        AuthorRole::User => format!("{USER_PREFIX}{content}\n\n"),
        _ => format!("{AGENT_HEADER}\n{content}\n\n"),
    }
}

pub(super) fn ensure_paragraph_break(buf: &mut String) {
    if !buf.is_empty() {
        if !buf.ends_with('\n') {
            buf.push('\n');
        }
        if !buf.ends_with("\n\n") {
            buf.push('\n');
        }
    }
}

pub(super) fn strip_trailing_shell_stub(buf: &mut String, command: &str) {
    let trimmed_end = buf.trim_end();
    let stub = format!("> $ {command}");
    if let Some(prefix) = trimmed_end.strip_suffix(&stub) {
        if prefix.is_empty() || prefix.ends_with('\n') {
            let clean_len = prefix.trim_end_matches(['\r', '\n']).len();
            buf.truncate(clean_len);
        }
    }
}

pub fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub(super) fn parse_anchors_for_transcript(transcript: &str) -> Vec<AnchorItem> {
    grein_core::anchor::parse_anchors(transcript)
        .into_iter()
        .filter(|a| {
            let raw = transcript
                .get(a.byte_range.clone())
                .map(str::trim)
                .unwrap_or("");
            !raw.starts_with(USER_PREFIX) && raw != AGENT_HEADER && raw != ASSISTANT_HEADER
        })
        .map(|a| AnchorItem {
            id: a.slug,
            label: a.title,
        })
        .collect()
}

pub(super) fn with_thread_transcript_mut<F>(app: &mut App, thread_id: &str, f: F)
where
    F: FnOnce(&mut String),
{
    if thread_id == app.active_thread_id {
        f(&mut app.active_transcript_md);
        app.thread_transcripts
            .insert(thread_id.to_string(), app.active_transcript_md.clone());
    } else {
        let entry = app
            .thread_transcripts
            .entry(thread_id.to_string())
            .or_default();
        f(entry);
    }
}

pub(super) fn sync_thread_blocks_md(app: &mut App, thread_id: &str) {
    let md = if thread_id == app.active_thread_id {
        app.active_transcript_md.clone()
    } else {
        app.thread_transcripts
            .get(thread_id)
            .cloned()
            .unwrap_or_default()
    };
    app.thread_blocks_synced_md
        .insert(thread_id.to_string(), md);
}

pub(super) fn build_blocks_from_turns(turns: &[HydratedTurn]) -> ThreadTranscript {
    let mut blocks = ThreadTranscript::new();
    for turn in turns {
        if turn.role.eq_ignore_ascii_case("user") {
            blocks.push_user(&turn.content);
        } else {
            blocks.blocks.push(TranscriptBlock::AssistantMarkdown {
                content: turn.content.clone(),
            });
        }
    }
    blocks
}

pub(super) fn format_tool_result(tool_name: &str, output: &str) -> String {
    let trimmed = output.trim_end_matches(['\r', '\n']);
    if trimmed.trim().is_empty() {
        format!("> ✓ `{tool_name}`\n")
    } else if !trimmed.contains('\n') {
        format!("> ✓ `{tool_name}`: {}\n", trimmed.trim())
    } else {
        format!("> ✓ `{tool_name}`:\n```text\n{trimmed}\n```\n")
    }
}

impl App {
    pub(crate) fn record_agent_commands_note(
        &mut self,
        thread_id: &str,
        commands: &[AgentSlashCommand],
    ) {
        if commands.is_empty() {
            self.status_notice = Some("No agent commands advertised".to_string());
            return;
        }
        let mut names = Vec::with_capacity(commands.len());
        let mut lines = Vec::with_capacity(commands.len());
        for cmd in commands {
            let cmd_name = if cmd.name.starts_with('/') {
                cmd.name.clone()
            } else {
                format!("/{}", cmd.name)
            };
            names.push(cmd_name.clone());
            if cmd.description.trim().is_empty() {
                lines.push(cmd_name);
            } else {
                lines.push(format!("{cmd_name} — {}", cmd.description.trim()));
            }
        }
        let note = format!("Agent commands:\n{}", lines.join("\n"));
        let md_note = lines
            .iter()
            .map(|l| format!("> ↳ {l}"))
            .collect::<Vec<_>>()
            .join("\n");
        with_thread_transcript_mut(self, thread_id, |buf| {
            ensure_paragraph_break(buf);
            buf.push_str(&md_note);
            buf.push_str("\n\n");
        });
        if thread_id == self.active_thread_id {
            self.sync_anchors();
        }
        let resolved_id = if thread_id == "trunk" && self.active_thread_id != "trunk" {
            self.active_thread_id.clone()
        } else {
            thread_id.to_string()
        };
        self.thread_blocks
            .entry(resolved_id.clone())
            .or_default()
            .push_join_note(note);
        sync_thread_blocks_md(self, &resolved_id);
        self.status_notice = Some(format!("Commands: {}", names.join(", ")));
    }

    pub fn submit_composer_prompt(&mut self, prompt: impl Into<String>) {
        self.worktree_autocomplete_open = false;
        self.worktree_autocomplete_idx = 0;
        self.composer_completion = None;
        self.composer_completion_dismissed = false;
        let prompt = prompt.into();
        let trimmed = prompt.trim();
        if trimmed.is_empty() {
            return;
        }

        if let Some(rest) = trimmed.strip_prefix('!') {
            let command = rest.trim();
            if !command.is_empty() {
                ensure_paragraph_break(&mut self.active_transcript_md);
                self.active_transcript_md
                    .push_str(&format!("> $ {command}\n\n"));
                self.sync_anchors();
                let active_id = self.active_thread_id.clone();
                self.thread_blocks
                    .entry(active_id.clone())
                    .or_default()
                    .push_shell_pending(command);
                sync_thread_blocks_md(self, &active_id);
                self.transcript_viewport_mut(&active_id).scroll_to_bottom();
                self.status_notice = None;
                self.thread_active_tool.remove(&active_id);
                self.thread_turn_started_ms
                    .insert(active_id.clone(), now_epoch_ms());
                self.thread_statuses
                    .insert(active_id.clone(), "executing".to_string());
                self.pending_actions.push_back(AppAction::RunShell {
                    thread_id: active_id,
                    command: command.to_string(),
                });
            }
            return;
        }

        if trimmed.starts_with('/') && self.execute_slash_command(trimmed, false) {
            return;
        }

        ensure_paragraph_break(&mut self.active_transcript_md);
        self.active_transcript_md
            .push_str(&render_node(&AuthorRole::User, trimmed));
        self.last_submitted_prompt = Some(trimmed.to_string());
        self.autocomplete_recent_prompts.retain(|p| p != trimmed);
        self.autocomplete_recent_prompts.insert(0, trimmed.to_string());
        if self.autocomplete_recent_prompts.len() > 200 {
            self.autocomplete_recent_prompts.truncate(200);
        }
        self.sync_anchors();
        let active_id = self.active_thread_id.clone();
        self.thread_blocks
            .entry(active_id.clone())
            .or_default()
            .push_user(trimmed);
        sync_thread_blocks_md(self, &active_id);
        self.transcript_viewport_mut(&active_id).scroll_to_bottom();
        self.status_notice = None;
        self.thread_active_tool.remove(&active_id);
        self.thread_turn_started_ms
            .insert(active_id.clone(), now_epoch_ms());
        self.thread_statuses
            .insert(active_id.clone(), "streaming".to_string());
        self.pending_actions.push_back(AppAction::SendPrompt {
            thread_id: active_id,
            prompt: trimmed.to_string(),
        });
    }

    pub fn apply_agent_response(&mut self, response: &str) {
        let clean = response.trim_end_matches('\n');
        let last_user = self.active_transcript_md.rfind(USER_PREFIX);
        let agent_marker = format!("{AGENT_HEADER}\n");
        let last_agent = self.active_transcript_md.rfind(agent_marker.as_str());
        let has_open_agent_turn = match (last_user, last_agent) {
            (Some(u), Some(a)) => a > u,
            (None, Some(_)) => true,
            _ => false,
        };
        if has_open_agent_turn {
            let trimmed_len = self.active_transcript_md.trim_end_matches('\n').len();
            self.active_transcript_md.truncate(trimmed_len);
            self.active_transcript_md.push('\n');
            self.active_transcript_md.push_str(clean);
            self.active_transcript_md.push_str("\n\n");
        } else {
            ensure_paragraph_break(&mut self.active_transcript_md);
            self.active_transcript_md
                .push_str(&render_node(&AuthorRole::Assistant, clean));
        }
        self.sync_anchors();
        let active_id = self.active_thread_id.clone();
        let blocks = self.thread_blocks.entry(active_id.clone()).or_default();
        if has_open_agent_turn {
            if let Some(TranscriptBlock::AssistantMarkdown { content }) = blocks.blocks.last_mut() {
                if !content.is_empty() && !clean.is_empty() {
                    content.push('\n');
                }
                content.push_str(clean);
            } else {
                blocks.append_assistant_chunk(clean);
            }
        } else {
            blocks.append_assistant_chunk(clean);
        }
        sync_thread_blocks_md(self, &active_id);
    }

    pub fn sync_anchors(&mut self) {
        let turns = collect_turns(&self.active_transcript_md);
        if self.threads.is_empty() && !turns.is_empty() {
            let title = turns
                .iter()
                .find(|(role, _)| *role == AuthorRole::User)
                .map(|(_, content)| {
                    content
                        .lines()
                        .next()
                        .unwrap_or("Active Session")
                        .to_string()
                })
                .unwrap_or_else(|| "Active Session".to_string());
            let old_id = self.active_thread_id.clone();
            let new_id = if self.active_thread_id.is_empty() || self.active_thread_id == "trunk" {
                let id = ThreadId::now_v7().to_string();
                self.active_thread_id = id.clone();
                id
            } else {
                self.active_thread_id.clone()
            };
            if old_id != new_id {
                if let Some(prev_blocks) = self.thread_blocks.remove(&old_id) {
                    self.thread_blocks.insert(new_id.clone(), prev_blocks);
                }
                if let Some(prev_synced) = self.thread_blocks_synced_md.remove(&old_id) {
                    self.thread_blocks_synced_md
                        .insert(new_id.clone(), prev_synced);
                }
                if let Some(prev_vp) = self.thread_viewports.remove(&old_id) {
                    self.thread_viewports.insert(new_id.clone(), prev_vp);
                }
                self.thread_transcripts.remove(&old_id);
            }
            self.threads.push(ThreadItem {
                id: new_id,
                title,
                custom_name: None,
                anchors: Vec::new(),
            });
        }

        self.thread_transcripts.insert(
            self.active_thread_id.clone(),
            self.active_transcript_md.clone(),
        );

        let anchor_items = parse_anchors_for_transcript(&self.active_transcript_md);
        if let Some(thread) = self
            .threads
            .iter_mut()
            .find(|t| t.id == self.active_thread_id)
        {
            thread.anchors = anchor_items;
        } else if let Some(thread) = self.threads.first_mut() {
            thread.anchors = anchor_items;
        }

        rebuild_tree_nodes(self);
    }
}
