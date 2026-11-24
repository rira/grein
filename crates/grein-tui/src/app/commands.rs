use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::keymap::BindableAction;

use super::editing::{clamp_char_boundary, is_altgr_char, prev_word_boundary};
use super::pickers::{find_config_option, match_config_choice};
use super::transcript_ops::{CopyMode, TranscriptScroll};
use super::types::{App, AppAction, FocusedPane, SidebarMode};

pub(crate) fn parse_config_arg(rest: &str) -> (bool, String) {
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        return (false, String::new());
    }
    if let Some(after) = trimmed.strip_prefix("--default") {
        if after.is_empty() || after.starts_with(char::is_whitespace) {
            return (true, after.trim().to_string());
        }
    }
    if let Some(before) = trimmed.strip_suffix("--default") {
        if before.ends_with(char::is_whitespace) {
            return (true, before.trim().to_string());
        }
    }
    let mut split = trimmed.splitn(2, char::is_whitespace);
    if let (Some(first), Some(second)) = (split.next(), split.next()) {
        if first.eq_ignore_ascii_case("default") && !second.trim().is_empty() {
            return (true, second.trim().to_string());
        }
    }
    (false, trimmed.to_string())
}

pub(crate) fn parse_term_cwd(rest: &str) -> Option<String> {
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        return None;
    }
    let unquoted = if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        trimmed[1..trimmed.len() - 1].trim()
    } else {
        trimmed
    };
    (!unquoted.is_empty()).then(|| unquoted.to_string())
}

pub fn palette_command_specs() -> &'static [(&'static str, &'static str)] {
    &[
        ("agent", "Open agent picker or spawn sibling thread"),
        ("theme", "Open theme picker or set theme"),
        ("bind", "Bind WRITE key: :bind <action> <chord>"),
        ("unbind", "Reset/unbind action: :unbind <action>"),
        ("keybinds", "Show live keybindings"),
        ("sidebar", "Configure sidebar width/dynamic/overflow"),
        ("worktree", "Ensure or switch thread worktree"),
        ("worktrees", "Switch to Worktrees sidebar view"),
        ("prune", "Prune stale git worktrees"),
        ("autocomplete", "Toggle inline word autocomplete (on|off)"),
        ("term", "Spawn terminal shell [path]"),
        ("fork", "Fork selected branch"),
        ("new", "Create new root agent thread"),
        ("rename", "Rename thread"),
        ("join", "Join branch into parent"),
        ("copy", "Copy reply/screen/code/all"),
        ("zoom", "Toggle sidebar zoom"),
        ("help", "Open keybindings help"),
        ("quit", "Quit Grein"),
        ("q", "Quit Grein"),
        ("exit", "Quit Grein"),
    ]
}

impl App {
    pub fn open_command_palette(&mut self, initial: &str) {
        self.status_notice = None;
        self.pending_g_prefix = false;
        self.rename_prompt = None;
        self.worktree_filter_active = false;
        self.sidebar_hidden = false;
        self.focused_pane = FocusedPane::Tree;
        self.palette_active = true;
        self.palette_input = initial.to_string();
        self.palette_cursor = self.palette_input.len();
        self.palette_match_idx = 0;
    }

    pub fn palette_suggestions(&self) -> Vec<(String, String)> {
        let trimmed = self
            .palette_input
            .trim_start_matches([':', '/'])
            .trim_start();
        let needle = trimmed
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let mut prefix_matches = Vec::new();
        let mut substring_matches = Vec::new();
        for &(cmd, desc) in palette_command_specs() {
            if needle.is_empty() || cmd.starts_with(&needle) {
                prefix_matches.push((cmd.to_string(), desc.to_string()));
            } else if cmd.contains(&needle) || desc.to_ascii_lowercase().contains(&needle) {
                substring_matches.push((cmd.to_string(), desc.to_string()));
            }
        }
        self.append_provider_palette_suggestions(
            &needle,
            &mut prefix_matches,
            &mut substring_matches,
        );
        prefix_matches.extend(substring_matches);
        prefix_matches
    }

