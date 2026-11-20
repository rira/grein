use std::io::{self, IsTerminal};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use grein_core::blackboard::{BlackboardEntry, BlackboardEntryKind};
use grein_core::model::{
    AnchorId, AuthorRole, BlackboardEntryId, ConversationNode, NodeId, ThreadId,
};
use grein_core::storage::sqlite::SqliteStorage;
use ratatui::backend::Backend;
use ratatui::Terminal;
use tokio::sync::mpsc;

use crate::app::{AnchorItem, App, FocusedPane, ThreadItem, TreeAction, TuiError};
use crate::engine_port::{HydratedSession, TuiEngineEvent, TuiEnginePort};

const USER_PREFIX: &str = "# User: ";
const AGENT_HEADER: &str = "## Agent (agy)";

pub struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    pub fn new() -> io::Result<Self> {
        if io::stdout().is_terminal() {
            enable_raw_mode()?;
            execute!(io::stdout(), EnterAlternateScreen)?;
            Ok(Self { active: true })
        } else {
            Ok(Self { active: false })
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
        }
    }
}

pub fn draw_frame<B: Backend>(terminal: &mut Terminal<B>, app: &App) -> io::Result<()> {
    terminal.draw(|frame| crate::render::render(frame, app))?;
    Ok(())
}

pub fn handle_event(app: &mut App, event: Event) -> io::Result<bool> {
    if let Event::Key(key) = event {
        let _ = handle_key_event(app, key);
    }
    Ok(app.should_quit())
}

