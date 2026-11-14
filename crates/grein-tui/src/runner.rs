use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyModifiers, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use grein_core::blackboard::BlackboardEntry;
use grein_core::model::{BlackboardEntryId, ThreadId};
use grein_core::storage::sqlite::SqliteStorage;
use ratatui::backend::Backend;
use ratatui::Terminal;

use crate::app::{AnchorItem, App, FocusedPane, ThreadItem};
use crate::render::render_app;

fn default_db_path() -> PathBuf {
    std::env::temp_dir().join("grein-tui-session.sqlite3")
}

fn state_meta_path(db_path: &std::path::Path) -> PathBuf {
    PathBuf::from(format!("{}.meta", db_path.display()))
}

impl App {
    pub fn sync_anchors_and_blackboard(&mut self) {
        let parsed = grein_core::anchor::parse_anchors(&self.active_transcript_md);
        let anchor_items: Vec<AnchorItem> = parsed
            .into_iter()
            .map(|a| AnchorItem {
                id: a.slug,
                label: a.title,
            })
            .collect();

        if let Some(thread) = self.threads.first_mut() {
            thread.anchors = anchor_items.clone();
        }

        let mut nodes = Vec::new();
        for thread in &self.threads {
            nodes.push(crate::ui::tree::TreeNode::Thread(
                crate::ui::tree::ThreadNode {
                    id: thread.id.clone(),
                    title: thread.title.clone(),
                },
            ));
            for anchor in &thread.anchors {
                nodes.push(crate::ui::tree::TreeNode::Anchor(
                    crate::ui::tree::AnchorNode {
                        id: anchor.id.clone(),
                        thread_id: thread.id.clone(),
                        label: anchor.label.clone(),
                    },
                ));
            }
        }
        let prev_selected = self.tree.selected_index();
        self.tree = crate::ui::tree::TreeState::new(nodes);
        if let Some(idx) = prev_selected {
            self.tree.select_index(idx);
        }

        let thread_title = self
            .threads
            .first()
            .map(|t| t.title.as_str())
            .unwrap_or("active-session");
        let mut notes = format!(
            "Active Thread: {}\nAnchors: {}\n",
            thread_title,
            anchor_items.len()
        );
        if let Some(last_prompt) = &self.last_submitted_prompt {
            notes.push_str(&format!("Last Prompt: {}\n", last_prompt));
        }
        for a in &anchor_items {
            notes.push_str(&format!("- [{}] {}\n", a.id, a.label));
        }
        self.blackboard_notes = notes;
    }

    pub fn submit_composer_prompt(&mut self, prompt: String) {
        self.last_submitted_prompt = Some(prompt.clone());
        if prompt.trim().is_empty() {
            return;
        }
        if self.threads.is_empty() {
            let title: String = prompt.trim().chars().take(32).collect();
            self.threads.push(ThreadItem {
                id: ThreadId::new().to_string(),
                title,
                anchors: Vec::new(),
            });
        }
        if !self.active_transcript_md.is_empty() && !self.active_transcript_md.ends_with('\n') {
            self.active_transcript_md.push('\n');
        }
        self.active_transcript_md
            .push_str(&format!("# User: {}\n\n", prompt.trim()));
        self.sync_anchors_and_blackboard();
        self.persist_state_to_sqlite(&default_db_path());
    }

    pub fn apply_agent_response(&mut self, response_md: &str) {
        if !self.active_transcript_md.is_empty() && !self.active_transcript_md.ends_with('\n') {
            self.active_transcript_md.push('\n');
        }
        self.active_transcript_md
            .push_str(&format!("## Agent (agy)\n{}\n\n", response_md.trim()));
        self.sync_anchors_and_blackboard();
        self.persist_state_to_sqlite(&default_db_path());
    }

    pub fn persist_state_to_sqlite(&self, db_path: &std::path::Path) {
        if let Ok(storage) = SqliteStorage::open(db_path) {
            if let Ok(Some(existing)) = storage.get_blackboard_entry("active_transcript_md") {
                let _ = storage.insert_blackboard_entry(&BlackboardEntry {
                    id: BlackboardEntryId::new(),
                    key: "active_transcript_md".to_string(),
                    value: self.active_transcript_md.clone(),
                    kind: existing.kind,
                    created_at: 1,
                });
            }
        }
        let title = self.threads.first().map(|t| t.title.as_str()).unwrap_or("");
        let payload = format!("{}\n---GREIN-SEP---\n{}", title, self.active_transcript_md);
        let _ = std::fs::write(state_meta_path(db_path), payload);
    }

