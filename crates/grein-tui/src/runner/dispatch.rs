use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::app::{normalize_custom_name, App, AppAction};
use crate::engine_port::{
    AgentOptionsSnapshot, PtyScreenSnapshot, ThreadMeta, TuiEngineEvent, TuiEnginePort,
    WorktreeEntry,
};

use super::engine_events::apply_engine_event;

pub(super) enum RunnerLocalEvent {
    Engine(TuiEngineEvent),
    OptimisticThreadMeta(ThreadMeta),
    StatusNotice(String),
    OpenConfigPicker {
        snapshot: AgentOptionsSnapshot,
        config_id: String,
    },
    ResolveConfigQuery {
        snapshot: AgentOptionsSnapshot,
        config_id: String,
        query: String,
        set_default: bool,
    },
    ConfigApplied {
        snapshot: AgentOptionsSnapshot,
        config_id: String,
        value: String,
        set_default: bool,
    },
    CommandsFetched {
        snapshot: AgentOptionsSnapshot,
    },
    MergeFallback {
        thread_id: String,
        target_thread_id: String,
        summary: Option<String>,
    },
    MergeAborted(String),
    PtySnapshotLoaded {
        thread_id: String,
        snapshot: PtyScreenSnapshot,
    },
    WorktreeReady {
        thread_id: String,
        path: String,
    },
    WorktreesListed(Vec<WorktreeEntry>),
    WorktreesListFailed(String),
    WorktreeSwitched {
        thread_id: String,
        path: String,
    },
    WorktreeRemoved {
        path: String,
    },
    WorktreeDiffLoaded {
        path: String,
        diff: String,
    },
    WorktreeDiffFailed {
        path: String,
        err: String,
    },
    WorktreesPruned(String),
}

async fn refresh_worktrees_and_metadata(
    engine: &dyn TuiEnginePort,
    local_ev_tx: &mpsc::UnboundedSender<RunnerLocalEvent>,
) {
    match engine.list_worktrees().await {
        Ok(entries) => {
            let _ = local_ev_tx.send(RunnerLocalEvent::WorktreesListed(entries));
        }
        Err(err) => {
            let _ = local_ev_tx.send(RunnerLocalEvent::WorktreesListFailed(err.to_string()));
        }
    }
    poll_metadata_to_local(engine, local_ev_tx).await;
}

async fn poll_metadata_to_local(
    engine: &dyn TuiEnginePort,
    local_ev_tx: &mpsc::UnboundedSender<RunnerLocalEvent>,
) {
    match engine.thread_metadata().await {
        Ok(meta) => {
            if !meta.is_empty() {
                let _ = local_ev_tx.send(RunnerLocalEvent::Engine(
                    TuiEngineEvent::ThreadMetadataUpdated { meta },
                ));
            }
        }
        Err(err) => {
            let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                "Metadata sync failed: {err}"
            )));
        }
    }
}