    pub(crate) fn execute_bindable_action(&mut self, action: BindableAction) {
        match action {
            BindableAction::EnterManageMode => {
                if self.focused_pane == FocusedPane::Tree {
                    self.toggle_composer_tree_focus();
                } else {
                    self.focus_tree();
                    if self.sidebar_view == super::types::SidebarView::Worktrees {
                        self.maybe_fetch_selected_worktree_diff(false);
                    }
                }
            }
            BindableAction::ToggleZoom => {
                self.toggle_sidebar_zoom();
            }
            BindableAction::ToggleWorktrees => {
                self.toggle_worktrees_sidebar_view();
            }
            BindableAction::ScrollUp => {
                if self.sidebar_view == super::types::SidebarView::Worktrees
                    && self.focused_pane == FocusedPane::Tree
                {
                    self.worktree_diff_scroll = self.worktree_diff_scroll.saturating_sub(10);
                } else {
                    let target_id = self.transcript_scroll_target_id();
                    if self.is_terminal_thread(&target_id) {
                        self.pending_actions.push_back(AppAction::PtyScroll {
                            thread_id: target_id,
                            delta: 5,
                        });
                    } else {
                        self.scroll_target_transcript(TranscriptScroll::Up(10));
                    }
                }
            }
            BindableAction::ScrollDown => {
                if self.sidebar_view == super::types::SidebarView::Worktrees
                    && self.focused_pane == FocusedPane::Tree
                {
                    self.worktree_diff_scroll = self
                        .worktree_diff_scroll
                        .saturating_add(10)
                        .min(self.worktree_diff_max_scroll);
                } else {
                    let target_id = self.transcript_scroll_target_id();
                    if self.is_terminal_thread(&target_id) {
                        self.pending_actions.push_back(AppAction::PtyScroll {
                            thread_id: target_id,
                            delta: -5,
                        });
                    } else {
                        self.scroll_target_transcript(TranscriptScroll::Down(10));
                    }
                }
            }
            BindableAction::PrevSibling => {
                self.cycle_sibling_thread(false);
            }
            BindableAction::NextSibling => {
                self.cycle_sibling_thread(true);
            }
            BindableAction::TreeUp => {
                self.step_sibling_at_current_depth(false);
            }
            BindableAction::TreeDown => {
                self.step_sibling_at_current_depth(true);
            }
            BindableAction::TreeParent => {
                self.ascend_to_parent_thread();
            }
            BindableAction::TreeChild => {
                self.descend_to_child_thread();
            }
            BindableAction::JumpTerminal => {
                self.open_or_jump_to_terminal();
            }
            BindableAction::NewAgent => {
                self.status_notice = None;
                self.pending_actions.push_back(AppAction::CreateRoot {
                    title: None,
                    cwd: None,
                });
            }
            BindableAction::ForkThread => {
                let source_thread_id = self.selected_or_active_thread_id();
                self.status_notice = None;
                self.pending_actions.push_back(AppAction::ForkBranch {
                    source_thread_id,
                    title: None,
                });
            }
            BindableAction::Worktree => {
                let thread_id = self.selected_or_active_thread_id();
                self.status_notice = Some("Allocating worktree...".to_string());
                self.pending_actions
                    .push_back(AppAction::EnsureThreadWorktree { thread_id });
            }
            BindableAction::CommandPalette => {
                self.open_command_palette("");
            }
            BindableAction::Help => {
                self.help_scroll_offset = 0;
                self.help_filter_active = false;
                self.help_filter_query.clear();
                self.show_help_modal = true;
            }
            BindableAction::ToggleCards => {
                self.toggle_transcript_cards();
            }
            BindableAction::CopyReply => {
                self.copy_transcript_content(CopyMode::LastReply);
            }
            BindableAction::Quit => {
                self.request_quit_with_confirm();
            }
            BindableAction::AcceptCompletion
            | BindableAction::NextCompletion
            | BindableAction::PrevCompletion
            | BindableAction::DismissCompletion => {}
        }
    }

