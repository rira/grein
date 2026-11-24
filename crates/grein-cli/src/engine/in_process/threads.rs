use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use grein_core::error::CoreError;
use grein_core::model::{
    Anchor, AnchorId, AuthorRole, ConversationNode, NodeId, Thread, ThreadId, ThreadKind,
    ThreadStatus,
};
use grein_core::port::dto::{EngineStreamEvent, HydratedThread, HydratedTurn, ThreadMeta};
use grein_core::sync::synthesize_thread_merge;
use grein_workspace::git_status::RepoGitStatus;
use tokio::sync::oneshot;

use super::super::types::{
    is_internal_root_node, node_to_transcript_entry, normalize_custom_name, path_from_node_map,
    parse_typed_id, slugify_anchor, storage_err, thread_to_summary, truncate_chars_with_ellipsis,
    EngineError, ThreadSummary, TranscriptEntry, CLOSE_SESSION_TIMEOUT, INTERNAL_ROOT_CONTENT,
};
use super::{EngineRuntime, InProcessEngine, SessionListenerCmd};

impl EngineRuntime {
    pub(super) fn load_thread_turns(
        &self,
        thread: &Thread,
    ) -> Result<Vec<TranscriptEntry>, EngineError> {
        let path = self
            .storage
            .get_trunk_path(&thread.active_leaf_id)
            .map_err(storage_err)?;
        let threads = self.storage.list_threads().map_err(storage_err)?;
        let is_first_thread = threads.first().is_some_and(|first| first.id == thread.id);

        let skip_count = match path.iter().position(|n| n.id == thread.parent_node_id) {
            Some(0) if is_first_thread && !is_internal_root_node(&path[0]) => 0,
            Some(idx) => idx + 1,
            None => 0,
        };

        Ok(path
            .into_iter()
            .skip(skip_count)
            .filter(|n| !is_internal_root_node(n))
            .map(|n| node_to_transcript_entry(&n))
            .collect())
    }

    pub(super) fn compute_thread_metadata(&self) -> Result<Vec<ThreadMeta>, EngineError> {
        let threads = self.storage.list_threads().map_err(storage_err)?;
        if threads.is_empty() {
            return Ok(Vec::new());
        }
        let known_ids: HashSet<ThreadId> = threads.iter().map(|t| t.id).collect();
        let nodes = self.storage.list_nodes().map_err(storage_err)?;
        let nodes_by_id: HashMap<NodeId, ConversationNode> =
            nodes.into_iter().map(|n| (n.id, n)).collect();

        let mut node_owner: HashMap<NodeId, ThreadId> = HashMap::new();

        for (idx, thread) in threads.iter().enumerate() {
            let path = path_from_node_map(thread.active_leaf_id, &nodes_by_id);
            let parent_pos = path.iter().position(|n| n.id == thread.parent_node_id);

            if idx == 0 || path.first().is_some_and(|n| n.id == thread.parent_node_id) {
                for node in &path {
                    node_owner.entry(node.id).or_insert(thread.id);
                }
            } else if let Some(p_idx) = parent_pos {
                for node in path.iter().skip(p_idx + 1) {
                    node_owner.entry(node.id).or_insert(thread.id);
                }
            } else {
                for node in &path {
                    node_owner.entry(node.id).or_insert(thread.id);
                }
            }
        }

        let mut git_cache: HashMap<PathBuf, Option<RepoGitStatus>> = HashMap::new();
        let mut meta = Vec::with_capacity(threads.len());
        for (idx, thread) in threads.iter().enumerate() {
            let trunk = path_from_node_map(thread.parent_node_id, &nodes_by_id);
            let is_offline_root_main = idx == 0
                && trunk.len() == 1
                && trunk[0].id == thread.parent_node_id
                && !is_internal_root_node(trunk[0]);

            let parent_thread_id = if let Some(explicit_pid) = thread
                .parent_thread_id
                .filter(|pid| *pid != thread.id && known_ids.contains(pid))
            {
                Some(explicit_pid.0.to_string())
            } else if idx == 0
                && trunk.len() <= 1
                && trunk.first().is_some_and(|n| n.id == thread.parent_node_id)
            {
                None
            } else {
                let mut found = None;
                let mut cur = Some(thread.parent_node_id);
                let mut visited = HashSet::new();
                while let Some(nid) = cur {
                    if !visited.insert(nid) {
                        break;
                    }
                    if let Some(owner) = node_owner.get(&nid) {
                        if *owner != thread.id {
                            found = Some(owner.0.to_string());
                            break;
                        }
                    }
                    cur = nodes_by_id.get(&nid).and_then(|n| n.parent_id);
                }
                found
            };

            let inherited_turn_count = if is_offline_root_main {
                0
            } else {
                trunk
                    .iter()
                    .filter(|n| !is_internal_root_node(n))
                    .count()
            };

            let target_path = thread
                .cwd
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| self.cwd.clone());
            let git_status = git_cache
                .entry(target_path)
                .or_insert_with_key(|path| self.git_inspector.inspect_git_status(path))
                .as_ref();
            let git_branch = git_status.and_then(|s| s.branch.clone());
            let git_dirty = git_status.map_or(0, |s| s.dirty_count);

            meta.push(ThreadMeta {
                thread_id: thread.id.0.to_string(),
                parent_thread_id,
                inherited_turn_count,
                merged: thread.merged,
                kind: thread.kind,
                cwd: thread.cwd.clone(),
                git_branch,
                git_dirty,
                custom_name: normalize_custom_name(thread.custom_name.as_deref()),
                agent_provider: thread.agent_provider.clone(),
            });
        }