/// Structured view of the transcript: ordered (role, content) turns.
fn collect_turns(transcript: &str) -> Vec<(AuthorRole, String)> {
    let mut turns: Vec<(AuthorRole, String)> = Vec::new();
    let mut current: Option<(AuthorRole, String)> = None;

    for line in transcript.lines() {
        if let Some(rest) = line.strip_prefix(USER_PREFIX) {
            if let Some((role, content)) = current.take() {
                turns.push((role, content.trim().to_string()));
            }
            current = Some((AuthorRole::User, rest.to_string()));
        } else if line == AGENT_HEADER {
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

fn render_node(role: &AuthorRole, content: &str) -> String {
    match role {
        AuthorRole::User => format!("{USER_PREFIX}{content}\n\n"),
        _ => format!("{AGENT_HEADER}\n{content}\n\n"),
    }
}

impl App {
    pub fn submit_composer_prompt(&mut self, prompt: impl Into<String>) {
        let prompt = prompt.into();
        let trimmed = prompt.trim();
        if trimmed.is_empty() {
            return;
        }
        if !self.active_transcript_md.is_empty() && !self.active_transcript_md.ends_with("\n\n") {
            if !self.active_transcript_md.ends_with('\n') {
                self.active_transcript_md.push('\n');
            }
            self.active_transcript_md.push('\n');
        }
        self.active_transcript_md
            .push_str(&render_node(&AuthorRole::User, trimmed));
        self.last_submitted_prompt = Some(trimmed.to_string());
        self.sync_anchors_and_blackboard();
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
            if !self.active_transcript_md.is_empty() && !self.active_transcript_md.ends_with("\n\n")
            {
                if !self.active_transcript_md.ends_with('\n') {
                    self.active_transcript_md.push('\n');
                }
                self.active_transcript_md.push('\n');
            }
            self.active_transcript_md
                .push_str(&render_node(&AuthorRole::Assistant, clean));
        }
        self.sync_anchors_and_blackboard();
    }

    pub fn sync_anchors_and_blackboard(&mut self) {
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
            self.threads.push(ThreadItem {
                id: ThreadId::now_v7().to_string(),
                title,
                anchors: Vec::new(),
            });
        }

        let parsed = grein_core::anchor::parse_anchors(&self.active_transcript_md);
        let mut anchor_items: Vec<AnchorItem> = parsed
            .into_iter()
            .map(|a| AnchorItem {
                id: a.slug,
                label: a.title,
            })
            .collect();

        if anchor_items.is_empty() {
            for (idx, (role, content)) in turns.iter().enumerate() {
                if *role == AuthorRole::User {
                    let label = content.lines().next().unwrap_or("Turn").to_string();
                    anchor_items.push(AnchorItem {
                        id: format!("anchor-{}", idx + 1),
                        label,
                    });
                }
            }
        }

        if let Some(thread) = self.threads.first_mut() {
            if thread.id == "thread-1" {
                thread.id = ThreadId::now_v7().to_string();
            }
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
        let prev_active = self.tree.active_anchor_id().map(str::to_string);
        self.tree = crate::ui::tree::TreeState::new(nodes);
        if let Some(idx) = prev_selected {
            self.tree.select_index(idx);
        }
        if let Some(active_id) = prev_active {
            self.tree.set_active_anchor_id(Some(active_id));
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

    pub fn persist_state_to_sqlite(&self, db_path: &Path) {
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let Ok(storage) = SqliteStorage::open(db_path) else {
            return;
        };
        let turns = collect_turns(&self.active_transcript_md);
        if turns.is_empty() {
            return;
        }

        let thread_id = self
            .threads
            .first()
            .and_then(|t| t.id.parse().ok().map(ThreadId))
            .or_else(|| {
                storage
                    .get_blackboard_entry("active_thread_id")
                    .ok()
                    .flatten()
                    .and_then(|e| e.value.parse().ok().map(ThreadId))
            });

        let existing_thread = thread_id.and_then(|tid| storage.get_thread(tid).ok().flatten());

        let existing_trunk = existing_thread
            .as_ref()
            .and_then(|t| storage.get_trunk_path(t.active_leaf_id).ok())
            .unwrap_or_default();

        let common_len = existing_trunk
            .iter()
            .zip(turns.iter())
            .take_while(|(node, (role, content))| node.role == *role && node.content == *content)
            .count();

        let mut parent_id = if common_len > 0 {
            existing_trunk.get(common_len - 1).map(|n| n.id)
        } else {
            None
        };
        let mut root_id = if common_len > 0 {
            existing_trunk.first().map(|n| n.id)
        } else {
            None
        };
        let mut last_node_id = parent_id;

        for (role, content) in turns.into_iter().skip(common_len) {
            let node_id = NodeId::now_v7();
            let node = ConversationNode::new(node_id, parent_id, role, content);
            if storage.insert_node(&node).is_ok() {
                if root_id.is_none() {
                    root_id = Some(node_id);
                }
                parent_id = Some(node_id);
                last_node_id = Some(node_id);
            }
        }

        let (Some(r_id), Some(leaf_id)) = (root_id, last_node_id) else {
            return;
        };

        let real_tid = if let Some(existing) = existing_thread {
            let _ = storage.update_active_leaf(existing.id, leaf_id);
            existing.id
        } else {
            match storage.create_thread(r_id, AnchorId::now_v7()) {
                Ok(tid) => {
                    let _ = storage.update_active_leaf(tid, leaf_id);
                    tid
                }
                Err(_) => return,
            }
        };

        let title = self
            .threads
            .first()
            .map(|t| t.title.clone())
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| "Active Session".to_string());

        let entries = [
            ("active_thread_id".to_string(), real_tid.to_string()),
            ("active_thread_title".to_string(), title),
            (
                "blackboard_notes".to_string(),
                self.blackboard_notes.clone(),
            ),
        ];

        for (key, value) in entries {
            let _ = storage.delete_blackboard_entry(&key);
            let _ = storage.insert_blackboard_entry(&BlackboardEntry {
                id: BlackboardEntryId::now_v7(),
                key,
                value,
                kind: BlackboardEntryKind::Note,
                created_at: 1,
            });
        }
    }

    pub fn load_state_from_sqlite(&mut self, db_path: &Path) {
        if !db_path.exists() {
            return;
        }
        let Ok(storage) = SqliteStorage::open(db_path) else {
            return;
        };
        let Some(tid) = storage
            .get_blackboard_entry("active_thread_id")
            .ok()
            .flatten()
            .and_then(|e| e.value.parse().ok().map(ThreadId))
        else {
            return;
        };
        let Some(thread) = storage.get_thread(tid).ok().flatten() else {
            return;
        };
        let Ok(trunk) = storage.get_trunk_path(thread.active_leaf_id) else {
            return;
        };

        let mut reconstructed = String::new();
        let mut first_user_content: Option<String> = None;
        for node in &trunk {
            if node.role == AuthorRole::User && first_user_content.is_none() {
                first_user_content = Some(node.content.clone());
            }
            reconstructed.push_str(&render_node(&node.role, &node.content));
        }
        self.active_transcript_md = reconstructed;

        let title = storage
            .get_blackboard_entry("active_thread_title")
            .ok()
            .flatten()
            .map(|e| e.value)
            .filter(|v| !v.trim().is_empty())
            .or(first_user_content)
            .unwrap_or_else(|| "Active Session".to_string());

        self.threads = vec![ThreadItem {
            id: thread.id.to_string(),
            title,
            anchors: Vec::new(),
        }];

        self.sync_anchors_and_blackboard();

        if let Some(notes_entry) = storage
            .get_blackboard_entry("blackboard_notes")
            .ok()
            .flatten()
        {
            if !notes_entry.value.is_empty() {
                self.blackboard_notes = notes_entry.value;
            }
        }
    }
}

pub fn handle_key_event(app: &mut App, key: KeyEvent) -> Result<Option<TreeAction>, TuiError> {
    let was_composer = app.focused_pane() == FocusedPane::Composer
        || app.focus() == FocusedPane::Composer
        || app.active_focus() == FocusedPane::Composer;
    let is_enter = key.code == KeyCode::Enter
        && !key
            .modifiers
            .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT);
    if was_composer && is_enter {
        app.last_submitted_prompt = None;
    }
    app.handle_key_event(key);
    if key.code == KeyCode::Enter && app.focused_pane() == FocusedPane::Tree {
        if let Some(idx) = app.tree.selected_index() {
            if let Some(crate::ui::tree::TreeNode::Anchor(anchor)) = app.tree.nodes.get(idx) {
                return Ok(Some(TreeAction::ActivateAnchor(anchor.id.clone())));
            }
        }
    }
    Ok(None)
}

/// Seeds the app (threads, transcript nodes, blackboard) from a hydrated session.
pub fn apply_hydrated_session(app: &mut App, session: &HydratedSession) {
    app.threads = session
        .threads
        .iter()
        .map(|t| ThreadItem {
            id: t.id.clone(),
            title: t.title.clone(),
            anchors: Vec::new(),
        })
        .collect();

    let active = session
        .threads
        .iter()
        .find(|t| t.id == session.active_thread_id)
        .or_else(|| session.threads.first());
    if let Some(thread) = active {
        if let Some(pos) = app.threads.iter().position(|t| t.id == thread.id) {
            let item = app.threads.remove(pos);
            app.threads.insert(0, item);
        }
        let mut transcript = String::new();
        for turn in &thread.turns {
            let role = if turn.role.eq_ignore_ascii_case("user") {
                AuthorRole::User
            } else {
                AuthorRole::Assistant
            };
            transcript.push_str(&render_node(&role, &turn.content));
        }
        app.active_transcript_md = transcript;
    }

    app.sync_anchors_and_blackboard();
    for entry in &session.blackboard {
        app.blackboard_notes
            .push_str(&format!("{}: {}\n", entry.key, entry.value));
    }
}

/// Folds a streamed engine event into the app state.
pub fn apply_engine_event(app: &mut App, event: TuiEngineEvent) {
    match event {
        TuiEngineEvent::StreamChunk { content, .. } => app.apply_agent_response(&content),
        TuiEngineEvent::TurnCompleted { .. } => app.sync_anchors_and_blackboard(),
        TuiEngineEvent::ToolInvoked { tool_name, .. } => {
            app.apply_agent_response(&format!("[tool invoked: {tool_name}]"));
        }
        TuiEngineEvent::ToolResult {
            tool_name, output, ..
        } => {
            app.apply_agent_response(&format!("[tool result: {tool_name}] {output}"));
        }
        TuiEngineEvent::PermissionRequested {
            request_id,
            description,
        } => {
            app.blackboard_notes.push_str(&format!(
                "Permission requested [{request_id}]: {description}\n"
            ));
        }
        TuiEngineEvent::ThreadStatusChanged { thread_id, status } => {
            app.blackboard_notes
                .push_str(&format!("Thread {thread_id}: {status}\n"));
        }
        TuiEngineEvent::BlackboardUpdated { key, value } => {
            app.blackboard_notes.push_str(&format!("{key}: {value}\n"));
        }
        TuiEngineEvent::Error { message } => {
            app.apply_agent_response(&format!("Error: {message}"));
        }
    }
}

fn spawn_key_reader(
    stop: Arc<AtomicBool>,
    tx: mpsc::UnboundedSender<Event>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            match event::poll(Duration::from_millis(25)) {
                Ok(true) => match event::read() {
                    Ok(ev) => {
                        if tx.send(ev).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                },
                Ok(false) => {}
                Err(_) => break,
            }
        }
    })
}

