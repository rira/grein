use serde::Serialize;
use serde_json::{json, Value};

use super::types::{DaemonEngine, EngineError, PermissionDecisionPayload};

pub(super) fn to_json<T: Serialize>(value: &T) -> Result<Value, EngineError> {
    serde_json::to_value(value).map_err(|e| EngineError::Protocol(e.to_string()))
}

fn required_str<'a>(params: &'a Value, key: &str) -> Result<&'a str, EngineError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| EngineError::InvalidParams(format!("missing string param `{key}`")))
}

pub(super) fn rpc_error(id: Value, code: i64, message: String) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

async fn route(
    engine: &dyn DaemonEngine,
    method: &str,
    params: &Value,
) -> Result<Option<Value>, EngineError> {
    let value = match method {
        "session.ping" => {
            engine.ping().await?;
            json!({"pong": true})
        }
        "session.ensure_main_thread" => to_json(&engine.ensure_main_thread().await?)?,
        "session.send_turn" => {
            let prompt = required_str(params, "prompt")?;
            if let Some(tid) = params.get("thread_id").and_then(Value::as_str) {
                to_json(&engine.send_turn_for_thread(tid, prompt).await?)?
            } else {
                to_json(&engine.send_turn(prompt).await?)?
            }
        }
        "session.send_thread_turn" => {
            let thread_id = required_str(params, "thread_id")?;
            let prompt = required_str(params, "prompt")?;
            to_json(&engine.send_turn_for_thread(thread_id, prompt).await?)?
        }
        "session.fork_thread" => {
            let source = params
                .get("parent_node_id")
                .or_else(|| params.get("source_thread_id"))
                .and_then(Value::as_str);
            let title = params.get("title").and_then(Value::as_str);
            to_json(&engine.fork_thread_with_title(source, title).await?)?
        }
        "session.create_root_thread" | "session.create_root" => {
            let title = params.get("title").and_then(Value::as_str);
            let cwd = params.get("cwd").and_then(Value::as_str);
            to_json(&engine.create_root_thread_in_cwd(title, cwd).await?)?
        }
        "session.rename_thread" => {
            let thread_id = required_str(params, "thread_id")?;
            let title = required_str(params, "title")?;
            to_json(&engine.rename_thread(thread_id, title).await?)?
        }
        "session.list_threads" => to_json(&engine.list_threads().await?)?,
        "session.get_transcript" => {
            if let Some(tid) = params.get("thread_id").and_then(Value::as_str) {
                to_json(&engine.get_thread_transcript(tid).await?)?
            } else {
                to_json(&engine.get_transcript().await?)?
            }
        }
        "session.merge_thread" => {
            let source_id = required_str(params, "thread_id")?;
            let target_id = params
                .get("target_thread_id")
                .or_else(|| params.get("target_id"))
                .and_then(Value::as_str);
            let summary = params.get("summary").and_then(Value::as_str);
            to_json(
                &engine
                    .merge_thread_with_summary(source_id, target_id, summary)
                    .await?,
            )?
        }
        "session.thread_metadata" => to_json(&engine.thread_metadata().await?)?,
        "session.delete_thread" => {
            let thread_id = required_str(params, "thread_id")?;
            let next_active_thread_id = engine.delete_thread(thread_id).await?;
            json!({"next_active_thread_id": next_active_thread_id})
        }
        "session.agent_metadata" => to_json(&engine.agent_metadata().await?)?,
        "session.agent_options" => {
            let thread_id = required_str(params, "thread_id")?;
            to_json(&engine.agent_options(thread_id).await?)?
        }
        "session.set_agent_config" => {
            let thread_id = required_str(params, "thread_id")?;
            let config_id = required_str(params, "config_id")?;
            let value = required_str(params, "value")?;
            let set_default = params
                .get("set_default")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            to_json(
                &engine
                    .set_agent_config(thread_id, config_id, value, set_default)
                    .await?,
            )?
        }
        "permissions.resolve" => {
            let decision: PermissionDecisionPayload = serde_json::from_value(params.clone())
                .map_err(|e| EngineError::InvalidParams(e.to_string()))?;
            to_json(&engine.resolve_permission(decision).await?)?
        }
        "session.cancel" => {
            let thread_id = required_str(params, "thread_id")?;
            engine.cancel_turn(thread_id).await?;
            json!({"cancelled": true})
        }
        "session.shell" => {
            let thread_id = required_str(params, "thread_id")?;
            let command = required_str(params, "command")?;
            to_json(&engine.run_shell_command(thread_id, command).await?)?
        }
        "session.create_terminal" | "create_terminal" => {
            let parent_thread_id = params.get("parent_thread_id").and_then(Value::as_str);
            let title = params.get("title").and_then(Value::as_str);
            let cwd = params.get("cwd").and_then(Value::as_str);
            let rows = parse_opt_u16(params, "rows", 24)?;
            let cols = parse_opt_u16(params, "cols", 80)?;
            to_json(
                &engine
                    .create_terminal(parent_thread_id, title, cwd, rows, cols)
                    .await?,
            )?
        }
        "session.pty_write" | "pty_write" => {
            let thread_id = required_str(params, "thread_id")?;
            let bytes_val = params
                .get("bytes")
                .cloned()
                .ok_or_else(|| EngineError::InvalidParams("missing `bytes` param".to_string()))?;
            let bytes: Vec<u8> = serde_json::from_value(bytes_val)
                .map_err(|e| EngineError::InvalidParams(format!("invalid `bytes`: {e}")))?;
            engine.pty_write(thread_id, bytes).await?;
            json!({"ok": true})
        }
        "session.pty_resize" | "pty_resize" => {
            let thread_id = required_str(params, "thread_id")?;
            let rows = parse_req_u16(params, "rows")?;
            let cols = parse_req_u16(params, "cols")?;
            engine.pty_resize(thread_id, rows, cols).await?;
            json!({"ok": true})
        }
        "session.pty_snapshot" | "pty_snapshot" => {
            let thread_id = required_str(params, "thread_id")?;
            let rows = parse_opt_u16(params, "rows", 0)?;
            let cols = parse_opt_u16(params, "cols", 0)?;
            to_json(&engine.pty_snapshot(thread_id, rows, cols).await?)?
        }
        "session.ensure_thread_worktree" | "ensure_thread_worktree" => {
            let thread_id = required_str(params, "thread_id")?;
            let cwd = engine.ensure_thread_worktree(thread_id).await?;
            json!({"cwd": cwd})
        }
        "session.list_worktrees" | "list_worktrees" => to_json(&engine.list_worktrees().await?)?,
        "session.switch_thread_worktree" | "switch_thread_worktree" => {
            let thread_id = required_str(params, "thread_id")?;
            let target_path = params
                .get("target_path")
                .or_else(|| params.get("path"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    EngineError::InvalidParams("missing string param `target_path`".to_string())
                })?;
            let cwd = engine
                .switch_thread_worktree(thread_id, target_path)
                .await?;
            json!({"cwd": cwd})
        }
        "session.remove_worktree" | "remove_worktree" => {
            let path = required_str(params, "path")?;
            let force = params
                .get("force")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            engine.remove_worktree(path, force).await?;
            json!({"ok": true})
        }
        "session.worktree_diff_preview" | "worktree_diff_preview" => {
            let path = required_str(params, "path")?;
            let diff = engine.worktree_diff_preview(path).await?;
            json!({"diff": diff})
        }
        "session.prune_worktrees" | "prune_worktrees" => {
            let summary = engine.prune_worktrees().await?;
            json!({"summary": summary})
        }
        "session.pty_scroll" | "pty_scroll" => {
            let thread_id = required_str(params, "thread_id")?;
            let delta = parse_req_isize(params, "delta")?;
            to_json(&engine.pty_scroll(thread_id, delta).await?)?
        }
        "session.pty_restart" | "pty_restart" => {
            let thread_id = required_str(params, "thread_id")?;
            let rows = parse_opt_u16(params, "rows", 24)?;
            let cols = parse_opt_u16(params, "cols", 80)?;
            to_json(&engine.pty_restart(thread_id, rows, cols).await?)?
        }
        "session.list_recent_prompts" | "list_recent_prompts" => {
            let limit = parse_opt_usize(params, "limit", 200)?;
            to_json(&engine.list_recent_user_prompts(limit).await?)?
        }
        "session.list_workspace_files" | "list_workspace_files" => {
            let root = params.get("root").and_then(Value::as_str);
            to_json(&engine.list_workspace_files(root).await?)?
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}

fn parse_opt_usize(params: &Value, key: &str, default: usize) -> Result<usize, EngineError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| EngineError::InvalidParams(format!("invalid usize param `{key}`"))),
    }
}

