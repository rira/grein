use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use grein_tui::engine_port::{SubagentSnapshot, SubagentStatus, TuiEngineEvent};
use serde_json::Value;
use tokio::sync::Notify;

mod parser;
#[cfg(test)]
mod tests;

pub use parser::{extract_created_subagent, shorten_model_name, DiscoveredSubagent};

use parser::{
    child_transcript_path, detect_child_terminal_state, extract_child_activity,
    extract_message_senders, find_matching_unlinked_key, read_complete_lines_from,
    resolve_truncated_line, subagent_key,
};

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(500);
const DEFAULT_REMOVAL_DELAY: Duration = Duration::from_millis(2500);
const RECENT_PARENT_WINDOW: Duration = Duration::from_secs(300);

pub type EventEmitter = Arc<dyn Fn(TuiEngineEvent) + Send + Sync>;

#[derive(Debug, Clone)]
pub(super) struct TrackedEntry {
    pub(super) thread_id: String,
    pub(super) snapshot: SubagentSnapshot,
    pub(super) child_conversation_id: Option<String>,
    pub(super) generation: u64,
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

pub(super) struct WatcherState {
    entries: HashMap<String, TrackedEntry>,
    child_to_subagent: HashMap<String, String>,
    seen_child_ids: HashSet<String>,
    parent_cursors: HashMap<PathBuf, ParentCursor>,
    child_cursors: HashMap<PathBuf, ChildCursor>,
    next_gen: u64,
    pub(super) polling_active: bool,
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
    pub(super) state: Arc<Mutex<WatcherState>>,
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
