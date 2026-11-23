use grein_core::model::AuthorRole;

use crate::app::{
    find_config_option, normalize_custom_name, App, FocusedPane, PendingPermission, SidebarView,
    ThreadItem,
};
use crate::engine_port::{HydratedSession, HydratedTurn, ThreadMeta, TuiEngineEvent};

use super::transcript_sync::{
    build_blocks_from_turns, ensure_paragraph_break, format_tool_result, now_epoch_ms,
    parse_anchors_for_transcript, render_node, strip_trailing_shell_stub, sync_thread_blocks_md,
    with_thread_transcript_mut, ASSISTANT_HEADER,
};
use super::tree_order::rebuild_tree_nodes;

fn render_turns_md(turns: &[HydratedTurn]) -> String {
    let mut transcript = String::new();
    for turn in turns {
        let role = if turn.role.eq_ignore_ascii_case("user") {
            AuthorRole::User
        } else {
            AuthorRole::Assistant
        };
        transcript.push_str(&render_node(&role, &turn.content));
    }
    transcript
}

/// Seeds the app (threads, transcript nodes) from a hydrated session.
pub fn apply_hydrated_session(app: &mut App, session: &HydratedSession) {
    app.threads.clear();
    app.thread_transcripts.clear();
    app.thread_blocks.clear();
    app.thread_blocks_synced_md.clear();
    app.thread_statuses.clear();
    app.streaming_assistant_threads.clear();
    app.thread_thoughts.clear();
    app.thread_meta.clear();
    app.thread_subagents.clear();
    app.thread_agent_meta.clear();
    app.thread_config_options.clear();
    app.thread_commands.clear();
    app.thread_active_tool.clear();
    app.thread_turn_started_ms.clear();
    app.pty_screens.clear();
    app.pty_exit_codes.clear();
    app.last_pty_sizes.clear();
    app.config_picker = None;
    app.theme_picker = None;
    app.pending_worktree_delete = None;
    app.pending_thread_delete = None;
    app.pending_quit_confirm = None;
    app.rename_prompt = None;
    app.palette_active = false;
    app.palette_input.clear();
    app.palette_cursor = 0;
    app.palette_match_idx = 0;
    app.worktree_autocomplete_open = false;
    app.worktree_autocomplete_idx = 0;
    app.worktree_list_in_flight = false;
    app.worktree_filter_active = false;
    app.worktree_filter_query.clear();
    app.worktrees.clear();
    app.worktree_selected_idx = 0;
    app.worktree_scroll_offset = 0;
    app.worktree_diff_path = None;
    app.worktree_diff_preview.clear();
    app.worktree_diff_scroll = 0;
    app.worktree_diff_max_scroll = 0;
    app.worktree_diff_in_flight = None;
    app.collapsed_worktree_repos.clear();
    app.sidebar_view = SidebarView::Grein;
    app.help_scroll_offset = 0;
    app.help_max_scroll = 0;
    app.status_notice = None;
    app.pending_rename_thread_id = None;
    app.transcript_search.clear();
    app.pending_g_prefix = false;
    app.collapsed_threads.clear();
    app.tree_has_children.clear();
    app.tree_hidden_counts.clear();

    for t in &session.threads {
        let transcript = render_turns_md(&t.turns);
        let blocks = build_blocks_from_turns(&t.turns);
        let anchors = parse_anchors_for_transcript(&transcript);
        app.thread_blocks_synced_md
            .insert(t.id.clone(), transcript.clone());
        app.thread_blocks.insert(t.id.clone(), blocks);
        app.thread_transcripts.insert(t.id.clone(), transcript);
        app.thread_statuses
            .entry(t.id.clone())
            .or_insert_with(|| "idle".to_string());
        app.threads.push(ThreadItem {
            id: t.id.clone(),
            title: t.title.clone(),
            custom_name: normalize_custom_name(t.custom_name.as_deref()),
            anchors,
        });
    }

    let active_id = if !session.active_thread_id.is_empty()
        && session.threads.iter().any(|t| t.id == session.active_thread_id)
    {
        session.active_thread_id.clone()
    } else if let Some(first) = session.threads.first() {
        first.id.clone()
    } else {
        session.active_thread_id.clone()
    };

    if !active_id.is_empty() {
        app.active_thread_id = active_id;
    }
    app.active_transcript_md = app
        .thread_transcripts
        .get(&app.active_thread_id)
        .cloned()
        .unwrap_or_default();

    app.sync_anchors();
    app.select_active_thread_in_tree();
}

