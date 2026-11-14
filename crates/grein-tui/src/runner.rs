use std::io::{self, BufRead, BufReader, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
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
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::Terminal;

use crate::app::{AnchorItem, App, FocusedPane, ThreadItem, TreeAction, TuiError};

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

pub fn default_db_path() -> PathBuf {
    if let Ok(val) = std::env::var("GREIN_DB_PATH") {
        let trimmed = val.trim();
        if !trimmed.is_empty() {
            let path = PathBuf::from(trimmed);
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            return path;
        }
    }
    let path = std::env::temp_dir().join(format!("grein-session-{}.sqlite3", std::process::id()));
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    path
}

fn shell_binary() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "/bin/sh".to_string())
}

fn build_shell_path() -> String {
    let current_path = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string());
    if let Ok(home) = std::env::var("HOME") {
        let local_bin = format!("{}/.local/bin", home);
        let cargo_bin = format!("{}/.cargo/bin", home);
        format!("{}:{}:{}", local_bin, cargo_bin, current_path)
    } else {
        current_path
    }
}

pub fn execute_agent_prompt(prompt: &str) -> String {
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    match Command::new(shell_binary())
        .args(["-c", trimmed])
        .env("PATH", build_shell_path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let mut combined = String::new();
            if !stdout.is_empty() {
                combined.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !combined.is_empty() && !combined.ends_with('\n') {
                    combined.push('\n');
                }
                combined.push_str(&stderr);
            }
            let result = combined.trim().to_string();
            if result.is_empty() && !output.status.success() {
                format!("Command exited with status {}", output.status)
            } else {
                result
            }
        }
        Err(err) => format!("Failed to execute command: {}", err),
    }
}