    pub fn execute_slash_command(&mut self, trimmed: &str, from_palette: bool) -> bool {
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let head = parts.next().unwrap_or("");
        let head_lower = head.to_ascii_lowercase();
        let rest = parts.next().unwrap_or("").trim();

        if head_lower != "/rename" && !matches!(head_lower.as_str(), "/?" | "/help") {
            self.pending_rename_thread_id = None;
        }

        match head_lower.as_str() {
            "/?" | "/help" => {
                self.help_scroll_offset = 0;
                self.help_filter_active = false;
                self.help_filter_query.clear();
                self.show_help_modal = true;
                true
            }
            "/zoom" => {
                self.toggle_sidebar_zoom();
                true
            }
            "/copy" => {
                match rest.to_ascii_lowercase().as_str() {
                    "" => self.copy_transcript_content(CopyMode::LastReply),
                    "code" => self.copy_transcript_content(CopyMode::CodeBlock),
                    "all" => self.copy_transcript_content(CopyMode::All),
                    _ => {
                        self.status_notice = Some("Usage: /copy [code|all]".to_string());
                    }
                }
                true
            }
            "/commands" => {
                let active_id = self.active_thread_id.clone();
                if let Some(cmds) = self.thread_commands.get(&active_id).cloned() {
                    if !cmds.is_empty() {
                        self.record_agent_commands_note(&active_id, &cmds);
                        return true;
                    }
                }
                self.status_notice = Some("Fetching agent commands...".to_string());
                self.pending_actions
                    .push_back(AppAction::FetchAgentCommands {
                        thread_id: active_id,
                    });
                true
            }
            "/model" | "/mode" | "/thinking" | "/effort" => {
                let target_config_id = match head_lower.as_str() {
                    "/model" => "model",
                    "/mode" => "mode",
                    _ => "thought_level",
                };
                let (set_default, query) = parse_config_arg(rest);
                let active_id = self.active_thread_id.clone();
                if query.is_empty() {
                    self.pending_actions
                        .push_back(AppAction::OpenConfigPicker {
                            thread_id: active_id,
                            config_id: target_config_id.to_string(),
                        });
                    return true;
                }
                if let Some(opt) = self
                    .thread_config_options
                    .get(&active_id)
                    .and_then(|opts| find_config_option(opts, target_config_id))
                    .cloned()
                {
                    if opt.options.is_empty() {
                        self.pending_actions.push_back(AppAction::SetAgentConfig {
                            thread_id: active_id,
                            config_id: opt.id,
                            value: query,
                            set_default,
                        });
                    } else if let Some(choice) = match_config_choice(&opt.options, &query) {
                        self.pending_actions.push_back(AppAction::SetAgentConfig {
                            thread_id: active_id,
                            config_id: opt.id,
                            value: choice.value.clone(),
                            set_default,
                        });
                    } else {
                        self.status_notice =
                            Some(format!("No {} option matching '{query}'", opt.name));
                    }
                } else {
                    self.pending_actions
                        .push_back(AppAction::ApplyAgentConfigQuery {
                            thread_id: active_id,
                            config_id: target_config_id.to_string(),
                            query,
                            set_default,
                        });
                }
                true
            }
            "/delete" | "/close" => {
                let target_id = if rest.is_empty() {
                    if from_palette {
                        Some(self.selected_or_active_thread_id())
                    } else {
                        Some(self.active_thread_id.clone())
                    }
                } else {
                    self.threads
                        .iter()
                        .find(|t| t.id == rest || t.title == rest)
                        .map(|t| t.id.clone())
                };
                if let Some(id) = target_id {
                    self.request_delete_thread(id);
                } else {
                    self.status_notice = Some(format!("Unknown context: {rest}"));
                }
                true
            }
            "/branch" | "/fork" | "/grein" => {
                let title = (!rest.is_empty()).then(|| rest.to_string());
                let source_thread_id = if from_palette {
                    self.selected_or_active_thread_id()
                } else {
                    self.active_thread_id.clone()
                };
                self.pending_actions.push_back(AppAction::ForkBranch {
                    source_thread_id,
                    title,
                });
                true
            }
            "/root" | "/new" => {
                let title = (!rest.is_empty()).then(|| rest.to_string());
                self.status_notice = None;
                self.pending_actions
                    .push_back(AppAction::CreateRoot { title, cwd: None });
                true
            }
            "/rename" => {
                if rest.is_empty() {
                    if from_palette {
                        self.begin_inline_rename();
                    } else {
                        self.status_notice = Some("Usage: /rename <new title>".to_string());
                    }
                } else {
                    let thread_id = self.pending_rename_thread_id.take().unwrap_or_else(|| {
                        if from_palette {
                            self.selected_or_active_thread_id()
                        } else {
                            self.active_thread_id.clone()
                        }
                    });
                    self.status_notice = None;
                    self.pending_actions.push_back(AppAction::RenameThread {
                        thread_id,
                        title: rest.to_string(),
                    });
                    self.sync_focus_for_active_thread();
                }
                true
            }
            "/join" | "/merge" => {
                let source_thread_id = if from_palette {
                    self.selected_or_active_thread_id()
                } else {
                    self.active_thread_id.clone()
                };
                let summary = (!rest.is_empty()).then(|| rest.to_string());
                self.request_join_thread(source_thread_id, summary);
                true
            }
            "/term" | "/terminal" | "/zsh" | "/sh" | "/shell" | "/bash" => {
                let cwd = parse_term_cwd(rest);
                let parent_id = if from_palette {
                    self.selected_or_active_thread_id()
                } else {
                    self.active_thread_id.clone()
                };
                self.dispatch_create_terminal(Some(parent_id), cwd);
                true
            }
            "/worktrees" => {
                let filter = (!rest.is_empty()).then(|| rest.to_string());
                self.open_worktrees_sidebar_view(filter);
                true
            }
            "/worktree" | "/wt" => {
                let target_thread = if from_palette {
                    self.selected_or_active_thread_id()
                } else {
                    self.active_thread_id.clone()
                };
                if rest.is_empty() {
                    self.status_notice = Some("Allocating worktree...".to_string());
                    self.pending_actions
                        .push_back(AppAction::EnsureThreadWorktree {
                            thread_id: target_thread,
                        });
                } else {
                    let query = parse_term_cwd(rest).unwrap_or_else(|| rest.to_string());
                    if let Some(target_path) = self.resolve_worktree_query_target(&query) {
                        self.status_notice = Some("Switching worktree...".to_string());
                        self.pending_actions
                            .push_back(AppAction::SwitchThreadWorktree {
                                thread_id: target_thread,
                                target_path,
                            });
                    } else {
                        self.open_worktrees_sidebar_view(Some(query));
                    }
                }
                true
            }
            "/prune" => {
                self.request_prune_worktrees();
                true
            }
            "/autocomplete" => {
                self.execute_autocomplete_command(rest);
                true
            }
            "/agent" => {
                self.execute_agent_command(rest, from_palette);
                true
            }
            "/theme" => {
                self.apply_theme_by_name(rest);
                true
            }
            "/bind" => {
                self.execute_bind_command(rest);
                true
            }
            "/unbind" => {
                self.execute_unbind_command(rest);
                true
            }
            "/keybinds" | "/binds" => {
                self.execute_keybinds_command(rest);
                true
            }
            "/sidebar" => {
                self.execute_sidebar_layout_command(rest);
                true
            }
            "/quit" | "/q" | "/exit" => {
                self.should_quit = true;
                true
            }
            _ => false,
        }
    }