/// Folds a streamed engine event into the app state.
pub fn apply_engine_event(app: &mut App, event: TuiEngineEvent) {
    match event {
        TuiEngineEvent::StreamChunk { thread_id, content } => {
            app.thread_thoughts.remove(&thread_id);
            app.thread_turn_started_ms
                .entry(thread_id.clone())
                .or_insert_with(now_epoch_ms);
            app.thread_statuses
                .insert(thread_id.clone(), "streaming".to_string());
            let is_first = app.streaming_assistant_threads.insert(thread_id.clone());
            with_thread_transcript_mut(app, &thread_id, |buf| {
                if is_first {
                    ensure_paragraph_break(buf);
                    buf.push_str(ASSISTANT_HEADER);
                    buf.push('\n');
                }
                buf.push_str(&content);
            });
            app.thread_blocks
                .entry(thread_id.clone())
                .or_default()
                .append_assistant_chunk(&content);
            sync_thread_blocks_md(app, &thread_id);
        }
        TuiEngineEvent::ThoughtChunk { thread_id, content } => {
            app.thread_turn_started_ms
                .entry(thread_id.clone())
                .or_insert_with(now_epoch_ms);
            let is_executing = app
                .thread_statuses
                .get(&thread_id)
                .map(String::as_str)
                == Some("executing");
            if !is_executing {
                app.thread_statuses
                    .insert(thread_id.clone(), "thinking".to_string());
            }
            app.thread_thoughts
                .entry(thread_id)
                .or_default()
                .push_str(&content);
        }
        TuiEngineEvent::ToolInvoked {
            thread_id,
            tool_name,
            tool_call_id,
            summary,
        } => {
            app.thread_thoughts.remove(&thread_id);
            app.thread_turn_started_ms
                .entry(thread_id.clone())
                .or_insert_with(now_epoch_ms);
            app.thread_active_tool
                .insert(thread_id.clone(), Some(tool_name.clone()));
            app.thread_statuses
                .insert(thread_id.clone(), "executing".to_string());
            let is_first = app.streaming_assistant_threads.insert(thread_id.clone());
            with_thread_transcript_mut(app, &thread_id, |buf| {
                if is_first {
                    ensure_paragraph_break(buf);
                    buf.push_str(ASSISTANT_HEADER);
                    buf.push('\n');
                } else if !buf.is_empty() && !buf.ends_with('\n') {
                    buf.push('\n');
                }
                buf.push_str(&format!("> ⚙ tool: `{tool_name}`\n"));
            });
            app.thread_blocks
                .entry(thread_id.clone())
                .or_default()
                .start_tool_call(tool_call_id, tool_name, summary);
            sync_thread_blocks_md(app, &thread_id);
        }
        TuiEngineEvent::ToolResult {
            thread_id,
            tool_name,
            output,
            tool_call_id,
            is_error,
        } => {
            app.thread_thoughts.remove(&thread_id);
            app.thread_active_tool.remove(&thread_id);
            if app
                .thread_statuses
                .get(&thread_id)
                .map(String::as_str)
                == Some("executing")
            {
                app.thread_statuses
                    .insert(thread_id.clone(), "streaming".to_string());
            }
            let is_first = app.streaming_assistant_threads.insert(thread_id.clone());
            let formatted = format_tool_result(&tool_name, &output);
            with_thread_transcript_mut(app, &thread_id, |buf| {
                if is_first {
                    ensure_paragraph_break(buf);
                    buf.push_str(ASSISTANT_HEADER);
                    buf.push('\n');
                } else if !buf.is_empty() && !buf.ends_with('\n') {
                    buf.push('\n');
                }
                buf.push_str(&formatted);
            });
            app.thread_blocks
                .entry(thread_id.clone())
                .or_default()
                .complete_tool_call(tool_call_id.as_deref(), &tool_name, output, is_error);
            sync_thread_blocks_md(app, &thread_id);
        }
        TuiEngineEvent::ToolActivityUpdated {
            thread_id,
            tool_name,
        } => {
            app.thread_thoughts.remove(&thread_id);
            if let Some(name) = tool_name {
                app.thread_turn_started_ms
                    .entry(thread_id.clone())
                    .or_insert_with(now_epoch_ms);
                app.thread_active_tool.insert(thread_id.clone(), Some(name));
                app.thread_statuses
                    .insert(thread_id, "executing".to_string());
            } else {
                app.thread_active_tool.remove(&thread_id);
                if app
                    .thread_statuses
                    .get(&thread_id)
                    .map(String::as_str)
                    == Some("executing")
                {
                    app.thread_statuses
                        .insert(thread_id, "streaming".to_string());
                }
            }
        }
        TuiEngineEvent::ShellOutput {
            thread_id,
            command,
            stdout,
            stderr,
            exit_code,
        } => {
            let mut body = String::new();
            if !stdout.is_empty() {
                body.push_str(stdout.trim_end_matches(['\r', '\n']));
            }
            if !stderr.is_empty() {
                let clean_stderr = stderr.trim_end_matches(['\r', '\n']);
                if !clean_stderr.is_empty() {
                    if !body.is_empty() {
                        body.push('\n');
                    }
                    body.push_str("[stderr]\n");
                    body.push_str(clean_stderr);
                }
            }
            let block = if body.is_empty() {
                format!("$ {command} (exit {exit_code})\n\n")
            } else {
                format!("$ {command} (exit {exit_code})\n```ansi\n{body}\n```\n\n")
            };
            with_thread_transcript_mut(app, &thread_id, |buf| {
                strip_trailing_shell_stub(buf, &command);
                ensure_paragraph_break(buf);
                buf.push_str(&block);
            });
            app.thread_blocks
                .entry(thread_id.clone())
                .or_default()
                .complete_shell(&command, stdout, stderr, exit_code);
            sync_thread_blocks_md(app, &thread_id);
            app.thread_active_tool.remove(&thread_id);
            app.thread_turn_started_ms.remove(&thread_id);
            app.thread_statuses.insert(thread_id.clone(), "idle".to_string());
            if thread_id == app.active_thread_id {
                app.sync_anchors();
            }
        }
        TuiEngineEvent::ThreadCreated { thread } => {
            app.rename_prompt = None;
            let incoming_custom = normalize_custom_name(thread.custom_name.as_deref());
            if let Some(existing) = app.threads.iter_mut().find(|t| t.id == thread.id) {
                if !thread.title.trim().is_empty() {
                    existing.title = thread.title;
                }
                if incoming_custom.is_some() {
                    existing.custom_name = incoming_custom;
                }
                rebuild_tree_nodes(app);
                return;
            }
            let transcript = render_turns_md(&thread.turns);
            let blocks = build_blocks_from_turns(&thread.turns);
            let anchors = parse_anchors_for_transcript(&transcript);
            let new_id = thread.id.clone();
            app.thread_blocks_synced_md
                .insert(new_id.clone(), transcript.clone());
            app.thread_blocks.insert(new_id.clone(), blocks);
            app.thread_transcripts.insert(new_id.clone(), transcript);
            app.thread_statuses
                .entry(new_id.clone())
                .or_insert_with(|| "idle".to_string());
            app.threads.push(ThreadItem {
                id: new_id.clone(),
                title: thread.title,
                custom_name: incoming_custom,
                anchors,
            });
            app.switch_thread(&new_id);
            if app.is_terminal_thread(&new_id) {
                app.set_focus(FocusedPane::Terminal);
                app.maybe_request_pty_snapshot(&new_id);
            } else {
                app.set_focus(FocusedPane::Composer);
            }
        }
        TuiEngineEvent::ThreadTitleUpdated { thread_id, title } => {
            if let Some(thread) = app.threads.iter_mut().find(|t| t.id == thread_id) {
                thread.title = title;
            }
            rebuild_tree_nodes(app);
        }
        TuiEngineEvent::ThreadCustomNameUpdated {
            thread_id,
            custom_name,
        } => {
            let normalized = normalize_custom_name(custom_name.as_deref());
            if let Some(thread) = app.threads.iter_mut().find(|t| t.id == thread_id) {
                thread.custom_name = normalized.clone();
            }
            if let Some(meta) = app.thread_meta.get_mut(&thread_id) {
                meta.custom_name = normalized;
            }
            rebuild_tree_nodes(app);
        }
        TuiEngineEvent::ThreadMerged {
            thread_id,
            target_thread_id,
            summary,
        } => {
            let has_target = !target_thread_id.is_empty() && target_thread_id != thread_id;
            app.thread_meta
                .entry(thread_id.clone())
                .and_modify(|m| m.merged = true)
                .or_insert_with(|| ThreadMeta {
                    thread_id: thread_id.clone(),
                    parent_thread_id: has_target.then(|| target_thread_id.clone()),
                    inherited_turn_count: 0,
                    merged: true,
                    kind: grein_core::ThreadKind::Agent,
                    cwd: None,
                    git_branch: None,
                    git_dirty: 0,
                    custom_name: None,
                });
            let source_title = app.display_title_for(&thread_id, &thread_id);
            if has_target {
                let clean_summary = summary.trim();
                let join_note = if clean_summary.is_empty() {
                    format!("Joined from {source_title}")
                } else {
                    format!("Joined from {source_title}: {clean_summary}")
                };
                let note = format!("> ↳ {join_note}\n\n");
                with_thread_transcript_mut(app, &target_thread_id, |buf| {
                    ensure_paragraph_break(buf);
                    buf.push_str(&note);
                });
                app.thread_blocks
                    .entry(target_thread_id.clone())
                    .or_default()
                    .push_join_note(join_note);
                sync_thread_blocks_md(app, &target_thread_id);
                if target_thread_id == app.active_thread_id {
                    app.sync_anchors();
                }
                let target_title = app.display_title_for(&target_thread_id, &target_thread_id);
                app.status_notice = Some(format!("Joined {source_title} → {target_title}"));
            } else {
                app.status_notice = Some(format!("Joined {source_title} to Shared Context"));
            }
            rebuild_tree_nodes(app);
        }
        TuiEngineEvent::ThreadMetadataUpdated { meta } => {
            app.thread_meta.clear();
            for m in meta {
                if let Some(custom) = normalize_custom_name(m.custom_name.as_deref()) {
                    if let Some(t) = app.threads.iter_mut().find(|t| t.id == m.thread_id) {
                        t.custom_name = Some(custom);
                    }
                }
                app.thread_meta.insert(m.thread_id.clone(), m);
            }
            let active = app.active_thread_id.clone();
            app.unfold_ancestors_for(&active);
            rebuild_tree_nodes(app);
            app.sync_focus_for_active_thread();
            app.maybe_request_pty_snapshot(&active);
            app.maybe_queue_terminal_resize();
        }
        TuiEngineEvent::ThreadDeleted {
            thread_id,
            next_active_thread_id,
        } => {
            app.threads.retain(|t| t.id != thread_id);
            app.thread_transcripts.remove(&thread_id);
            app.thread_blocks.remove(&thread_id);
            app.thread_blocks_synced_md.remove(&thread_id);
            app.thread_statuses.remove(&thread_id);
            app.streaming_assistant_threads.remove(&thread_id);
            app.thread_thoughts.remove(&thread_id);
            app.thread_viewports.remove(&thread_id);
            app.thread_drafts.remove(&thread_id);
            app.thread_meta.remove(&thread_id);
            app.thread_subagents.remove(&thread_id);
            app.thread_agent_meta.remove(&thread_id);
            app.thread_config_options.remove(&thread_id);
            app.thread_commands.remove(&thread_id);
            app.thread_active_tool.remove(&thread_id);
            app.thread_turn_started_ms.remove(&thread_id);
            app.pty_screens.remove(&thread_id);
            app.pty_exit_codes.remove(&thread_id);
            app.last_pty_sizes.remove(&thread_id);
            app.collapsed_threads.remove(&thread_id);
            app.tree_has_children.remove(&thread_id);
            app.tree_hidden_counts.remove(&thread_id);
            if app
                .config_picker
                .as_ref()
                .is_some_and(|p| p.thread_id == thread_id)
            {
                app.config_picker = None;
            }
            app.status_notice = None;
            app.pending_rename_thread_id = None;

            if app.active_thread_id == thread_id
                || !app.threads.iter().any(|t| t.id == app.active_thread_id)
            {
                let target = if !next_active_thread_id.is_empty()
                    && app.threads.iter().any(|t| t.id == next_active_thread_id)
                {
                    next_active_thread_id
                } else {
                    app.threads
                        .first()
                        .map(|t| t.id.clone())
                        .unwrap_or_default()
                };
                app.active_thread_id = target;
                app.input_buffer = app
                    .thread_drafts
                    .get(&app.active_thread_id)
                    .cloned()
                    .unwrap_or_default();
                app.cursor_position = app.input_buffer.len();
                app.active_transcript_md = app
                    .thread_transcripts
                    .get(&app.active_thread_id)
                    .cloned()
                    .unwrap_or_default();
                app.thread_viewports
                    .entry(app.active_thread_id.clone())
                    .or_default();
            }

            let active_for_unfold = app.active_thread_id.clone();
            app.unfold_ancestors_for(&active_for_unfold);
            rebuild_tree_nodes(app);
            app.select_active_thread_in_tree();
            app.sync_focus_for_active_thread();
            let active = app.active_thread_id.clone();
            app.maybe_request_pty_snapshot(&active);
        }
        TuiEngineEvent::AgentMetaUpdated {
            thread_id,
            model,
            mode,
        } => {
            let entry = app.thread_agent_meta.entry(thread_id).or_default();
            if model.is_some() {
                entry.0 = model;
            }
            if mode.is_some() {
                entry.1 = mode;
            }
        }
        TuiEngineEvent::AgentConfigUpdated {
            thread_id,
            config_options,
        } => {
            app.sync_agent_meta_from_config_options(&thread_id, &config_options);
            if let Some(ref mut picker) = app.config_picker {
                if picker.thread_id == thread_id {
                    if let Some(opt) = find_config_option(&config_options, &picker.config_id) {
                        picker.current_value = opt.current_value.clone();
                        if !opt.options.is_empty() {
                            picker.options = opt.options.clone();
                            picker.selected_index =
                                picker.selected_index.min(picker.options.len() - 1);
                        }
                    }
                }
            }
            app.thread_config_options.insert(thread_id, config_options);
        }
        TuiEngineEvent::AgentCommandsUpdated {
            thread_id,
            commands,
        } => {
            app.thread_commands.insert(thread_id, commands);
        }
        TuiEngineEvent::SubagentUpdated {
            thread_id,
            subagent,
        } => {
            let list = app.thread_subagents.entry(thread_id).or_default();
            if let Some(existing) = list.iter_mut().find(|s| s.id == subagent.id) {
                *existing = subagent;
            } else {
                list.push(subagent);
            }
        }
        TuiEngineEvent::SubagentRemoved {
            thread_id,
            subagent_id,
        } => {
            if let Some(list) = app.thread_subagents.get_mut(&thread_id) {
                list.retain(|s| s.id != subagent_id);
                if list.is_empty() {
                    app.thread_subagents.remove(&thread_id);
                }
            }
        }
        TuiEngineEvent::TurnCompleted { thread_id } => {
            app.streaming_assistant_threads.remove(&thread_id);
            app.thread_thoughts.remove(&thread_id);
            app.thread_active_tool.remove(&thread_id);
            app.thread_turn_started_ms.remove(&thread_id);
            app.thread_statuses
                .insert(thread_id.clone(), "idle".to_string());
            with_thread_transcript_mut(app, &thread_id, ensure_paragraph_break);
            sync_thread_blocks_md(app, &thread_id);
            if thread_id == app.active_thread_id {
                app.sync_anchors();
            } else {
                let anchors = app
                    .thread_transcripts
                    .get(&thread_id)
                    .map(|t| parse_anchors_for_transcript(t))
                    .unwrap_or_default();
                if let Some(thread) = app.threads.iter_mut().find(|t| t.id == thread_id) {
                    thread.anchors = anchors;
                }
                rebuild_tree_nodes(app);
            }
        }
        TuiEngineEvent::PermissionRequested {
            request_id,
            description,
        } => {
            app.push_permission(PendingPermission {
                request_id,
                description,
            });
        }
        TuiEngineEvent::ThreadStatusChanged { thread_id, status } => {
            if matches!(status.as_str(), "idle" | "error") {
                app.thread_active_tool.remove(&thread_id);
                app.thread_turn_started_ms.remove(&thread_id);
                app.thread_thoughts.remove(&thread_id);
            } else if matches!(status.as_str(), "streaming" | "executing" | "thinking") {
                app.thread_turn_started_ms
                    .entry(thread_id.clone())
                    .or_insert_with(now_epoch_ms);
            }
            app.thread_statuses.insert(thread_id, status);
        }
        TuiEngineEvent::PtyScreenUpdated { thread_id, snapshot } => {
            if snapshot.exited {
                app.pty_exit_codes
                    .insert(thread_id.clone(), snapshot.exit_code);
                let status = if snapshot.exit_code.is_some_and(|c| c != 0) {
                    "error"
                } else {
                    "idle"
                };
                app.thread_statuses
                    .insert(thread_id.clone(), status.to_string());
            } else {
                let was_exited = app.pty_exit_codes.remove(&thread_id).is_some()
                    || app.pty_screens.get(&thread_id).is_some_and(|s| s.exited);
                if was_exited {
                    app.thread_statuses
                        .insert(thread_id.clone(), "idle".to_string());
                }
            }
            app.pty_screens.insert(thread_id, snapshot);
        }
        TuiEngineEvent::PtyExited {
            thread_id,
            exit_code,
        } => {
            app.pty_exit_codes.insert(thread_id.clone(), exit_code);
            if let Some(snap) = app.pty_screens.get_mut(&thread_id) {
                snap.exited = true;
                snap.exit_code = exit_code;
            }
            let status = if exit_code.is_some_and(|c| c != 0) {
                "error"
            } else {
                "idle"
            };
            app.thread_statuses.insert(thread_id, status.to_string());
        }
        TuiEngineEvent::Error { message } => {
            let active = app.active_thread_id.clone();
            app.status_notice = Some(format!("Error: {message}"));
            app.streaming_assistant_threads.remove(&active);
            app.thread_thoughts.remove(&active);
            app.thread_active_tool.remove(&active);
            app.thread_turn_started_ms.remove(&active);
            app.thread_statuses
                .insert(active.clone(), "error".to_string());
            with_thread_transcript_mut(app, &active, |buf| {
                ensure_paragraph_break(buf);
                buf.push_str(&format!("Error: {message}\n\n"));
            });
            app.thread_blocks
                .entry(active.clone())
                .or_default()
                .push_error(message);
            sync_thread_blocks_md(app, &active);
            app.sync_anchors();
        }
    }
}