fn parse_turns(transcript_md: &str) -> Vec<(AuthorRole, String)> {
    let mut turns = Vec::new();
    let mut current_role: Option<AuthorRole> = None;
    let mut current_content = String::new();

    for line in transcript_md.lines() {
        if let Some(rest) = line.strip_prefix("# User: ") {
            if let Some(role) = current_role.take() {
                turns.push((role, current_content.trim().to_string()));
                current_content.clear();
            }
            current_role = Some(AuthorRole::User);
            current_content.push_str(rest);
        } else if line == "## Agent (agy)" {
            if let Some(role) = current_role.take() {
                turns.push((role, current_content.trim().to_string()));
                current_content.clear();
            }
            current_role = Some(AuthorRole::Assistant);
        } else if current_role.is_some() {
            if !current_content.is_empty() {
                current_content.push('\n');
            }
            current_content.push_str(line);
        }
    }

    if let Some(role) = current_role.take() {
        turns.push((role, current_content.trim().to_string()));
    }

    turns
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
            .push_str(&format!("# User: {}\n\n", trimmed));
        self.last_submitted_prompt = Some(trimmed.to_string());
        self.sync_anchors_and_blackboard();
    }

    pub fn apply_agent_response(&mut self, response: &str) {
        let clean = response.trim_end_matches('\n');
        let last_user = self.active_transcript_md.rfind("# User: ");
        let last_agent = self.active_transcript_md.rfind("## Agent (agy)\n");
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
                .push_str(&format!("## Agent (agy)\n{}\n\n", clean));
        }
        self.sync_anchors_and_blackboard();
    }

    pub fn sync_anchors_and_blackboard(&mut self) {
        let turns = parse_turns(&self.active_transcript_md);
        if self.threads.is_empty() && !turns.is_empty() {
            let title = turns
                .iter()
                .find(|(role, _)| *role == AuthorRole::User)
                .map(|(_, content)| content.lines().next().unwrap_or("Active Session").to_string())
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
        let turns = parse_turns(&self.active_transcript_md);
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
            match node.role {
                AuthorRole::User => {
                    if first_user_content.is_none() {
                        first_user_content = Some(node.content.clone());
                    }
                    reconstructed.push_str(&format!("# User: {}\n\n", node.content));
                }
                _ => {
                    reconstructed.push_str(&format!("## Agent (agy)\n{}\n\n", node.content));
                }
            }
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

pub fn run<B: Backend>(terminal: &mut Terminal<B>, mut app: App) -> io::Result<()> {
    let db_path = default_db_path();
    app.load_state_from_sqlite(&db_path);

    let mut active_child: Option<(Child, Option<ChildStdin>)> = None;
    let (tx, rx) = std::sync::mpsc::channel::<String>();

    loop {
        while let Ok(line) = rx.try_recv() {
            app.apply_agent_response(&line);
            app.persist_state_to_sqlite(&db_path);
        }

        if let Some((ref mut child, _)) = active_child {
            if let Ok(Some(_status)) = child.try_wait() {
                while let Ok(line) = rx.try_recv() {
                    app.apply_agent_response(&line);
                }
                app.persist_state_to_sqlite(&db_path);
                active_child = None;
            }
        }

        draw_frame(terminal, &app)?;

        if app.should_quit() {
            if let Some((mut child, _)) = active_child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            app.persist_state_to_sqlite(&db_path);
            break;
        }

        if event::poll(Duration::from_millis(25))? {
            if let Event::Key(key) = event::read()? {
                let _ = handle_key_event(&mut app, key);

                if let Some(prompt) = app.last_submitted_prompt.take() {
                    let cmd = prompt.trim();
                    if cmd == "/clear" {
                        app.active_transcript_md.clear();
                        app.persist_state_to_sqlite(&db_path);
                        continue;
                    }
                    if cmd == "/kill" {
                        if let Some((mut child, _)) = active_child.take() {
                            let _ = child.kill();
                            let _ = child.wait();
                            app.apply_agent_response("[Process killed]");
                            app.persist_state_to_sqlite(&db_path);
                        }
                        continue;
                    }

                    let mut piped_to_running = false;
                    if let Some((ref mut child, ref mut stdin_opt)) = active_child {
                        if let Ok(None) = child.try_wait() {
                            if let Some(ref mut stdin) = stdin_opt {
                                if stdin.write_all(format!("{}\n", prompt).as_bytes()).is_ok() {
                                    let _ = stdin.flush();
                                    piped_to_running = true;
                                }
                            }
                        }
                    }

                    if !piped_to_running && !cmd.is_empty() {
                        match Command::new(shell_binary())
                            .args(["-c", cmd])
                            .env("PATH", build_shell_path())
                            .stdin(Stdio::piped())
                            .stdout(Stdio::piped())
                            .stderr(Stdio::piped())
                            .spawn()
                        {
                            Ok(mut child) => {
                                if let Some(stdout) = child.stdout.take() {
                                    let tx_out = tx.clone();
                                    std::thread::spawn(move || {
                                        let reader = BufReader::new(stdout);
                                        for line in reader.lines().map_while(Result::ok) {
                                            let _ = tx_out.send(line);
                                        }
                                    });
                                }
                                if let Some(stderr) = child.stderr.take() {
                                    let tx_err = tx.clone();
                                    std::thread::spawn(move || {
                                        let reader = BufReader::new(stderr);
                                        for line in reader.lines().map_while(Result::ok) {
                                            let _ = tx_err.send(line);
                                        }
                                    });
                                }
                                let child_stdin = child.stdin.take();
                                active_child = Some((child, child_stdin));
                            }
                            Err(err) => {
                                app.apply_agent_response(&format!(
                                    "Failed to spawn shell: {}",
                                    err
                                ));
                            }
                        }
                    }

                    app.persist_state_to_sqlite(&db_path);
                }
            }
        }
    }

    Ok(())
}

pub fn run_default() -> io::Result<()> {
    let _guard = TerminalGuard::new()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    let app = App::new();
    run(&mut terminal, app)
}