        Ok(meta)
    }

    pub(super) fn insert_new_thread_with_anchor(
        &self,
        parent_node_id: NodeId,
        default_title: String,
        custom_name: Option<String>,
        parent_thread_id: Option<ThreadId>,
        kind: ThreadKind,
        cwd: Option<String>,
    ) -> Result<Thread, EngineError> {
        let anchor_title = custom_name.as_deref().unwrap_or(&default_title);
        let anchor_id = AnchorId::new();
        let anchor = Anchor {
            level: 1,
            title: anchor_title.to_string(),
            slug: slugify_anchor(anchor_title),
            line_number: 1,
        };
        self.storage
            .insert_anchor(&anchor_id.0, &anchor)
            .map_err(storage_err)?;

        let mut thread = Thread::new(
            ThreadId::new_v7(),
            parent_node_id,
            anchor_id,
            parent_node_id,
            ThreadStatus::Idle,
        )
        .with_kind(kind)
        .with_title(default_title)
        .with_custom_name(custom_name);
        thread.parent_thread_id = parent_thread_id;
        if let Some(dir) = cwd {
            thread = thread.with_cwd(dir);
        }
        self.storage.insert_thread(&thread).map_err(storage_err)?;
        self.storage
            .get_thread(&thread.id)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::Storage("created thread missing".to_string()))
    }
}

impl InProcessEngine {
    pub(super) async fn ensure_main_thread_impl(&self) -> Result<ThreadSummary, EngineError> {
        let mut state = self.runtime.lock()?;
        let threads = self.runtime.storage.list_threads().map_err(storage_err)?;
        if let Some(first_agent) = threads.iter().find(|t| t.kind == ThreadKind::Agent) {
            let is_first = threads.first().is_some_and(|t| t.id == first_agent.id);
            return Ok(thread_to_summary(first_agent, is_first));
        }

        let nodes = self.runtime.storage.list_nodes().map_err(storage_err)?;
        let (root_id, leaf_id) = if let (Some(first), Some(last)) = (nodes.first(), nodes.last()) {
            (first.id, last.id)
        } else {
            let root = ConversationNode::new(
                NodeId::new(),
                None::<NodeId>,
                AuthorRole::System,
                INTERNAL_ROOT_CONTENT.to_string(),
            );
            self.runtime
                .storage
                .insert_node(&root)
                .map_err(storage_err)?;
            (root.id, root.id)
        };

        let anchor_id = AnchorId::new();
        let anchor = Anchor {
            level: 1,
            title: "Main".to_string(),
            slug: "main".to_string(),
            line_number: 1,
        };
        self.runtime
            .storage
            .insert_anchor(&anchor_id.0, &anchor)
            .map_err(storage_err)?;
        let tid = self
            .runtime
            .storage
            .create_thread_with_title(root_id, anchor_id, "Main")
            .map_err(storage_err)?;
        if leaf_id != root_id {
            self.runtime
                .storage
                .update_thread_leaf(&tid, &leaf_id)
                .map_err(storage_err)?;
        }
        state.last_node = Some(leaf_id);
        let created = self
            .runtime
            .storage
            .get_thread(&tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::Storage("created Main thread missing".to_string()))?;
        Ok(thread_to_summary(&created, true))
    }

