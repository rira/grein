use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use grein_tui::engine_port::{SubagentSnapshot, SubagentStatus, TuiEngineEvent};
use serde_json::Value;
use tokio::sync::Notify;

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(500);
const DEFAULT_REMOVAL_DELAY: Duration = Duration::from_millis(2500);
const RECENT_PARENT_WINDOW: Duration = Duration::from_secs(300);

pub type EventEmitter = Arc<dyn Fn(TuiEngineEvent) + Send + Sync>;

#[derive(Debug, Clone)]
struct TrackedEntry {
    thread_id: String,
    snapshot: SubagentSnapshot,
    child_conversation_id: Option<String>,
    generation: u64,
}

#[derive(Debug, Default)]
struct ParentCursor {
    offset: u64,
    pending_invocations: Vec<(Option<String>, Option<String>)>,
}

#[derive(Debug, Default)]
struct ChildCursor {
    offset: u64,
}

struct WatcherState {
    entries: HashMap<String, TrackedEntry>,
    child_to_subagent: HashMap<String, String>,
    seen_child_ids: HashSet<String>,
    parent_cursors: HashMap<PathBuf, ParentCursor>,
    child_cursors: HashMap<PathBuf, ChildCursor>,
    next_gen: u64,
    polling_active: bool,
    shut_down: bool,
    created_at: SystemTime,
}

impl WatcherState {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            child_to_subagent: HashMap::new(),
            seen_child_ids: HashSet::new(),
            parent_cursors: HashMap::new(),
            child_cursors: HashMap::new(),
            next_gen: 1,
            polling_active: false,
            shut_down: false,
            created_at: SystemTime::now(),
        }
    }

    fn has_running(&self) -> bool {
        self.entries
            .values()
            .any(|e| e.snapshot.status == SubagentStatus::Running)
    }
}