    pub fn execute_sidebar_command(&mut self, raw: &str) {
        let trimmed = raw.trim().trim_start_matches([':', '/']).trim();
        if trimmed.is_empty() {
            return;
        }
        let normalized = format!("/{trimmed}");
        if !self.execute_slash_command(&normalized, true)
            && !self.try_execute_provider_palette_command(trimmed)
        {
            let cmd_name = trimmed.split_whitespace().next().unwrap_or(trimmed);
            self.status_notice = Some(format!("Unknown sidebar command: :{cmd_name}"));
        }
    }

    fn execute_sidebar_layout_command(&mut self, rest: &str) {
        let tokens: Vec<&str> = rest.split_whitespace().collect();
        if tokens.is_empty() {
            self.status_notice = Some(self.sidebar_layout.summary());
            return;
        }
        let first = tokens[0].to_ascii_lowercase();
        match first.as_str() {
            "dynamic" | "auto" => {
                self.sidebar_layout.mode = SidebarMode::Dynamic;
                self.pending_actions.push_back(AppAction::SaveUiPreference {
                    key: "sidebar_mode".to_string(),
                    value: "dynamic".to_string(),
                });
                if let Some(min_tok) = tokens.get(1) {
                    if let Ok(v) = min_tok.trim_end_matches('%').parse::<u16>() {
                        let clamped = v.clamp(10, 50);
                        self.sidebar_layout.min_pct = clamped;
                        if self.sidebar_layout.max_pct < clamped {
                            self.sidebar_layout.max_pct = clamped;
                            self.pending_actions.push_back(AppAction::SaveUiPreference {
                                key: "sidebar_max_pct".to_string(),
                                value: self.sidebar_layout.max_pct.to_string(),
                            });
                        }
                        self.pending_actions.push_back(AppAction::SaveUiPreference {
                            key: "sidebar_min_pct".to_string(),
                            value: clamped.to_string(),
                        });
                    }
                }
                if let Some(max_tok) = tokens.get(2) {
                    if let Ok(v) = max_tok.trim_end_matches('%').parse::<u16>() {
                        let clamped = v.clamp(self.sidebar_layout.min_pct, 60);
                        self.sidebar_layout.max_pct = clamped;
                        self.pending_actions.push_back(AppAction::SaveUiPreference {
                            key: "sidebar_max_pct".to_string(),
                            value: clamped.to_string(),
                        });
                    }
                }
            }
            "fixed" => {
                self.sidebar_layout.mode = SidebarMode::Fixed;
                self.pending_actions.push_back(AppAction::SaveUiPreference {
                    key: "sidebar_mode".to_string(),
                    value: "fixed".to_string(),
                });
                if let Some(val_tok) = tokens.get(1) {
                    if let Some(pct_str) = val_tok.strip_suffix('%') {
                        if let Ok(v) = pct_str.parse::<u16>() {
                            let clamped = v.clamp(10, 60);
                            self.sidebar_layout.fixed_pct = Some(clamped);
                            self.pending_actions.push_back(AppAction::SaveUiPreference {
                                key: "sidebar_fixed_pct".to_string(),
                                value: clamped.to_string(),
                            });
                        }
                    } else if let Ok(v) = val_tok.parse::<u16>() {
                        let clamped = v.clamp(16, 80);
                        self.sidebar_layout.fixed_cols = clamped;
                        self.sidebar_layout.fixed_pct = None;
                        self.pending_actions.push_back(AppAction::SaveUiPreference {
                            key: "sidebar_fixed_cols".to_string(),
                            value: clamped.to_string(),
                        });
                        self.pending_actions.push_back(AppAction::SaveUiPreference {
                            key: "sidebar_fixed_pct".to_string(),
                            value: String::new(),
                        });
                    }
                }
            }
            "min" => {
                if let Some(val_tok) = tokens.get(1) {
                    if let Ok(v) = val_tok.trim_end_matches('%').parse::<u16>() {
                        let clamped = v.clamp(10, 50);
                        self.sidebar_layout.min_pct = clamped;
                        self.pending_actions.push_back(AppAction::SaveUiPreference {
                            key: "sidebar_min_pct".to_string(),
                            value: clamped.to_string(),
                        });
                        if self.sidebar_layout.max_pct < clamped {
                            self.sidebar_layout.max_pct = clamped;
                            self.pending_actions.push_back(AppAction::SaveUiPreference {
                                key: "sidebar_max_pct".to_string(),
                                value: clamped.to_string(),
                            });
                        }
                    } else {
                        self.status_notice = Some("Usage: :sidebar min <10..50%>".to_string());
                        return;
                    }
                } else {
                    self.status_notice = Some("Usage: :sidebar min <10..50%>".to_string());
                    return;
                }
            }
            "max" => {
                if let Some(val_tok) = tokens.get(1) {
                    if let Ok(v) = val_tok.trim_end_matches('%').parse::<u16>() {
                        let clamped = v.clamp(self.sidebar_layout.min_pct, 60);
                        self.sidebar_layout.max_pct = clamped;
                        self.pending_actions.push_back(AppAction::SaveUiPreference {
                            key: "sidebar_max_pct".to_string(),
                            value: clamped.to_string(),
                        });
                    } else {
                        self.status_notice = Some("Usage: :sidebar max <min..60%>".to_string());
                        return;
                    }
                } else {
                    self.status_notice = Some("Usage: :sidebar max <min..60%>".to_string());
                    return;
                }
            }
            "overflow" => {
                let next = match tokens.get(1).map(|s| s.to_ascii_lowercase()).as_deref() {
                    Some("on" | "true" | "1" | "yes") => true,
                    Some("off" | "false" | "0" | "no") => false,
                    None => !self.sidebar_layout.overflow,
                    _ => {
                        self.status_notice = Some("Usage: :sidebar overflow <on|off>".to_string());
                        return;
                    }
                };
                self.sidebar_layout.overflow = next;
                self.pending_actions.push_back(AppAction::SaveUiPreference {
                    key: "sidebar_overflow".to_string(),
                    value: if next { "on" } else { "off" }.to_string(),
                });
            }
            other => {
                if let Some(pct_str) = other.strip_suffix('%') {
                    if let Ok(v) = pct_str.parse::<u16>() {
                        let clamped = v.clamp(10, 60);
                        self.sidebar_layout.mode = SidebarMode::Fixed;
                        self.sidebar_layout.fixed_pct = Some(clamped);
                        self.pending_actions.push_back(AppAction::SaveUiPreference {
                            key: "sidebar_mode".to_string(),
                            value: "fixed".to_string(),
                        });
                        self.pending_actions.push_back(AppAction::SaveUiPreference {
                            key: "sidebar_fixed_pct".to_string(),
                            value: clamped.to_string(),
                        });
                    } else {
                        self.status_notice = Some(
                            "Usage: :sidebar [dynamic [min max]|fixed <cols|pct%>|min <%>|max <%>|overflow <on|off>]"
                                .to_string(),
                        );
                        return;
                    }
                } else if let Ok(v) = other.parse::<u16>() {
                    let clamped = v.clamp(16, 80);
                    self.sidebar_layout.mode = SidebarMode::Fixed;
                    self.sidebar_layout.fixed_cols = clamped;
                    self.sidebar_layout.fixed_pct = None;
                    self.pending_actions.push_back(AppAction::SaveUiPreference {
                        key: "sidebar_mode".to_string(),
                        value: "fixed".to_string(),
                    });
                    self.pending_actions.push_back(AppAction::SaveUiPreference {
                        key: "sidebar_fixed_cols".to_string(),
                        value: clamped.to_string(),
                    });
                    self.pending_actions.push_back(AppAction::SaveUiPreference {
                        key: "sidebar_fixed_pct".to_string(),
                        value: String::new(),
                    });
                } else {
                    self.status_notice = Some(
                        "Usage: :sidebar [dynamic [min max]|fixed <cols|pct%>|min <%>|max <%>|overflow <on|off>]"
                            .to_string(),
                    );
                    return;
                }
            }
        }
        self.maybe_queue_terminal_resize();
        self.status_notice = Some(self.sidebar_layout.summary());
    }