    pub(super) async fn fork_thread_with_title_impl(
        &self,
        source_thread_or_node: Option<&str>,
        title: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        let existing_threads = self.runtime.storage.list_threads().map_err(storage_err)?;
        let (parent_node_id, parent_thread_id, parent_cwd) =
            match source_thread_or_node.map(str::trim).filter(|s| !s.is_empty()) {
                Some(raw) => {
                    if let Some(tid) = parse_typed_id::<ThreadId>(raw) {
                        if let Some(thread) =
                            self.runtime.storage.get_thread(&tid).map_err(storage_err)?
                        {
                            if thread.kind == ThreadKind::Terminal {
                                return Err(EngineError::InvalidParams(
                                    "cannot fork an agent branch from a terminal thread"
                                        .to_string(),
                                ));
                            }
                            (thread.active_leaf_id, Some(thread.id), thread.cwd)
                        } else if let Some(node) = self
                            .runtime
                            .storage
                            .get_node(&NodeId(tid.0))
                            .map_err(storage_err)?
                        {
                            (node.id, None, None)
                        } else {
                            return Err(EngineError::NotFound(format!("thread or node {raw}")));
                        }
                    } else {
                        return Err(EngineError::NotFound(format!("thread or node {raw}")));
                    }
                }
                None => {
                    if let Some(first) = existing_threads.first() {
                        if first.kind == ThreadKind::Terminal {
                            return Err(EngineError::InvalidParams(
                                "cannot fork an agent branch from a terminal thread".to_string(),
                            ));
                        }
                        (first.active_leaf_id, Some(first.id), first.cwd.clone())
                    } else if let Some(last_node) = self.runtime.lock()?.last_node {
                        (last_node, None, None)
                    } else {
                        return Err(EngineError::NotFound("no turns recorded".to_string()));
                    }
                }
            };

        let explicit_custom = normalize_custom_name(title);
        let default_title = if existing_threads.is_empty() {
            "Branch".to_string()
        } else {
            format!("Branch {}", existing_threads.len() + 1)
        };

        let created = self.runtime.insert_new_thread_with_anchor(
            parent_node_id,
            default_title,
            explicit_custom,
            parent_thread_id,
            ThreadKind::Agent,
            parent_cwd,
        )?;

        let turns: Vec<HydratedTurn> = self
            .runtime
            .load_thread_turns(&created)?
            .into_iter()
            .map(|e| HydratedTurn {
                id: e.node_id,
                role: e.role,
                content: e.content,
            })
            .collect();

        let summary = thread_to_summary(&created, false);
        self.runtime.emit_tui(EngineStreamEvent::ThreadCreated {
            thread: HydratedThread {
                id: summary.thread_id.clone(),
                title: summary.title.clone(),
                custom_name: summary.custom_name.clone(),
                agent_provider: summary.agent_provider.clone(),
                turns,
            },
        });
        let meta = self.runtime.compute_thread_metadata()?;
        self.runtime
            .emit_tui(EngineStreamEvent::ThreadMetadataUpdated { meta });
        Ok(summary)
    }

    pub(super) async fn create_root_thread_in_cwd_impl(
        &self,
        title: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        let resolved_cwd = match cwd.map(str::trim).filter(|s| !s.is_empty()) {
            Some(raw) => {
                let path = PathBuf::from(raw);
                if !path.is_dir() {
                    return Err(EngineError::InvalidParams(format!(
                        "root cwd does not exist or is not a directory: {}",
                        path.display()
                    )));
                }
                let canonical = std::fs::canonicalize(&path).map_err(|e| {
                    EngineError::InvalidParams(format!(
                        "failed to resolve root cwd {}: {e}",
                        path.display()
                    ))
                })?;
                let dir_str = canonical.to_str().map(ToOwned::to_owned).ok_or_else(|| {
                    EngineError::InvalidParams("root cwd is not valid UTF-8".to_string())
                })?;
                Some(dir_str)
            }
            None => None,
        };

        let existing_threads = self.runtime.storage.list_threads().map_err(storage_err)?;
        let explicit_custom = normalize_custom_name(title);
        let meta_before = self.runtime.compute_thread_metadata()?;
        let mut n = meta_before
            .iter()
            .filter(|m| m.parent_thread_id.is_none())
            .count()
            + 1;
        while existing_threads
            .iter()
            .any(|t| t.title == format!("Root {n}"))
        {
            n += 1;
        }
        let default_title = format!("Root {n}");

        let root_node = ConversationNode::new_root(AuthorRole::System, "");
        self.runtime
            .storage
            .insert_node(&root_node)
            .map_err(storage_err)?;

        let created = self.runtime.insert_new_thread_with_anchor(
            root_node.id,
            default_title,
            explicit_custom,
            None,
            ThreadKind::Agent,
            resolved_cwd,
        )?;
        {
            let mut state = self.runtime.lock()?;
            state.last_node = Some(root_node.id);
        }

        let summary = thread_to_summary(&created, false);
        self.runtime.emit_tui(EngineStreamEvent::ThreadCreated {
            thread: HydratedThread {
                id: summary.thread_id.clone(),
                title: summary.title.clone(),
                custom_name: summary.custom_name.clone(),
                agent_provider: summary.agent_provider.clone(),
                turns: Vec::new(),
            },
        });
        let meta = self.runtime.compute_thread_metadata()?;
        self.runtime
            .emit_tui(EngineStreamEvent::ThreadMetadataUpdated { meta });
        Ok(summary)
    }