/// Read-only Layer-2 tailer over `~/.gemini/antigravity-cli/brain` transcripts.
pub struct SubagentWatcher {
    brain_root: PathBuf,
    emit: EventEmitter,
    poll_interval: Duration,
    removal_delay: Duration,
    state: Arc<Mutex<WatcherState>>,
    shutdown_notify: Arc<Notify>,
    poll_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl SubagentWatcher {
    pub fn default_brain_root() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join(".gemini")
                .join("antigravity-cli")
                .join("brain")
        })
    }

    pub fn new(brain_root: PathBuf, emit: EventEmitter) -> Arc<Self> {
        Self::with_timings(brain_root, emit, DEFAULT_POLL_INTERVAL, DEFAULT_REMOVAL_DELAY)
    }

    pub fn with_timings(
        brain_root: PathBuf,
        emit: EventEmitter,
        poll_interval: Duration,
        removal_delay: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            brain_root,
            emit,
            poll_interval,
            removal_delay,
            state: Arc::new(Mutex::new(WatcherState::new())),
            shutdown_notify: Arc::new(Notify::new()),
            poll_handle: Mutex::new(None),
        })
    }

    pub fn register_subagent(self: &Arc<Self>, thread_id: &str, snapshot: SubagentSnapshot) {
        let should_spawn = {
            let mut guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if guard.shut_down {
                return;
            }
            let gen_id = guard.next_gen;
            guard.next_gen += 1;
            let key = subagent_key(thread_id, &snapshot.id);
            guard.entries.insert(
                key,
                TrackedEntry {
                    thread_id: thread_id.to_string(),
                    snapshot,
                    child_conversation_id: None,
                    generation: gen_id,
                },
            );
            if !guard.polling_active {
                guard.polling_active = true;
                true
            } else {
                false
            }
        };

        if should_spawn {
            self.spawn_poller();
        }
    }

    /// Associates a child conversationId directly if the ACP `invoke_subagent` tool output contains it.
    pub fn ingest_tool_output(self: &Arc<Self>, thread_id: &str, subagent_id: &str, output: &str) {
        let Some(discovered) = extract_created_subagent(output) else {
            return;
        };
        let key = subagent_key(thread_id, subagent_id);
        let mut guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if guard.entries.contains_key(&key) {
            let cid = discovered.conversation_id;
            guard.seen_child_ids.insert(cid.clone());
            guard.child_to_subagent.insert(cid.clone(), key.clone());
            if let Some(entry) = guard.entries.get_mut(&key) {
                entry.child_conversation_id = Some(cid);
            }
        }
    }

    pub fn update_activity_if_unlinked(&self, thread_id: &str, subagent_id: &str, activity: &str) {
        let key = subagent_key(thread_id, subagent_id);
        let updated = {
            let mut guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
            let Some(entry) = guard.entries.get_mut(&key) else {
                return;
            };
            if entry.child_conversation_id.is_some()
                || entry.snapshot.status != SubagentStatus::Running
            {
                return;
            }
            if entry.snapshot.activity.as_deref() == Some(activity) {
                return;
            }
            entry.snapshot.activity = Some(activity.to_string());
            entry.snapshot.clone()
        };
        (self.emit)(TuiEngineEvent::SubagentUpdated {
            thread_id: thread_id.to_string(),
            subagent: updated,
        });
    }

    pub fn is_confirmed_running(&self, thread_id: &str, subagent_id: &str) -> bool {
        let key = subagent_key(thread_id, subagent_id);
        let guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let Some(entry) = guard.entries.get(&key) else {
            return false;
        };
        if entry.snapshot.status != SubagentStatus::Running {
            return false;
        }
        let Some(ref child_id) = entry.child_conversation_id else {
            return false;
        };
        let transcript_path = child_transcript_path(&self.brain_root, child_id);
        transcript_path.exists()
    }

    pub fn mark_terminal(
        self: &Arc<Self>,
        thread_id: &str,
        subagent_id: &str,
        status: SubagentStatus,
        activity: Option<String>,
    ) {
        let key = subagent_key(thread_id, subagent_id);
        let transition = {
            let mut guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
            let Some(entry) = guard.entries.get_mut(&key) else {
                return;
            };
            if entry.snapshot.status != SubagentStatus::Running && entry.snapshot.status == status {
                return;
            }
            entry.snapshot.status = status;
            if let Some(act) = activity {
                entry.snapshot.activity = Some(act);
            } else {
                entry.snapshot.activity = Some(match status {
                    SubagentStatus::Completed => "done".to_string(),
                    SubagentStatus::Failed => "failed".to_string(),
                    SubagentStatus::Running => "running".to_string(),
                });
            }
            (entry.snapshot.clone(), entry.generation)
        };
        (self.emit)(TuiEngineEvent::SubagentUpdated {
            thread_id: thread_id.to_string(),
            subagent: transition.0,
        });
        self.schedule_removal(thread_id.to_string(), subagent_id.to_string(), transition.1);
    }

    pub fn clear_thread(&self, thread_id: &str) {
        let removed: Vec<(String, String)> = {
            let mut guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
            let keys: Vec<String> = guard
                .entries
                .iter()
                .filter(|(_, e)| e.thread_id == thread_id)
                .map(|(k, _)| k.clone())
                .collect();
            let mut out = Vec::new();
            for k in keys {
                if let Some(entry) = guard.entries.remove(&k) {
                    if let Some(cid) = entry.child_conversation_id {
                        guard.child_to_subagent.remove(&cid);
                    }
                    out.push((entry.thread_id, entry.snapshot.id));
                }
            }
            out
        };
        for (tid, sid) in removed {
            (self.emit)(TuiEngineEvent::SubagentRemoved {
                thread_id: tid,
                subagent_id: sid,
            });
        }
    }

    pub fn shutdown(&self) {
        {
            let mut guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
            guard.shut_down = true;
            guard.polling_active = false;
            guard.entries.clear();
            guard.child_to_subagent.clear();
        }
        self.shutdown_notify.notify_waiters();
        if let Ok(mut handle_guard) = self.poll_handle.lock() {
            if let Some(handle) = handle_guard.take() {
                handle.abort();
            }
        }
    }

    fn schedule_removal(
        self: &Arc<Self>,
        thread_id: String,
        subagent_id: String,
        expected_gen: u64,
    ) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let watcher = Arc::clone(self);
        let delay = self.removal_delay;
        tokio::spawn(async move {
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = watcher.shutdown_notify.notified() => {
                    return;
                }
            }
            let should_emit = {
                let mut guard = watcher.state.lock().unwrap_or_else(|p| p.into_inner());
                if guard.shut_down {
                    return;
                }
                let key = subagent_key(&thread_id, &subagent_id);
                let matches = guard.entries.get(&key).is_some_and(|e| {
                    e.generation == expected_gen && e.snapshot.status != SubagentStatus::Running
                });
                if matches {
                    if let Some(entry) = guard.entries.remove(&key) {
                        if let Some(cid) = entry.child_conversation_id {
                            guard.child_to_subagent.remove(&cid);
                        }
                    }
                    true
                } else {
                    false
                }
            };
            if should_emit {
                (watcher.emit)(TuiEngineEvent::SubagentRemoved {
                    thread_id,
                    subagent_id,
                });
            }
        });
    }

    fn spawn_poller(self: &Arc<Self>) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let watcher = Arc::clone(self);
        let handle = tokio::spawn(async move {
            loop {
                let w = Arc::clone(&watcher);
                let _ = tokio::task::spawn_blocking(move || {
                    w.poll_once();
                })
                .await;

                let should_stop = {
                    let mut guard = watcher.state.lock().unwrap_or_else(|p| p.into_inner());
                    if guard.shut_down || !guard.has_running() {
                        guard.polling_active = false;
                        true
                    } else {
                        false
                    }
                };
                if should_stop {
                    break;
                }

                tokio::select! {
                    _ = tokio::time::sleep(watcher.poll_interval) => {}
                    _ = watcher.shutdown_notify.notified() => {
                        let mut guard = watcher.state.lock().unwrap_or_else(|p| p.into_inner());
                        guard.polling_active = false;
                        break;
                    }
                }
            }
        });
        if let Ok(mut handle_guard) = self.poll_handle.lock() {
            *handle_guard = Some(handle);
        }
    }

    pub fn poll_once(self: &Arc<Self>) {
        if !self.brain_root.is_dir() {
            return;
        }
        let Ok(dir_entries) = fs::read_dir(&self.brain_root) else {
            return;
        };

        let mut events_to_emit: Vec<TuiEngineEvent> = Vec::new();
        let mut removals_to_schedule: Vec<(String, String, u64)> = Vec::new();

        {
            let mut guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if guard.shut_down || !guard.has_running() {
                return;
            }

            let cutoff = guard
                .created_at
                .checked_sub(RECENT_PARENT_WINDOW)
                .unwrap_or(SystemTime::UNIX_EPOCH);

            let mut candidate_transcripts: Vec<(String, PathBuf)> = Vec::new();
            for entry in dir_entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let Some(conv_id) = path.file_name().and_then(|n| n.to_str()).map(str::to_string)
                else {
                    continue;
                };
                let t_path = child_transcript_path(&self.brain_root, &conv_id);
                if t_path.is_file() {
                    candidate_transcripts.push((conv_id, t_path));
                }
            }

            let stale_child_cutoff = guard
                .created_at
                .checked_sub(Duration::from_secs(30))
                .unwrap_or(SystemTime::UNIX_EPOCH);

            let mut discovered_senders: Vec<String> = Vec::new();

            // 1. Scan recently modified non-child transcripts for subagent creation & wakeup messages.
            for (conv_id, t_path) in &candidate_transcripts {
                if guard.child_to_subagent.contains_key(conv_id)
                    || guard.seen_child_ids.contains(conv_id)
                {
                    continue;
                }
                let modified_recent = fs::metadata(t_path)
                    .and_then(|m| m.modified())
                    .map(|mt| mt >= cutoff)
                    .unwrap_or(true);
                if !modified_recent && !guard.parent_cursors.contains_key(t_path) {
                    continue;
                }

                let mut cursor = guard.parent_cursors.remove(t_path).unwrap_or_default();
                let (new_offset, lines) = read_complete_lines_from(t_path, cursor.offset);
                cursor.offset = new_offset;

                for line in lines {
                    let Ok(val) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    let full_val = resolve_truncated_line(t_path, &val);
                    let item = full_val.as_ref().unwrap_or(&val);

                    // Collect any invoke_subagent tool calls for role/type pairing.
                    if let Some(calls) = item.get("tool_calls").and_then(Value::as_array) {
                        for tc in calls {
                            let tc_name = tc.get("name").and_then(Value::as_str).unwrap_or("");
                            if tc_name.contains("invoke_subagent") {
                                let args = tc.get("args");
                                let role = args
                                    .and_then(|a| a.get("Role").or_else(|| a.get("role")))
                                    .and_then(Value::as_str)
                                    .map(|s| s.trim_matches('"').trim().to_string())
                                    .filter(|s| !s.is_empty());
                                let ty = args
                                    .and_then(|a| a.get("TypeName").or_else(|| a.get("typeName")))
                                    .and_then(Value::as_str)
                                    .map(|s| s.trim_matches('"').trim().to_string())
                                    .filter(|s| !s.is_empty());
                                cursor.pending_invocations.push((role, ty));
                            }
                        }
                    }

                    if let Some(content) = item.get("content").and_then(Value::as_str) {
                        if let Some(mut discovered) = extract_created_subagent(content) {
                            let child_dir = self.brain_root.join(&discovered.conversation_id);
                            let child_is_stale = fs::metadata(&child_dir)
                                .and_then(|m| m.modified())
                                .map(|mt| mt < stale_child_cutoff)
                                .unwrap_or(false);
                            if !child_is_stale
                                && !guard.seen_child_ids.contains(&discovered.conversation_id)
                            {
                                if discovered.role.is_none() || discovered.agent_type.is_none() {
                                    if let Some((p_role, p_type)) =
                                        (!cursor.pending_invocations.is_empty())
                                            .then(|| cursor.pending_invocations.remove(0))
                                    {
                                        if discovered.role.is_none() {
                                            discovered.role = p_role;
                                        }
                                        if discovered.agent_type.is_none() {
                                            discovered.agent_type = p_type;
                                        }
                                    }
                                }
                                let cid = discovered.conversation_id.clone();
                                if let Some(matched_key) = find_matching_unlinked_key(
                                    &guard.entries,
                                    discovered.role.as_deref(),
                                    discovered.agent_type.as_deref(),
                                ) {
                                    guard.seen_child_ids.insert(cid.clone());
                                    guard.child_to_subagent.insert(cid.clone(), matched_key.clone());
                                    if let Some(entry) = guard.entries.get_mut(&matched_key) {
                                        entry.child_conversation_id = Some(cid);
                                    }
                                }
                            }
                        }

                        discovered_senders.extend(extract_message_senders(content));
                    }
                }
                guard.parent_cursors.insert(t_path.clone(), cursor);
            }

            // 2. Tail linked child transcripts incrementally for model, live activity, and terminal state.
            let linked_keys: Vec<(String, String)> = guard
                .entries
                .iter()
                .filter_map(|(k, e)| {
                    (e.snapshot.status == SubagentStatus::Running)
                        .then(|| e.child_conversation_id.clone().map(|cid| (k.clone(), cid)))
                        .flatten()
                })
                .collect();

            for (sub_key, child_id) in linked_keys {
                let child_path = child_transcript_path(&self.brain_root, &child_id);
                if !child_path.is_file() {
                    continue;
                }
                let prev_offset = guard
                    .child_cursors
                    .get(&child_path)
                    .map(|c| c.offset)
                    .unwrap_or(0);
                let (new_offset, lines) = read_complete_lines_from(&child_path, prev_offset);
                guard
                    .child_cursors
                    .entry(child_path.clone())
                    .or_default()
                    .offset = new_offset;

                if lines.is_empty() {
                    continue;
                }

                let Some(entry) = guard.entries.get_mut(&sub_key) else {
                    continue;
                };
                let mut changed = false;

                for line in lines {
                    let Ok(val) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    if let Some(raw_model) = val.get("model").and_then(Value::as_str) {
                        let short = shorten_model_name(raw_model);
                        if !short.is_empty() && entry.snapshot.model.as_deref() != Some(&short) {
                            entry.snapshot.model = Some(short);
                            changed = true;
                        }
                    }
                    if let Some(act) = extract_child_activity(&val) {
                        if entry.snapshot.activity.as_deref() != Some(&act) {
                            entry.snapshot.activity = Some(act);
                            changed = true;
                        }
                    }
                    if let Some((term_status, term_act)) = detect_child_terminal_state(&val) {
                        entry.snapshot.status = term_status;
                        entry.snapshot.activity = Some(term_act);
                        changed = true;
                        removals_to_schedule.push((
                            entry.thread_id.clone(),
                            entry.snapshot.id.clone(),
                            entry.generation,
                        ));
                        break;
                    }
                }

                if changed {
                    events_to_emit.push(TuiEngineEvent::SubagentUpdated {
                        thread_id: entry.thread_id.clone(),
                        subagent: entry.snapshot.clone(),
                    });
                }
            }

            // 3. Apply parent [Message] sender=<child_id> completions after child details are read.
            for sender_id in discovered_senders {
                if let Some(sub_key) = guard.child_to_subagent.get(&sender_id).cloned() {
                    if let Some(entry) = guard.entries.get_mut(&sub_key) {
                        if entry.snapshot.status == SubagentStatus::Running {
                            entry.snapshot.status = SubagentStatus::Completed;
                            entry.snapshot.activity = Some("done".to_string());
                            events_to_emit.push(TuiEngineEvent::SubagentUpdated {
                                thread_id: entry.thread_id.clone(),
                                subagent: entry.snapshot.clone(),
                            });
                            removals_to_schedule.push((
                                entry.thread_id.clone(),
                                entry.snapshot.id.clone(),
                                entry.generation,
                            ));
                        }
                    }
                }
            }
        }

        for ev in events_to_emit {
            (self.emit)(ev);
        }
        for (tid, sid, gen_id) in removals_to_schedule {
            self.schedule_removal(tid, sid, gen_id);
        }
    }
}