    fn execute_bind_command(&mut self, rest: &str) {
        let mut parts = rest.splitn(2, char::is_whitespace);
        let action_str = parts.next().unwrap_or("").trim();
        let chord_str = parts.next().unwrap_or("").trim();
        if action_str.is_empty() {
            self.execute_keybinds_command("");
            return;
        }
        let Some(action) = BindableAction::from_str(action_str) else {
            self.status_notice = Some(format!(
                "Unknown action '{action_str}'. Try: enter_manage_mode, toggle_zoom, toggle_worktrees, scroll_up, scroll_down, prev_sibling, next_sibling, tree_up, tree_down, tree_parent, tree_child, jump_terminal, new_agent, fork_thread, worktree, command_palette, help"
            ));
            return;
        };
        if chord_str.is_empty() {
            self.status_notice = Some(format!(
                "{} = {} (usage: :bind {} <chord>)",
                action.as_str(),
                self.keymap.display_for(action),
                action.as_str()
            ));
            return;
        }
        match self.keymap.bind(action, chord_str) {
            Ok(canonical) => {
                self.pending_actions
                    .push_back(AppAction::SaveKeybindingPreference {
                        action: action.as_str().to_string(),
                        binding: canonical,
                    });
                self.status_notice = Some(format!(
                    "Bound {} -> {}",
                    action.as_str(),
                    self.keymap.display_for(action)
                ));
            }
            Err(err) => {
                self.status_notice = Some(format!("Invalid keybind: {err}"));
            }
        }
    }