fn dispatch_action(
    engine: Arc<dyn TuiEnginePort>,
    action: AppAction,
    local_ev_tx: mpsc::UnboundedSender<RunnerLocalEvent>,
    err_tx: mpsc::UnboundedSender<(Option<String>, String)>,
    pty_write_tx: &mpsc::UnboundedSender<(String, Vec<u8>)>,
) {
    match action {
        AppAction::SendPrompt { thread_id, prompt } => {
            tokio::spawn(async move {
                if let Err(err) = engine.send_turn(&thread_id, &prompt).await {
                    let _ = err_tx.send((Some(thread_id), err.to_string()));
                }
            });
        }
        AppAction::RunShell { thread_id, command } => {
            tokio::spawn(async move {
                if let Err(err) = engine.run_shell_command(&thread_id, &command).await {
                    let _ = err_tx.send((Some(thread_id), err.to_string()));
                }
            });
        }
        AppAction::ForkBranch {
            source_thread_id,
            title,
        } => {
            tokio::spawn(async move {
                match engine.fork_branch(&source_thread_id, title.as_deref()).await {
                    Ok(thread) => {
                        let optimistic = ThreadMeta {
                            thread_id: thread.id.clone(),
                            parent_thread_id: Some(source_thread_id),
                            inherited_turn_count: 0,
                            merged: false,
                            kind: grein_core::ThreadKind::Agent,
                            cwd: None,
                            git_branch: None,
                            git_dirty: 0,
                            custom_name: None,
                        };
                        let _ = local_ev_tx.send(RunnerLocalEvent::Engine(
                            TuiEngineEvent::ThreadCreated { thread },
                        ));
                        let _ =
                            local_ev_tx.send(RunnerLocalEvent::OptimisticThreadMeta(optimistic));
                        poll_metadata_to_local(engine.as_ref(), &local_ev_tx).await;
                    }
                    Err(err) => {
                        let _ = err_tx.send((Some(source_thread_id), err.to_string()));
                    }
                }
            });
        }
        AppAction::CreateRoot { title } => {
            tokio::spawn(async move {
                match engine.create_root(title.as_deref()).await {
                    Ok(thread) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::Engine(
                            TuiEngineEvent::ThreadCreated { thread },
                        ));
                        poll_metadata_to_local(engine.as_ref(), &local_ev_tx).await;
                    }
                    Err(err) => {
                        let _ = err_tx.send((None, err.to_string()));
                    }
                }
            });
        }
        AppAction::RenameThread { thread_id, title } => {
            tokio::spawn(async move {
                match engine.rename_thread(&thread_id, &title).await {
                    Ok(()) => {
                        let custom_name = normalize_custom_name(Some(&title));
                        let _ = local_ev_tx.send(RunnerLocalEvent::Engine(
                            TuiEngineEvent::ThreadCustomNameUpdated {
                                thread_id,
                                custom_name,
                            },
                        ));
                    }
                    Err(err) => {
                        let _ = err_tx.send((Some(thread_id), err.to_string()));
                    }
                }
            });
        }
        AppAction::MergeBranch {
            source_thread_id,
            target_thread_id,
            summary,
        } => {
            tokio::spawn(async move {
                match engine
                    .merge_thread_with_summary(
                        &source_thread_id,
                        &target_thread_id,
                        summary.as_deref(),
                    )
                    .await
                {
                    Ok(()) => {
                        tokio::task::yield_now().await;
                        let _ = local_ev_tx.send(RunnerLocalEvent::MergeFallback {
                            thread_id: source_thread_id,
                            target_thread_id,
                            summary,
                        });
                        poll_metadata_to_local(engine.as_ref(), &local_ev_tx).await;
                    }
                    Err(err) => {
                        let _ = local_ev_tx
                            .send(RunnerLocalEvent::MergeAborted(source_thread_id.clone()));
                        let _ = err_tx.send((Some(source_thread_id), err.to_string()));
                    }
                }
            });
        }
        AppAction::DeleteThread(thread_id) => {
            tokio::spawn(async move {
                match engine.delete_thread(&thread_id).await {
                    Ok(next_active_thread_id) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::Engine(
                            TuiEngineEvent::ThreadDeleted {
                                thread_id,
                                next_active_thread_id,
                            },
                        ));
                        poll_metadata_to_local(engine.as_ref(), &local_ev_tx).await;
                    }
                    Err(err) => {
                        let _ = err_tx.send((Some(thread_id), err));
                    }
                }
            });
        }
        AppAction::CancelTurn { thread_id } => {
            tokio::spawn(async move {
                if let Err(err) = engine.cancel_turn(&thread_id).await {
                    let _ = err_tx.send((Some(thread_id), err.to_string()));
                }
            });
        }
        AppAction::ResolvePermission {
            request_id,
            approved,
        } => {
            tokio::spawn(async move {
                if let Err(err) = engine.resolve_permission(&request_id, approved).await {
                    let _ = err_tx.send((None, err.to_string()));
                }
            });
        }
        AppAction::OpenConfigPicker {
            thread_id,
            config_id,
        } => {
            tokio::spawn(async move {
                match engine.agent_options(&thread_id).await {
                    Ok(snapshot) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::OpenConfigPicker {
                            snapshot,
                            config_id,
                        });
                    }
                    Err(err) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                            "Failed to load options: {err}"
                        )));
                    }
                }
            });
        }
        AppAction::SetAgentConfig {
            thread_id,
            config_id,
            value,
            set_default,
        } => {
            tokio::spawn(async move {
                match engine
                    .set_agent_config(&thread_id, &config_id, &value, set_default)
                    .await
                {
                    Ok(snapshot) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::ConfigApplied {
                            snapshot,
                            config_id,
                            value,
                            set_default,
                        });
                    }
                    Err(err) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                            "Config update failed: {err}"
                        )));
                    }
                }
            });
        }
        AppAction::ApplyAgentConfigQuery {
            thread_id,
            config_id,
            query,
            set_default,
        } => {
            tokio::spawn(async move {
                match engine.agent_options(&thread_id).await {
                    Ok(snapshot) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::ResolveConfigQuery {
                            snapshot,
                            config_id,
                            query,
                            set_default,
                        });
                    }
                    Err(err) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                            "Failed to load options: {err}"
                        )));
                    }
                }
            });
        }
        AppAction::FetchAgentCommands { thread_id } => {
            tokio::spawn(async move {
                match engine.agent_options(&thread_id).await {
                    Ok(snapshot) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::CommandsFetched { snapshot });
                    }
                    Err(err) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                            "Failed to load commands: {err}"
                        )));
                    }
                }
            });
        }
        AppAction::CreateTerminal {
            parent_thread_id,
            title,
            cwd,
            rows,
            cols,
        } => {
            tokio::spawn(async move {
                match engine
                    .create_terminal(
                        parent_thread_id.as_deref(),
                        title.as_deref(),
                        cwd.as_deref(),
                        rows,
                        cols,
                    )
                    .await
                {
                    Ok(thread) => {
                        let optimistic = ThreadMeta {
                            thread_id: thread.id.clone(),
                            parent_thread_id,
                            inherited_turn_count: 0,
                            merged: false,
                            kind: grein_core::ThreadKind::Terminal,
                            cwd,
                            git_branch: None,
                            git_dirty: 0,
                            custom_name: None,
                        };
                        let _ =
                            local_ev_tx.send(RunnerLocalEvent::OptimisticThreadMeta(optimistic));
                        let _ = local_ev_tx.send(RunnerLocalEvent::Engine(
                            TuiEngineEvent::ThreadCreated { thread },
                        ));
                        poll_metadata_to_local(engine.as_ref(), &local_ev_tx).await;
                    }
                    Err(err) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                            "Create terminal failed: {err}"
                        )));
                    }
                }
            });
        }
        AppAction::PtyWrite { thread_id, bytes } => {
            let _ = pty_write_tx.send((thread_id, bytes));
        }
        AppAction::PtyResize {
            thread_id,
            rows,
            cols,
        } => {
            tokio::spawn(async move {
                if let Err(err) = engine.pty_resize(&thread_id, rows, cols).await {
                    let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                        "Terminal resize failed: {err}"
                    )));
                }
            });
        }
        AppAction::PtySnapshot {
            thread_id,
            rows,
            cols,
        } => {
            tokio::spawn(async move {
                match engine.pty_snapshot(&thread_id, rows, cols).await {
                    Ok(snapshot) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::PtySnapshotLoaded {
                            thread_id,
                            snapshot,
                        });
                    }
                    Err(err) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                            "Terminal snapshot failed: {err}"
                        )));
                    }
                }
            });
        }
        AppAction::PtyScroll { thread_id, delta } => {
            tokio::spawn(async move {
                match engine.pty_scroll(&thread_id, delta).await {
                    Ok(snapshot) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::PtySnapshotLoaded {
                            thread_id,
                            snapshot,
                        });
                    }
                    Err(err) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                            "Terminal scroll failed: {err}"
                        )));
                    }
                }
            });
        }
        AppAction::PtyRestart {
            thread_id,
            rows,
            cols,
        } => {
            tokio::spawn(async move {
                match engine.pty_restart(&thread_id, rows, cols).await {
                    Ok(snapshot) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::PtySnapshotLoaded {
                            thread_id,
                            snapshot,
                        });
                    }
                    Err(err) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                            "Terminal restart failed: {err}"
                        )));
                    }
                }
            });
        }
        AppAction::EnsureThreadWorktree { thread_id } => {
            tokio::spawn(async move {
                match engine.ensure_thread_worktree(&thread_id).await {
                    Ok(path) => {
                        let _ = local_ev_tx
                            .send(RunnerLocalEvent::WorktreeReady { thread_id, path });
                        refresh_worktrees_and_metadata(engine.as_ref(), &local_ev_tx).await;
                    }
                    Err(err) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                            "Worktree failed: {err}"
                        )));
                    }
                }
            });
        }
        AppAction::ListWorktrees => {
            tokio::spawn(async move {
                match engine.list_worktrees().await {
                    Ok(entries) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::WorktreesListed(entries));
                    }
                    Err(err) => {
                        let _ = local_ev_tx
                            .send(RunnerLocalEvent::WorktreesListFailed(err.to_string()));
                    }
                }
            });
        }
        AppAction::SwitchThreadWorktree {
            thread_id,
            target_path,
        } => {
            tokio::spawn(async move {
                match engine
                    .switch_thread_worktree(&thread_id, &target_path)
                    .await
                {
                    Ok(path) => {
                        let _ = local_ev_tx
                            .send(RunnerLocalEvent::WorktreeSwitched { thread_id, path });
                        refresh_worktrees_and_metadata(engine.as_ref(), &local_ev_tx).await;
                    }
                    Err(err) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                            "Worktree switch failed: {err}"
                        )));
                    }
                }
            });
        }
        AppAction::RemoveWorktree { path, force } => {
            tokio::spawn(async move {
                match engine.remove_worktree(&path, force).await {
                    Ok(()) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::WorktreeRemoved { path });
                        refresh_worktrees_and_metadata(engine.as_ref(), &local_ev_tx).await;
                    }
                    Err(err) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                            "Delete worktree failed: {err}"
                        )));
                    }
                }
            });
        }
        AppAction::FetchWorktreeDiff { path } => {
            tokio::spawn(async move {
                match engine.worktree_diff_preview(&path).await {
                    Ok(diff) => {
                        let _ = local_ev_tx
                            .send(RunnerLocalEvent::WorktreeDiffLoaded { path, diff });
                    }
                    Err(err) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::WorktreeDiffFailed {
                            path,
                            err: err.to_string(),
                        });
                    }
                }
            });
        }
        AppAction::PruneWorktrees => {
            tokio::spawn(async move {
                match engine.prune_worktrees().await {
                    Ok(summary) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::WorktreesPruned(summary));
                        refresh_worktrees_and_metadata(engine.as_ref(), &local_ev_tx).await;
                    }
                    Err(err) => {
                        let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                            "Prune worktrees failed: {err}"
                        )));
                    }
                }
            });
        }
        AppAction::SaveThemePreference { theme } => {
            tokio::spawn(async move {
                if let Err(err) = engine.save_theme_preference(&theme).await {
                    let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                        "Save theme failed: {err}"
                    )));
                }
            });
        }
        AppAction::SaveKeybindingPreference { action, binding } => {
            tokio::spawn(async move {
                if let Err(err) = engine.save_keybinding_preference(&action, &binding).await {
                    let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                        "Save keybinding failed: {err}"
                    )));
                }
            });
        }
        AppAction::RemoveKeybindingPreference { action } => {
            tokio::spawn(async move {
                if let Err(err) = engine.remove_keybinding_preference(&action).await {
                    let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                        "Remove keybinding failed: {err}"
                    )));
                }
            });
        }
        AppAction::SaveUiPreference { key, value } => {
            tokio::spawn(async move {
                if let Err(err) = engine.save_ui_preference(&key, &value).await {
                    let _ = local_ev_tx.send(RunnerLocalEvent::StatusNotice(format!(
                        "Save UI preference failed: {err}"
                    )));
                }
            });
        }
        AppAction::Scrolled { .. } | AppAction::None => {}
    }
}