impl Drop for SubagentWatcher {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn subagent_key(thread_id: &str, subagent_id: &str) -> String {
    format!("{thread_id}::{subagent_id}")
}

fn child_transcript_path(brain_root: &Path, conversation_id: &str) -> PathBuf {
    brain_root
        .join(conversation_id)
        .join(".system_generated")
        .join("logs")
        .join("transcript.jsonl")
}

fn resolve_truncated_line(transcript_path: &Path, val: &Value) -> Option<Value> {
    let has_truncated = val
        .get("truncated_fields")
        .and_then(Value::as_array)
        .is_some_and(|arr| !arr.is_empty());
    if !has_truncated {
        return None;
    }
    let step_idx = val.get("step_index").and_then(Value::as_u64)?;
    let full_path = transcript_path.with_file_name("transcript_full.jsonl");
    let content = fs::read_to_string(full_path).ok()?;
    for line in content.lines() {
        if let Ok(full_val) = serde_json::from_str::<Value>(line) {
            if full_val.get("step_index").and_then(Value::as_u64) == Some(step_idx) {
                return Some(full_val);
            }
        }
    }
    None
}

fn read_complete_lines_from(path: &Path, prev_offset: u64) -> (u64, Vec<String>) {
    let Ok(mut file) = File::open(path) else {
        return (prev_offset, Vec::new());
    };
    let Ok(meta) = file.metadata() else {
        return (prev_offset, Vec::new());
    };
    let len = meta.len();
    let start = if len < prev_offset { 0 } else { prev_offset };
    if len == start {
        return (start, Vec::new());
    }
    if file.seek(SeekFrom::Start(start)).is_err() {
        return (start, Vec::new());
    }
    let mut buf = Vec::with_capacity((len - start) as usize);
    if file.read_to_end(&mut buf).is_err() {
        return (start, Vec::new());
    }
    let Some(last_nl) = buf.iter().rposition(|&b| b == b'\n') else {
        return (start, Vec::new());
    };
    let complete_slice = &buf[..=last_nl];
    let text = String::from_utf8_lossy(complete_slice);
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    (start + (last_nl as u64) + 1, lines)
}

#[derive(Debug, PartialEq, Eq)]
pub struct DiscoveredSubagent {
    pub conversation_id: String,
    pub role: Option<String>,
    pub agent_type: Option<String>,
}

pub fn extract_created_subagent(text: &str) -> Option<DiscoveredSubagent> {
    const MARKER: &str = "Created the following subagent:";
    let idx = text.find(MARKER)?;
    let rest = &text[idx + MARKER.len()..];
    let brace_start = rest.find('{')?;
    let json_candidate = extract_balanced_json_object(&rest[brace_start..])?;
    let val: Value = serde_json::from_str(json_candidate).ok()?;
    let conversation_id = val
        .get("conversationId")
        .or_else(|| val.get("conversation_id"))
        .or_else(|| val.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())?
        .to_string();
    let role = val
        .get("role")
        .or_else(|| val.get("Role"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let agent_type = val
        .get("typeName")
        .or_else(|| val.get("TypeName"))
        .or_else(|| val.get("agentType"))
        .or_else(|| val.get("type"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Some(DiscoveredSubagent {
        conversation_id,
        role,
        agent_type,
    })
}

fn extract_balanced_json_object(s: &str) -> Option<&str> {
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    for (idx, ch) in s.char_indices() {
        if in_str {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_str = false;
            }
            continue;
        }
        match ch {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(&s[..=idx]);
                }
            }
            _ => {}
        }
    }
    None
}

fn extract_message_senders(content: &str) -> Vec<String> {
    let mut senders = Vec::new();
    for line in content.lines() {
        if !line.contains("[Message]") {
            continue;
        }
        if let Some(pos) = line.find("sender=") {
            let after = &line[pos + "sender=".len()..];
            let id: String = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            if !id.is_empty() {
                senders.push(id);
            }
        }
    }
    senders
}

fn find_matching_unlinked_key(
    entries: &HashMap<String, TrackedEntry>,
    role: Option<&str>,
    agent_type: Option<&str>,
) -> Option<String> {
    let mut unlinked: Vec<(&String, &TrackedEntry)> = entries
        .iter()
        .filter(|(_, e)| {
            e.snapshot.status == SubagentStatus::Running && e.child_conversation_id.is_none()
        })
        .collect();
    unlinked.sort_by_key(|(_, e)| e.generation);

    if let (Some(r), Some(t)) = (role, agent_type) {
        if let Some((k, _)) = unlinked.iter().find(|(_, e)| {
            e.snapshot.role.eq_ignore_ascii_case(r)
                && e.snapshot.agent_type.eq_ignore_ascii_case(t)
        }) {
            return Some((*k).clone());
        }
    }
    if let Some(r) = role {
        if let Some((k, _)) = unlinked
            .iter()
            .find(|(_, e)| e.snapshot.role.eq_ignore_ascii_case(r))
        {
            return Some((*k).clone());
        }
    }
    if let Some(t) = agent_type {
        let matches: Vec<_> = unlinked
            .iter()
            .filter(|(_, e)| e.snapshot.agent_type.eq_ignore_ascii_case(t))
            .collect();
        if matches.len() == 1 {
            return Some(matches[0].0.clone());
        }
    }
    unlinked.first().map(|(k, _)| (*k).clone())
}

pub fn shorten_model_name(raw: &str) -> String {
    let base = raw.trim().rsplit('/').next().unwrap_or(raw.trim());
    let without_at = base.split('@').next().unwrap_or(base);
    let mut result = without_at.to_string();

    if let Some(idx) = result.find("-preview") {
        result.truncate(idx);
    }
    if let Some(idx) = result.find("-exp-") {
        result.truncate(idx);
    }
    if let Some((head, tail)) = result.rsplit_once('-') {
        if tail.len() == 8 && tail.chars().all(|c| c.is_ascii_digit()) {
            result = head.to_string();
        }
    }
    result
}

fn strip_extra_quotes(s: &str) -> &str {
    s.trim().trim_matches('"')
}

fn format_tool_activity(tc: &Value) -> Option<String> {
    let name = tc
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let args = tc.get("args");
    let detail = args.and_then(|a| {
        if let Some(p) = a.get("Path").or_else(|| a.get("path")).and_then(Value::as_str) {
            let clean = strip_extra_quotes(p);
            return Path::new(clean)
                .file_name()
                .and_then(|f| f.to_str())
                .map(str::to_string)
                .filter(|s| !s.is_empty());
        }
        if let Some(cmd) = a
            .get("CommandLine")
            .or_else(|| a.get("command"))
            .and_then(Value::as_str)
        {
            let clean = strip_extra_quotes(cmd);
            return Some(truncate_chars(clean, 28));
        }
        if let Some(q) = a.get("query").or_else(|| a.get("Query")).and_then(Value::as_str) {
            let clean = strip_extra_quotes(q);
            return Some(truncate_chars(clean, 28));
        }
        None
    });
    Some(match detail {
        Some(d) if !d.is_empty() => format!("{name} {d}"),
        _ => name.to_string(),
    })
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let single_line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = single_line.chars();
    let taken: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() && max_chars > 1 {
        let mut out: String = taken.chars().take(max_chars - 1).collect();
        out.push('…');
        out
    } else {
        taken
    }
}

fn extract_child_activity(val: &Value) -> Option<String> {
    if let Some(calls) = val.get("tool_calls").and_then(Value::as_array) {
        if let Some(last_tc) = calls.last() {
            if let Some(act) = format_tool_activity(last_tc) {
                return Some(act);
            }
        }
    }
    if let Some(name) = val
        .get("tool_name")
        .or_else(|| val.get("name"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    {
        return Some(name.trim().to_string());
    }
    if let Some(thinking) = val.get("thinking").and_then(Value::as_str) {
        let snippet = truncate_chars(thinking, 36);
        if !snippet.is_empty() {
            return Some(format!("thinking: {snippet}"));
        }
    }
    None
}

fn detect_child_terminal_state(val: &Value) -> Option<(SubagentStatus, String)> {
    let ty = val.get("type").and_then(Value::as_str).unwrap_or("");
    let status = val.get("status").and_then(Value::as_str).unwrap_or("");

    if ty.eq_ignore_ascii_case("ERROR")
        || ty.eq_ignore_ascii_case("FAILED")
        || ty.eq_ignore_ascii_case("TASK_FAILED")
        || status.eq_ignore_ascii_case("ERROR")
        || status.eq_ignore_ascii_case("FAILED")
    {
        let err_act = val
            .get("error")
            .or_else(|| val.get("content"))
            .and_then(Value::as_str)
            .map(|s| truncate_chars(s, 40))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "failed".to_string());
        return Some((SubagentStatus::Failed, err_act));
    }

    if ty.eq_ignore_ascii_case("COMPLETED")
        || ty.eq_ignore_ascii_case("FINAL_RESPONSE")
        || ty.eq_ignore_ascii_case("TURN_COMPLETE")
        || ty.eq_ignore_ascii_case("TASK_COMPLETE")
    {
        return Some((SubagentStatus::Completed, "done".to_string()));
    }

    if ty == "PLANNER_RESPONSE" && status.eq_ignore_ascii_case("DONE") {
        let no_tool_calls = val
            .get("tool_calls")
            .and_then(Value::as_array)
            .is_none_or(|arr| arr.is_empty());
        let has_final_text = val
            .get("response")
            .or_else(|| val.get("content"))
            .or_else(|| val.get("text"))
            .and_then(Value::as_str)
            .is_some_and(|s| !s.trim().is_empty());
        if no_tool_calls && has_final_text {
            return Some((SubagentStatus::Completed, "done".to_string()));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write_jsonl(path: &Path, lines: &[Value]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        let mut body = String::new();
        for l in lines {
            body.push_str(&serde_json::to_string(l).expect("json"));
            body.push('\n');
        }
        fs::write(path, body).expect("write jsonl");
    }

    fn append_jsonl(path: &Path, line: &Value) {
        use std::io::Write;
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("open append");
        writeln!(f, "{}", serde_json::to_string(line).expect("json")).expect("append line");
    }

    #[test]
    fn missing_brain_root_no_ops_gracefully() {
        let temp = tempfile::tempdir().expect("tempdir");
        let missing = temp.path().join("does-not-exist");
        let events = Arc::new(Mutex::new(Vec::<TuiEngineEvent>::new()));
        let events_cb = Arc::clone(&events);
        let watcher = SubagentWatcher::with_timings(
            missing,
            Arc::new(move |ev| events_cb.lock().unwrap().push(ev)),
            Duration::from_millis(50),
            Duration::from_millis(20),
        );

        watcher.register_subagent(
            "t-1",
            SubagentSnapshot {
                id: "tc-1".to_string(),
                role: "Codebase Researcher".to_string(),
                agent_type: "research".to_string(),
                model: None,
                activity: Some("running".to_string()),
                status: SubagentStatus::Running,
                started_at_ms: 1000,
            },
        );
        watcher.poll_once();
        assert!(!watcher.is_confirmed_running("t-1", "tc-1"));
        assert!(events.lock().unwrap().is_empty());
        watcher.shutdown();
    }

    #[test]
    fn model_name_shortening_handles_common_variants() {
        assert_eq!(shorten_model_name("gemini-2.5-pro"), "gemini-2.5-pro");
        assert_eq!(
            shorten_model_name("models/gemini-2.5-pro-preview-06-05"),
            "gemini-2.5-pro"
        );
        assert_eq!(
            shorten_model_name("claude-3-7-sonnet-20250219"),
            "claude-3-7-sonnet"
        );
        assert_eq!(
            shorten_model_name("claude-sonnet-4-5@20250929"),
            "claude-sonnet-4-5"
        );
    }

    #[tokio::test]
    async fn extracts_model_live_activity_and_parent_sender_completion() {
        let temp = tempfile::tempdir().expect("tempdir");
        let brain = temp.path().join("brain");
        let parent_id = "parent-conv-1";
        let child_id = "child-conv-99";
        let parent_log = brain
            .join(parent_id)
            .join(".system_generated/logs/transcript.jsonl");
        let child_log = brain
            .join(child_id)
            .join(".system_generated/logs/transcript.jsonl");

        write_jsonl(
            &parent_log,
            &[
                json!({
                    "step_index": 1,
                    "type": "PLANNER_RESPONSE",
                    "status": "DONE",
                    "tool_calls": [{
                        "name": "invoke_subagent",
                        "args": {"Role": "Database Auditor", "TypeName": "research"}
                    }]
                }),
                json!({
                    "step_index": 2,
                    "type": "GENERIC",
                    "status": "DONE",
                    "content": format!("Created the following subagent:\n{{\"conversationId\":\"{child_id}\"}}")
                }),
            ],
        );

        write_jsonl(
            &child_log,
            &[json!({
                "step_index": 1,
                "type": "PLANNER_RESPONSE",
                "status": "DONE",
                "model": "models/gemini-2.5-pro-preview-05-06",
                "thinking": "Inspecting schema files",
                "tool_calls": [{
                    "name": "read_file",
                    "args": {"Path": "/repo/crates/grein-core/src/storage/sqlite.rs"}
                }]
            })],
        );

        let events = Arc::new(Mutex::new(Vec::<TuiEngineEvent>::new()));
        let events_cb = Arc::clone(&events);
        let watcher = SubagentWatcher::with_timings(
            brain.clone(),
            Arc::new(move |ev| events_cb.lock().unwrap().push(ev)),
            Duration::from_millis(50),
            Duration::from_millis(40),
        );

        watcher.register_subagent(
            "t-main",
            SubagentSnapshot {
                id: "tc-sub-1".to_string(),
                role: "Database Auditor".to_string(),
                agent_type: "research".to_string(),
                model: None,
                activity: Some("running".to_string()),
                status: SubagentStatus::Running,
                started_at_ms: 1000,
            },
        );

        watcher.poll_once();
        assert!(watcher.is_confirmed_running("t-main", "tc-sub-1"));

        {
            let evs = events.lock().unwrap();
            assert!(evs.iter().any(|e| matches!(
                e,
                TuiEngineEvent::SubagentUpdated { thread_id, subagent }
                    if thread_id == "t-main"
                        && subagent.id == "tc-sub-1"
                        && subagent.model.as_deref() == Some("gemini-2.5-pro")
                        && subagent.activity.as_deref() == Some("read_file sqlite.rs")
                        && subagent.status == SubagentStatus::Running
            )));
        }

        // Parent receives wakeup [Message] sender=<child_id> -> marks Completed and schedules removal.
        append_jsonl(
            &parent_log,
            &json!({
                "step_index": 3,
                "type": "SYSTEM_MESSAGE",
                "status": "DONE",
                "content": format!("[Message] timestamp=2026-11-22T10:30:00Z sender={child_id} priority=HIGH content=Audit complete")
            }),
        );

        watcher.poll_once();
        assert!(!watcher.is_confirmed_running("t-main", "tc-sub-1"));
        {
            let evs = events.lock().unwrap();
            assert!(evs.iter().any(|e| matches!(
                e,
                TuiEngineEvent::SubagentUpdated { subagent, .. }
                    if subagent.id == "tc-sub-1" && subagent.status == SubagentStatus::Completed
            )));
        }

        tokio::time::sleep(Duration::from_millis(120)).await;
        {
            let evs = events.lock().unwrap();
            assert!(evs.iter().any(|e| matches!(
                e,
                TuiEngineEvent::SubagentRemoved { thread_id, subagent_id }
                    if thread_id == "t-main" && subagent_id == "tc-sub-1"
            )));
        }
        watcher.shutdown();
    }

    #[tokio::test]
    async fn detects_child_terminal_completion_and_stops_polling_when_empty() {
        let temp = tempfile::tempdir().expect("tempdir");
        let brain = temp.path().join("brain");
        let child_id = "child-direct-42";
        let child_log = brain
            .join(child_id)
            .join(".system_generated/logs/transcript.jsonl");

        write_jsonl(
            &child_log,
            &[json!({
                "step_index": 1,
                "type": "PLANNER_RESPONSE",
                "status": "DONE",
                "model": "claude-sonnet-4-5@20250929",
                "thinking": "Analyzing logs"
            })],
        );

        let events = Arc::new(Mutex::new(Vec::<TuiEngineEvent>::new()));
        let events_cb = Arc::clone(&events);
        let watcher = SubagentWatcher::with_timings(
            brain.clone(),
            Arc::new(move |ev| events_cb.lock().unwrap().push(ev)),
            Duration::from_millis(30),
            Duration::from_millis(30),
        );

        watcher.register_subagent(
            "t-1",
            SubagentSnapshot {
                id: "tc-2".to_string(),
                role: "Log Analyzer".to_string(),
                agent_type: "research".to_string(),
                model: None,
                activity: Some("running".to_string()),
                status: SubagentStatus::Running,
                started_at_ms: 1000,
            },
        );
        watcher.ingest_tool_output(
            "t-1",
            "tc-2",
            &format!("Created the following subagent:\n{{\"conversationId\":\"{child_id}\"}}"),
        );

        tokio::time::sleep(Duration::from_millis(90)).await;
        assert!(watcher.is_confirmed_running("t-1", "tc-2"));

        append_jsonl(
            &child_log,
            &json!({
                "step_index": 2,
                "type": "PLANNER_RESPONSE",
                "status": "DONE",
                "model": "claude-sonnet-4-5@20250929",
                "response": "All logs look healthy.",
                "tool_calls": []
            }),
        );

        tokio::time::sleep(Duration::from_millis(160)).await;
        let evs = events.lock().unwrap().clone();
        assert!(evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::SubagentUpdated { subagent, .. }
                if subagent.id == "tc-2"
                    && subagent.model.as_deref() == Some("claude-sonnet-4-5")
                    && subagent.status == SubagentStatus::Completed
        )));
        assert!(evs.iter().any(|e| matches!(
            e,
            TuiEngineEvent::SubagentRemoved { subagent_id, .. } if subagent_id == "tc-2"
        )));
        // Polling task should automatically stop once no subagents are running.
        assert!(!watcher.state.lock().unwrap().polling_active);
    }
}
