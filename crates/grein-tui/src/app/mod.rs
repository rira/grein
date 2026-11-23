mod commands;
mod editing;
mod input;
mod navigation;
mod pickers;
#[cfg(test)]
mod tests;
mod threads;
mod transcript_ops;
mod types;
mod worktrees;

#[allow(unused_imports)]
pub(crate) use commands::{parse_config_arg, parse_term_cwd};
pub use commands::palette_command_specs;
#[allow(unused_imports)]
pub(crate) use editing::is_altgr_char;
pub use editing::{line_end, line_start, next_word_boundary, prev_word_boundary};
#[allow(unused_imports)]
pub(crate) use pickers::{find_config_option, is_config_option_kind, match_config_choice};
pub use threads::{
    active_shell_label, display_thread_title, format_terminal_sidebar_cwd_with_home,
    is_generic_agent_placeholder, normalize_custom_name, shell_label_from_env,
    shorten_terminal_cwd, shorten_terminal_cwd_with_home, thread_display_parts,
    thread_display_parts_with_home,
};
#[allow(unused_imports)]
pub(crate) use transcript_ops::CopyMode;
pub use types::{
    AnchorItem, App, AppAction, AppConfig, AppEvent, AppState, ConfigPickerModal, FocusDirection,
    FocusPane, FocusedPane, PaneFocus, PendingPermission, PendingQuitConfirm, PendingThreadDelete,
    PendingWorktreeDelete, RenamePromptState, SidebarLayoutConfig, SidebarMode, SidebarView,
    ThreadDisplayParts, ThreadItem, TreeAction, TuiComponent, TuiError, WorktreeAutocompleteItem,
    WorktreeRow,
};
#[allow(unused_imports)]
pub(crate) use worktrees::{parse_worktree_autocomplete_query, worktree_matches_autocomplete};
pub use worktrees::worktree_display_label;

use std::collections::BTreeSet;
use std::io::IsTerminal;

use crossterm::event::KeyEvent;

impl App {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(config: AppConfig) -> Self {
        Self {
            config,
            ..Self::default()
        }
    }

    pub(crate) fn has_active_modal(&self) -> bool {
        self.pending_permission.is_some()
            || self.pending_worktree_delete.is_some()
            || self.pending_thread_delete.is_some()
            || self.pending_quit_confirm.is_some()
            || self.theme_picker.is_some()
            || self.config_picker.is_some()
            || self.show_help_modal
    }

    pub fn is_running(&self) -> bool {
        !self.should_quit
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    pub fn poll_events(&mut self, timeout: std::time::Duration) -> Result<(), TuiError> {
        if !std::io::stdin().is_terminal() {
            return Ok(());
        }
        if crossterm::event::poll(timeout)? {
            if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
                self.handle_key_event(key);
            }
        }
        Ok(())
    }

    pub fn handle_event(&mut self, event: AppEvent) -> bool {
        match event {
            AppEvent::Key(k) => self.handle_key_event(k),
            AppEvent::Tick => {
                self.tick_count = self.tick_count.wrapping_add(1);
                false
            }
            AppEvent::Quit => {
                self.should_quit = true;
                true
            }
            AppEvent::FocusChanged(pane) => {
                self.set_focus(pane);
                true
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        self.handle_key_event(key)
    }

    pub fn request_quit_with_confirm(&mut self) {
        let mut terminal_ids: BTreeSet<&str> = self
            .thread_meta
            .values()
            .filter(|m| m.kind == grein_core::ThreadKind::Terminal)
            .map(|m| m.thread_id.as_str())
            .collect();
        for id in self.pty_screens.keys() {
            terminal_ids.insert(id.as_str());
        }
        let active_terminals = terminal_ids
            .into_iter()
            .filter(|id| !self.is_terminal_exited(id))
            .count();
        let busy_agents = self
            .thread_statuses
            .values()
            .filter(|s| matches!(s.as_str(), "streaming" | "executing" | "thinking"))
            .count();
        if active_terminals > 0 || busy_agents > 0 {
            self.pending_quit_confirm = Some(PendingQuitConfirm {
                active_terminals,
                busy_agents,
            });
        } else {
            self.should_quit = true;
        }
    }
}