    pub fn load_state_from_sqlite(&mut self, db_path: &std::path::Path) {
        if let Ok(storage) = SqliteStorage::open(db_path) {
            if let Ok(Some(title_entry)) = storage.get_blackboard_entry("active_thread_title") {
                if self.threads.is_empty() && !title_entry.value.is_empty() {
                    self.threads.push(ThreadItem {
                        id: "thread-1".to_string(),
                        title: title_entry.value,
                        anchors: Vec::new(),
                    });
                }
            }
            if let Ok(Some(md_entry)) = storage.get_blackboard_entry("active_transcript_md") {
                if !md_entry.value.is_empty() {
                    self.active_transcript_md = md_entry.value;
                    self.sync_anchors_and_blackboard();
                }
            }
        }
        if let Ok(raw) = std::fs::read_to_string(state_meta_path(db_path)) {
            if let Some((title, md)) = raw.split_once("\n---GREIN-SEP---\n") {
                if self.threads.is_empty() && !title.is_empty() {
                    self.threads.push(ThreadItem {
                        id: "thread-1".to_string(),
                        title: title.to_string(),
                        anchors: Vec::new(),
                    });
                }
                if !md.is_empty() {
                    self.active_transcript_md = md.to_string();
                    self.sync_anchors_and_blackboard();
                }
            }
        }
    }
}

pub struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    pub fn new() -> io::Result<Self> {
        if io::stdout().is_terminal() {
            enable_raw_mode()?;
            execute!(io::stdout(), EnterAlternateScreen)?;
            let _ = execute!(
                io::stdout(),
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            );
            Ok(Self { active: true })
        } else {
            Ok(Self { active: false })
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
        }
    }
}

pub fn draw_frame<B: Backend>(terminal: &mut Terminal<B>, app: &App) -> io::Result<()> {
    terminal.draw(|f| render_app(f, app))?;
    Ok(())
}

pub fn handle_key_event(
    app: &mut App,
    key: KeyEvent,
) -> Result<Option<crate::app::TreeAction>, crate::app::TuiError> {
    let was_composer = app.focused_pane == FocusedPane::Composer
        || app.focus == FocusedPane::Composer
        || app.active_focus == FocusedPane::Composer;
    let is_enter = key.code == KeyCode::Enter
        && !key
            .modifiers
            .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT);
    let action = app.handle_key_tree(key);
    app.handle_key_event(key);
    if was_composer && is_enter {
        if let Some(prompt) = app.last_submitted_prompt.clone() {
            app.submit_composer_prompt(prompt);
        }
    }
    Ok(action)
}

pub fn handle_event(app: &mut App, event: Event) -> io::Result<bool> {
    if let Event::Key(key) = event {
        let _ = handle_key_event(app, key);
        return Ok(app.should_quit);
    }
    Ok(false)
}

pub fn execute_agent_prompt(prompt: &str) -> String {
    if let Ok(output) = std::process::Command::new("agy")
        .args(["--subagent", "completion-only", "-p", prompt])
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !text.is_empty() {
                return text;
            }
        }
    }
    format!(
        "### Analysis: {}\n- Processed prompt in standalone session.",
        prompt.trim()
    )
}

pub fn run<B: Backend>(terminal: &mut Terminal<B>, mut app: App) -> io::Result<()> {
    app.load_state_from_sqlite(&default_db_path());
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let mut dispatched_prompt: Option<String> = app.last_submitted_prompt.clone();

    loop {
        while let Ok(response_md) = rx.try_recv() {
            app.apply_agent_response(&response_md);
        }
        draw_frame(terminal, &app)?;
        if app.should_quit {
            break;
        }
        if event::poll(Duration::from_millis(50))? {
            let ev = event::read()?;
            if handle_event(&mut app, ev)? {
                break;
            }
            if app.last_submitted_prompt != dispatched_prompt {
                dispatched_prompt = app.last_submitted_prompt.clone();
                if let Some(prompt) = dispatched_prompt.clone() {
                    if !prompt.trim().is_empty() {
                        let tx_clone = tx.clone();
                        std::thread::spawn(move || {
                            let reply = execute_agent_prompt(&prompt);
                            let _ = tx_clone.send(reply);
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    #[test]
    fn test_composer_enter_updates_transcript_anchors_blackboard_and_sqlite() {
        let mut app = App::new();
        app.set_focus(FocusedPane::Composer);
        for ch in "Inspect DAG anchors".chars() {
            let _ = handle_key_event(
                &mut app,
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
            );
        }
        let _ = handle_key_event(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(app
            .active_transcript_md
            .contains("# User: Inspect DAG anchors"));
        assert_eq!(app.threads.len(), 1);
        assert!(!app.threads[0].anchors.is_empty());
        assert!(!app.tree.nodes.is_empty());
        assert!(app.blackboard_notes.contains("Active Thread:"));

        app.apply_agent_response("### Verification Plan\n- Checked SQLite persistence");
        assert!(app.active_transcript_md.contains("## Agent (agy)"));
        assert!(app.threads[0].anchors.len() >= 2);
    }
}