pub(super) fn drain_pending_actions(
    app: &mut App,
    engine: &Arc<dyn TuiEnginePort>,
    local_ev_tx: &mpsc::UnboundedSender<RunnerLocalEvent>,
    err_tx: &mpsc::UnboundedSender<(Option<String>, String)>,
    pty_write_tx: &mpsc::UnboundedSender<(String, Vec<u8>)>,
    pending_local_merges: &mut HashMap<String, usize>,
) {
    while let Some(action) = app.pending_actions.pop_front() {
        if let AppAction::MergeBranch {
            ref source_thread_id,
            ..
        } = action
        {
            *pending_local_merges
                .entry(source_thread_id.clone())
                .or_default() += 1;
        }
        dispatch_action(
            Arc::clone(engine),
            action,
            local_ev_tx.clone(),
            err_tx.clone(),
            pty_write_tx,
        );
    }
}

pub(super) fn cache_snapshot_options(app: &mut App, snapshot: &AgentOptionsSnapshot) {
    if !snapshot.config_options.is_empty() {
        apply_engine_event(
            app,
            TuiEngineEvent::AgentConfigUpdated {
                thread_id: snapshot.thread_id.clone(),
                config_options: snapshot.config_options.clone(),
            },
        );
    }
    if !snapshot.available_commands.is_empty() {
        apply_engine_event(
            app,
            TuiEngineEvent::AgentCommandsUpdated {
                thread_id: snapshot.thread_id.clone(),
                commands: snapshot.available_commands.clone(),
            },
        );
    }
}

pub(super) fn handle_streamed_engine_event(
    app: &mut App,
    event: TuiEngineEvent,
    pending_local_merges: &mut HashMap<String, usize>,
) {
    if let TuiEngineEvent::ThreadMerged { ref thread_id, .. } = event {
        if let Some(cnt) = pending_local_merges.get_mut(thread_id) {
            if *cnt > 0 {
                *cnt -= 1;
            }
        }
    }
    apply_engine_event(app, event);
}