fn parse_opt_u16(params: &Value, key: &str, default: u16) -> Result<u16, EngineError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_u64()
            .and_then(|n| u16::try_from(n).ok())
            .ok_or_else(|| EngineError::InvalidParams(format!("invalid u16 param `{key}`"))),
    }
}

fn parse_req_u16(params: &Value, key: &str) -> Result<u16, EngineError> {
    params
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|n| u16::try_from(n).ok())
        .ok_or_else(|| EngineError::InvalidParams(format!("missing u16 param `{key}`")))
}

fn parse_req_isize(params: &Value, key: &str) -> Result<isize, EngineError> {
    params
        .get(key)
        .and_then(Value::as_i64)
        .and_then(|n| isize::try_from(n).ok())
        .ok_or_else(|| EngineError::InvalidParams(format!("missing isize param `{key}`")))
}

/// Handles one JSON-RPC 2.0 request line and returns the serialized response.
pub async fn dispatch_json_rpc(engine: &dyn DaemonEngine, line: &str) -> String {
    let request: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return rpc_error(Value::Null, -32700, format!("parse error: {e}")).to_string(),
    };
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        return rpc_error(id, -32600, "invalid request: missing method".to_string()).to_string();
    };
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
    let response = match route(engine, method, &params).await {
        Ok(Some(result)) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Ok(None) => rpc_error(id, -32601, format!("method not found: {method}")),
        Err(e) => rpc_error(id, e.rpc_code(), e.to_string()),
    };
    response.to_string()
}