    fn execute_unbind_command(&mut self, rest: &str) {
        let action_str = rest.trim();
        if action_str.is_empty() {
            self.status_notice = Some("Usage: :unbind <action>".to_string());
            return;
        }
        let Some(action) = BindableAction::from_str(action_str) else {
            self.status_notice = Some(format!("Unknown action '{action_str}'"));
            return;
        };
        self.keymap.unbind(action);
        self.pending_actions
            .push_back(AppAction::RemoveKeybindingPreference {
                action: action.as_str().to_string(),
            });
        self.status_notice = Some(format!(
            "Reset {} -> {}",
            action.as_str(),
            self.keymap.display_for(action)
        ));
    }

    fn execute_keybinds_command(&mut self, rest: &str) {
        if rest.trim().eq_ignore_ascii_case("custom") {
            let custom = self.keymap.custom_entries();
            if custom.is_empty() {
                self.status_notice = Some("No custom keybindings; defaults active".to_string());
            } else {
                let summary = custom
                    .into_iter()
                    .map(|(a, d)| format!("{}={d}", a.as_str()))
                    .collect::<Vec<_>>()
                    .join(" · ");
                self.status_notice = Some(format!("Custom keybinds: {summary}"));
            }
            return;
        }
        let summary = self
            .keymap
            .all_entries()
            .into_iter()
            .map(|(a, d)| format!("{}={d}", a.as_str()))
            .collect::<Vec<_>>()
            .join(" · ");
        self.status_notice = Some(format!("WRITE keybinds: {summary}"));
    }

