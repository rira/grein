use std::io;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::args::{AgentsArgs, StatusArgs, ThreadsArgs};
use crate::daemon::{default_db_path, default_socket_path, resolve_state_dir};
use crate::discovery::{resolve_agents_configuration, AcpAgentKind};
use crate::engine::{DaemonEngine, InProcessEngine};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadOutput {
    pub thread_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_name: Option<String>,
    pub parent_node_id: String,
    pub active_leaf_id: String,
    pub merged: bool,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatusOutput {
    pub workspace_mode: String,
    pub state_dir: PathBuf,
    pub db_path: PathBuf,
    pub socket_path: PathBuf,
    pub socket_reachable: bool,
    pub thread_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentOutput {
    pub kind: String,
    pub executable_path: PathBuf,
}

pub async fn collect_threads_output(
    args: &ThreadsArgs,
) -> Result<Vec<ThreadOutput>, Box<dyn std::error::Error + Send + Sync>> {
    let db_path = args.db.clone().unwrap_or_else(default_db_path);
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let engine = InProcessEngine::open_with_config(&db_path, cwd, None)
        .map_err(|e| io::Error::other(format!("{e:?}")))?;
    let summaries = engine
        .list_threads()
        .await
        .map_err(|e| io::Error::other(format!("{e:?}")))?;
    Ok(summaries
        .into_iter()
        .map(|s| ThreadOutput {
            thread_id: s.thread_id,
            title: s.title,
            custom_name: s.custom_name,
            parent_node_id: s.parent_node_id,
            active_leaf_id: s.active_leaf_id,
            merged: s.merged,
            status: serde_json::to_string(&s.status)
                .map(|raw| raw.trim_matches('"').to_string())
                .unwrap_or_else(|_| "idle".to_string()),
        })
        .collect())
}

pub async fn handle_threads(
    args: &ThreadsArgs,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let threads = collect_threads_output(args).await?;

    if args.json {
        let json = serde_json::to_string_pretty(&threads)?;
        println!("{json}");
    } else if threads.is_empty() {
        println!("No threads found.");
    } else {
        println!(
            "{:<36} {:<20} {:<36} {:<10} {:<6}",
            "THREAD ID", "TITLE", "PARENT NODE ID", "STATUS", "MERGED"
        );
        for t in threads {
            let display_title = t.custom_name.as_deref().unwrap_or(&t.title);
            println!(
                "{:<36} {:<20} {:<36} {:<10} {:<6}",
                t.thread_id, display_title, t.parent_node_id, t.status, t.merged
            );
        }
    }

    Ok(())
}

pub async fn collect_status_output(args: &StatusArgs) -> StatusOutput {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let state_dir = resolve_state_dir(&cwd);
    let db_path = args.db.clone().unwrap_or_else(default_db_path);
    let socket_path = args.socket.clone().unwrap_or_else(default_socket_path);

    let workspace_mode = {
        let env = grein_workspace::environment::detect_environment(&cwd);
        format!("{env:?}")
    };

    let socket_reachable = if socket_path.exists() {
        UnixStream::connect(&socket_path).is_ok()
    } else {
        false
    };

    let thread_count = if db_path.exists() {
        if let Ok(engine) = InProcessEngine::open_with_config(&db_path, cwd, None) {
            engine.list_threads().await.map(|t| t.len()).unwrap_or(0)
        } else {
            0
        }
    } else {
        0
    };

    StatusOutput {
        workspace_mode,
        state_dir,
        db_path,
        socket_path,
        socket_reachable,
        thread_count,
    }
}

pub async fn handle_status(
    args: &StatusArgs,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let status = collect_status_output(args).await;

    if args.json {
        let json = serde_json::to_string_pretty(&status)?;
        println!("{json}");
    } else {
        println!("Workspace Mode:    {}", status.workspace_mode);
        println!("State Directory:   {}", status.state_dir.display());
        println!("Database Path:     {}", status.db_path.display());
        println!("Socket Path:       {}", status.socket_path.display());
        println!("Socket Reachable:  {}", status.socket_reachable);
        println!("Thread Count:      {}", status.thread_count);
    }

    Ok(())
}

pub async fn handle_agents(
    args: &AgentsArgs,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let resolved = resolve_agents_configuration(args.config.as_deref(), None)
        .map_err(|e| io::Error::other(format!("{e:?}")))?;

    let agents: Vec<AgentOutput> = resolved
        .into_iter()
        .map(|a| AgentOutput {
            kind: match a.kind {
                AcpAgentKind::Agy => "agy".to_string(),
                AcpAgentKind::Claude => "claude".to_string(),
                AcpAgentKind::Custom => "custom".to_string(),
            },
            executable_path: a.executable_path,
        })
        .collect();

    if args.json {
        let json = serde_json::to_string_pretty(&agents)?;
        println!("{json}");
    } else if agents.is_empty() {
        println!("No ACP agents found.");
    } else {
        println!("{:<12} {:<}", "KIND", "EXECUTABLE");
        for a in agents {
            println!("{:<12} {:<}", a.kind, a.executable_path.display());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_headless_threads_and_status_reflect_hydrated_sqlite_state() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("session.db");
        let socket_path = temp.path().join("daemon.sock");

        let engine =
            InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
                .expect("open");
        let main = engine.ensure_main_thread().await.expect("main");
        engine
            .fork_thread_with_title(Some(&main.thread_id), Some("topic-a"))
            .await
            .expect("fork");
        drop(engine);

        let threads_out = collect_threads_output(&ThreadsArgs {
            json: true,
            db: Some(db_path.clone()),
        })
        .await
        .expect("collect_threads_output");
        assert_eq!(threads_out.len(), 2);
        assert_eq!(threads_out[0].thread_id, main.thread_id);
        assert_eq!(threads_out[0].title, "Main");
        assert_eq!(threads_out[0].custom_name, None);
        assert_eq!(threads_out[0].status, "idle");
        assert!(!threads_out[0].merged);
        assert_eq!(threads_out[1].custom_name.as_deref(), Some("topic-a"));
        assert_eq!(threads_out[1].title, "Branch 2");

        let status_out = collect_status_output(&StatusArgs {
            json: true,
            socket: Some(socket_path.clone()),
            db: Some(db_path.clone()),
        })
        .await;
        assert_eq!(status_out.thread_count, 2);

        handle_threads(&ThreadsArgs {
            json: true,
            db: Some(db_path.clone()),
        })
        .await
        .expect("handle_threads");

        handle_status(&StatusArgs {
            json: true,
            socket: Some(socket_path.clone()),
            db: Some(db_path.clone()),
        })
        .await
        .expect("handle_status");
    }
}