    pub(super) async fn list_recent_user_prompts_impl(
        &self,
        limit: usize,
    ) -> Result<Vec<String>, EngineError> {
        self.runtime
            .storage
            .list_recent_user_prompts(limit)
            .map_err(storage_err)
    }

    pub(super) async fn list_workspace_files_impl(
        &self,
        root: Option<&str>,
    ) -> Result<Vec<String>, EngineError> {
        let fallback = self.runtime.cwd.clone();
        let explicit = root
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        let inspector = Arc::clone(&self.runtime.git_inspector);
        tokio::task::spawn_blocking(move || {
            let target_root = explicit.filter(|p| p.is_dir()).unwrap_or(fallback);
            inspector.list_workspace_files(&target_root, 5_000)
        })
        .await
        .map_err(|e| EngineError::Transport(format!("list_workspace_files task panicked: {e}")))
    }

    pub(super) async fn rename_thread_impl(
        &self,
        thread_id: &str,
        title: &str,
    ) -> Result<ThreadSummary, EngineError> {
        let trimmed = title.trim();
        let custom_name = (!trimmed.is_empty()).then_some(trimmed);

        let tid: ThreadId = parse_typed_id(thread_id)
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        self.runtime
            .storage
            .update_thread_custom_name(&tid, custom_name)
            .map_err(|e| match e {
                CoreError::ThreadNotFound(_) => EngineError::NotFound(format!("thread {thread_id}")),
                other => storage_err(other),
            })?;

        let updated = self
            .runtime
            .storage
            .get_thread(&tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        let threads = self.runtime.storage.list_threads().map_err(storage_err)?;
        let is_first = threads.first().is_some_and(|t| t.id == updated.id);
        let summary = thread_to_summary(&updated, is_first);

        self.runtime
            .emit_tui(EngineStreamEvent::ThreadCustomNameUpdated {
                thread_id: summary.thread_id.clone(),
                custom_name: summary.custom_name.clone(),
            });
        Ok(summary)
    }

    pub(super) async fn list_threads_impl(&self) -> Result<Vec<ThreadSummary>, EngineError> {
        let threads = self.runtime.storage.list_threads().map_err(storage_err)?;
        Ok(threads
            .iter()
            .enumerate()
            .map(|(i, t)| thread_to_summary(t, i == 0))
            .collect())
    }

    pub(super) async fn get_transcript_impl(&self) -> Result<Vec<TranscriptEntry>, EngineError> {
        let threads = self.runtime.storage.list_threads().map_err(storage_err)?;
        if let Some(first) = threads.first() {
            return self.runtime.load_thread_turns(first);
        }
        let nodes = self.runtime.storage.list_nodes().map_err(storage_err)?;
        Ok(nodes
            .into_iter()
            .filter(|n| !is_internal_root_node(n))
            .map(|n| node_to_transcript_entry(&n))
            .collect())
    }

    pub(super) async fn get_thread_transcript_impl(
        &self,
        thread_id: &str,
    ) -> Result<Vec<TranscriptEntry>, EngineError> {
        let tid: ThreadId = parse_typed_id(thread_id)
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        let thread = self
            .runtime
            .storage
            .get_thread(&tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        self.runtime.load_thread_turns(&thread)
    }

    pub(super) async fn merge_thread_with_summary_impl(
        &self,
        source_id: &str,
        target_id: Option<&str>,
        summary: Option<&str>,
    ) -> Result<ThreadSummary, EngineError> {
        let source_tid: ThreadId = parse_typed_id(source_id)
            .ok_or_else(|| EngineError::NotFound(format!("thread {source_id}")))?;
        let source = self
            .runtime
            .storage
            .get_thread(&source_tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {source_id}")))?;
        if source.kind == ThreadKind::Terminal {
            return Err(EngineError::InvalidParams(
                "cannot merge a terminal thread into an agent context".to_string(),
            ));
        }

        let explicit_target = match target_id.map(str::trim).filter(|s| !s.is_empty()) {
            Some(raw) => match parse_typed_id::<ThreadId>(raw) {
                Some(tid) if tid != source_tid => {
                    self.runtime.storage.get_thread(&tid).map_err(storage_err)?
                }
                _ => None,
            },
            None => None,
        };

        let target_thread = match explicit_target {
            Some(t) => Some(t),
            None => {
                let meta = self.runtime.compute_thread_metadata()?;
                let parent_tid = meta
                    .into_iter()
                    .find(|m| m.thread_id == source.id.0.to_string())
                    .and_then(|m| m.parent_thread_id)
                    .and_then(|pid| parse_typed_id::<ThreadId>(&pid))
                    .filter(|tid| *tid != source_tid);
                match parent_tid {
                    Some(pid) => self.runtime.storage.get_thread(&pid).map_err(storage_err)?,
                    None => None,
                }
            }
        };

        let Some(target) = target_thread.filter(|t| t.id != source_tid) else {
            return Err(EngineError::InvalidParams(
                "cannot join a root context without a parent branch".to_string(),
            ));
        };
        if target.kind == ThreadKind::Terminal {
            return Err(EngineError::InvalidParams(
                "cannot merge into a terminal thread".to_string(),
            ));
        }

        let source_threads = self.runtime.storage.list_threads().map_err(storage_err)?;
        let source_is_first = source_threads.first().is_some_and(|t| t.id == source.id);
        let source_summary = thread_to_summary(&source, source_is_first);
        let source_title = source_summary
            .custom_name
            .clone()
            .unwrap_or_else(|| source_summary.title.clone());
        let resolved_summary = match summary.map(str::trim).filter(|s| !s.is_empty()) {
            Some(explicit) => truncate_chars_with_ellipsis(explicit, 400),
            None => {
                let turns = self.runtime.load_thread_turns(&source)?;
                let raw_digest = turns
                    .iter()
                    .rev()
                    .find(|t| t.role == "assistant" && !t.content.trim().is_empty())
                    .or_else(|| {
                        turns
                            .iter()
                            .rev()
                            .find(|t| t.role == "user" && !t.content.trim().is_empty())
                    })
                    .or_else(|| turns.iter().rev().find(|t| !t.content.trim().is_empty()))
                    .map(|t| t.content.as_str())
                    .unwrap_or(source_title.as_str());
                let normalized = raw_digest.split_whitespace().collect::<Vec<_>>().join(" ");
                truncate_chars_with_ellipsis(&normalized, 400)
            }
        };

        let target_id_str = target.id.0.to_string();
        let target_cmd_tx = {
            let state = self.runtime.lock()?;
            state
                .active_acp_sessions
                .get(&target_id_str)
                .map(|s| s.cmd_tx.clone())
        };
        if let Some(cmd_tx) = target_cmd_tx {
            let (reply_tx, reply_rx) = oneshot::channel();
            if cmd_tx
                .send(SessionListenerCmd::PrepareSolicited { reply_tx })
                .is_ok()
            {
                let _ = reply_rx.await;
            }
        }
        let fresh_target = self
            .runtime
            .storage
            .get_thread(&target.id)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {target_id_str}")))?;
        let landmark_node = synthesize_thread_merge(
            source_tid,
            &source_title,
            &resolved_summary,
            Some(fresh_target.active_leaf_id),
        )
        .map_err(storage_err)?;
        self.runtime
            .storage
            .insert_node(&landmark_node)
            .map_err(storage_err)?;
        self.runtime
            .storage
            .update_thread_leaf(&fresh_target.id, &landmark_node.id)
            .map_err(storage_err)?;
        {
            let mut state = self.runtime.lock()?;
            state.last_node = Some(landmark_node.id);
            if let Some(entry) = state.active_acp_sessions.get_mut(&target_id_str) {
                entry.pending_landmarks.push(landmark_node.content.clone());
            }
        }

        self.runtime
            .storage
            .mark_thread_merged(&source_tid, true)
            .map_err(storage_err)?;
        let updated = self
            .runtime
            .storage
            .get_thread(&source_tid)
            .map_err(storage_err)?
            .ok_or_else(|| EngineError::NotFound(format!("thread {source_id}")))?;

        self.runtime.emit_tui(EngineStreamEvent::ThreadMerged {
            thread_id: updated.id.0.to_string(),
            target_thread_id: target_id_str,
            summary: resolved_summary,
        });
        let meta = self.runtime.compute_thread_metadata()?;
        self.runtime
            .emit_tui(EngineStreamEvent::ThreadMetadataUpdated { meta });

        Ok(thread_to_summary(&updated, source_is_first))
    }

    pub(super) async fn delete_thread_impl(&self, thread_id: &str) -> Result<String, EngineError> {
        let tid: ThreadId = parse_typed_id(thread_id)
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        let threads = self.runtime.storage.list_threads().map_err(storage_err)?;
        let deleted_thread = threads
            .iter()
            .find(|t| t.id == tid)
            .cloned()
            .ok_or_else(|| EngineError::NotFound(format!("thread {thread_id}")))?;
        if threads.len() <= 1 {
            return Err(EngineError::InvalidParams(
                "cannot delete the only thread".to_string(),
            ));
        }

        let meta_before = self.runtime.compute_thread_metadata()?;
        let parent_thread_id = meta_before
            .into_iter()
            .find(|m| m.thread_id == thread_id)
            .and_then(|m| m.parent_thread_id);

        let removed_pty = self
            .runtime
            .active_pty_sessions
            .lock()
            .ok()
            .and_then(|mut map| map.remove(thread_id));
        if let Ok(mut pending) = self.runtime.pending_pty_sizes.lock() {
            pending.remove(thread_id);
        }
        if let Ok(mut exited) = self.runtime.pty_exited_threads.lock() {
            exited.remove(thread_id);
        }
        if let Some(pty_session) = removed_pty {
            let _ = tokio::task::spawn_blocking(move || drop(pty_session)).await;
        }

        let removed_session = {
            let mut state = self.runtime.lock()?;
            state.pending_thread_config.remove(thread_id);
            state.thread_turn_locks.remove(thread_id);
            state
                .active_acp_sessions
                .remove(thread_id)
                .map(|s| (s.session_id.clone(), s.driver_gen))
        };
        self.runtime.subagent_observer.clear_thread(thread_id);

        if let Some((session_id, driver_gen)) = removed_session {
            let driver_opt = {
                let guard = self.runtime.acp_driver.lock().await;
                guard
                    .as_ref()
                    .filter(|s| s.generation == driver_gen && s.driver.is_running())
                    .map(|s| Arc::clone(&s.driver))
            };
            if let Some(driver) = driver_opt {
                let _ = tokio::time::timeout(
                    CLOSE_SESSION_TIMEOUT,
                    driver.close_session(&session_id),
                )
                .await;
            }
        }

        let reparent_target = deleted_thread
            .parent_thread_id
            .as_ref()
            .filter(|pid| **pid != tid && threads.iter().any(|t| t.id == **pid));
        self.runtime
            .storage
            .reparent_child_threads(&tid, reparent_target)
            .map_err(storage_err)?;
        self.runtime
            .storage
            .delete_thread(&tid)
            .map_err(storage_err)?;
        let remaining = self.runtime.storage.list_threads().map_err(storage_err)?;
        if let Ok(mut state) = self.runtime.lock() {
            state.last_node = remaining.first().map(|t| t.active_leaf_id);
        }

        let next_active_thread_id = parent_thread_id
            .filter(|pid| remaining.iter().any(|t| t.id.0.to_string() == *pid))
            .or_else(|| remaining.first().map(|t| t.id.0.to_string()))
            .unwrap_or_default();

        let meta_after = self.runtime.compute_thread_metadata()?;
        self.runtime.emit_tui(EngineStreamEvent::ThreadDeleted {
            thread_id: thread_id.to_string(),
            next_active_thread_id: next_active_thread_id.clone(),
        });
        self.runtime
            .emit_tui(EngineStreamEvent::ThreadMetadataUpdated { meta: meta_after });
        Ok(next_active_thread_id)
    }
}