    pub(super) fn handle_key_palette(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let pos = clamp_char_boundary(&self.palette_input, self.palette_cursor);
        let suggestions = self.palette_suggestions();

        match (key.code, ctrl, alt) {
            (KeyCode::Esc, false, false) | (KeyCode::Char('c' | 'C'), true, false) => {
                self.palette_active = false;
                self.palette_input.clear();
                self.palette_cursor = 0;
                self.palette_match_idx = 0;
            }
            (KeyCode::Enter, false, false) => {
                let raw = std::mem::take(&mut self.palette_input);
                self.palette_active = false;
                self.palette_cursor = 0;
                self.palette_match_idx = 0;
                if !raw.trim().is_empty() {
                    self.execute_sidebar_command(&raw);
                }
            }
            (KeyCode::Tab, false, false) => {
                if !suggestions.is_empty() {
                    let idx = self.palette_match_idx.min(suggestions.len() - 1);
                    self.palette_input = suggestions[idx].0.clone();
                    self.palette_cursor = self.palette_input.len();
                    self.palette_match_idx = 0;
                }
            }
            (KeyCode::Up, false, false) | (KeyCode::Char('p' | 'P'), true, false) => {
                if !suggestions.is_empty() {
                    let len = suggestions.len();
                    let cur = self.palette_match_idx.min(len - 1);
                    self.palette_match_idx = (cur + len - 1) % len;
                }
            }
            (KeyCode::Down, false, false) | (KeyCode::Char('n' | 'N'), true, false) => {
                if !suggestions.is_empty() {
                    let len = suggestions.len();
                    let cur = self.palette_match_idx.min(len - 1);
                    self.palette_match_idx = (cur + 1) % len;
                }
            }
            (KeyCode::Char('a' | 'A'), true, false) | (KeyCode::Home, false, false) => {
                self.palette_cursor = 0;
            }
            (KeyCode::Char('e' | 'E'), true, false) | (KeyCode::End, false, false) => {
                self.palette_cursor = self.palette_input.len();
            }
            (KeyCode::Char('u' | 'U'), true, false) => {
                self.palette_input.drain(..pos);
                self.palette_cursor = 0;
                self.palette_match_idx = 0;
            }
            (KeyCode::Char('w' | 'W'), true, false) => {
                let start = prev_word_boundary(&self.palette_input, pos);
                self.palette_input.drain(start..pos);
                self.palette_cursor = start;
                self.palette_match_idx = 0;
            }
            (KeyCode::Left, false, false) if pos > 0 => {
                if let Some((idx, _)) = self.palette_input[..pos].char_indices().next_back() {
                    self.palette_cursor = idx;
                }
            }
            (KeyCode::Right, false, false) if pos < self.palette_input.len() => {
                if let Some(ch) = self.palette_input[pos..].chars().next() {
                    self.palette_cursor = pos + ch.len_utf8();
                }
            }
            (KeyCode::Backspace, false, false) => {
                if self.palette_input.is_empty() {
                    self.palette_active = false;
                    self.palette_cursor = 0;
                    self.palette_match_idx = 0;
                } else if pos > 0 {
                    if let Some((idx, _)) = self.palette_input[..pos].char_indices().next_back() {
                        self.palette_input.remove(idx);
                        self.palette_cursor = idx;
                        self.palette_match_idx = 0;
                    }
                }
            }
            (KeyCode::Delete, false, false) if pos < self.palette_input.len() => {
                self.palette_input.remove(pos);
                self.palette_cursor = pos;
                self.palette_match_idx = 0;
            }
            (KeyCode::Char('v' | 'V'), true, false) => {
                self.paste_from_clipboard();
            }
            (KeyCode::Char(c), _, _) if is_altgr_char(c, ctrl, alt) => {
                self.palette_input.insert(pos, c);
                self.palette_cursor = pos + c.len_utf8();
                self.palette_match_idx = 0;
            }
            _ => {}
        }
    }
}