pub async fn run_with_engine<B: Backend>(
    terminal: &mut Terminal<B>,
    mut app: App,
    engine: Arc<dyn TuiEnginePort>,
) -> io::Result<()> {
    let session = engine.hydrate_session().await?;
    apply_hydrated_session(&mut app, &session);
    let mut engine_events = engine.subscribe_events().await?;

    let stop = Arc::new(AtomicBool::new(false));
    let (key_tx, mut key_rx) = mpsc::unbounded_channel::<Event>();
    let reader = spawn_key_reader(Arc::clone(&stop), key_tx);

    let mut tick = tokio::time::interval(Duration::from_millis(50));
    let mut result = Ok(());

    loop {
        if let Err(err) = draw_frame(terminal, &app) {
            result = Err(err);
            break;
        }
        if app.should_quit() {
            break;
        }

        tokio::select! {
            Some(ev) = key_rx.recv() => {
                if let Err(err) = handle_event(&mut app, ev) {
                    result = Err(err);
                    break;
                }
                if let Some(prompt) = app.last_submitted_prompt.take() {
                    let thread_id = app
                        .threads
                        .first()
                        .map(|t| t.id.clone())
                        .unwrap_or_else(|| session.active_thread_id.clone());
                    if let Err(err) = engine.send_turn(&thread_id, prompt.trim()).await {
                        app.apply_agent_response(&format!("Error: {err}"));
                    }
                }
            }
            Some(event) = engine_events.recv() => {
                apply_engine_event(&mut app, event);
            }
            _ = tick.tick() => {}
        }
    }

    stop.store(true, Ordering::Relaxed);
    let _ = reader.join();
    result
}
