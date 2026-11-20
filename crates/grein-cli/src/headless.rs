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
    pub parent_node_id: String,
    pub merged: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatusOutput {
    pub workspace_mode: String,
    pub state_dir: PathBuf,
    pub db_path: PathBuf,
    pub socket_path: PathBuf,
    pub socket_reachable: bool,
    pub thread_count: usize,
    pub blackboard_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentOutput {
    pub kind: String,
    pub executable_path: PathBuf,
}

pub async fn handle_threads(
    args: &ThreadsArgs,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let db_path = args.db.clone().unwrap_or_else(default_db_path);
    let threads: Vec<ThreadOutput> = if db_path.exists() {
        let engine =
            InProcessEngine::open(&db_path).map_err(|e| io::Error::other(format!("{e:?}")))?;
        let summaries = engine
            .list_threads()
            .await
            .map_err(|e| io::Error::other(format!("{e:?}")))?;
        summaries
            .into_iter()
            .map(|s| ThreadOutput {
                thread_id: s.thread_id,
                parent_node_id: s.parent_node_id,
                merged: s.merged,
            })
            .collect()
    } else {
        Vec::new()
    };

    if args.json {
        let json = serde_json::to_string_pretty(&threads)?;
        println!("{json}");
    } else if threads.is_empty() {
        println!("No threads found.");
    } else {
        println!(
            "{:<36} {:<36} {:<6}",
            "THREAD ID", "PARENT NODE ID", "MERGED"
        );
        for t in threads {
            println!(
                "{:<36} {:<36} {:<6}",
                t.thread_id, t.parent_node_id, t.merged
            );
        }
    }

    Ok(())
}

pub async fn handle_status(
    args: &StatusArgs,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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

    let (thread_count, blackboard_count) = if db_path.exists() {
        if let Ok(engine) = InProcessEngine::open(&db_path) {
            let tc = engine.list_threads().await.map(|t| t.len()).unwrap_or(0);
            let bc = engine.blackboard_list().await.map(|b| b.len()).unwrap_or(0);
            (tc, bc)
        } else {
            (0, 0)
        }
    } else {
        (0, 0)
    };

    let status = StatusOutput {
        workspace_mode,
        state_dir,
        db_path,
        socket_path,
        socket_reachable,
        thread_count,
        blackboard_count,
    };

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
        println!("Blackboard Count:  {}", status.blackboard_count);
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
