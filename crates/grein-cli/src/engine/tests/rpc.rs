use std::sync::Arc;
use std::time::Duration;

use grein_core::model::{AuthorRole, ConversationNode, NodeId};
use grein_tui::engine_port::TuiEngineEvent;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::timeout;

use super::super::rpc::rpc_error;
use super::super::{
    dispatch_json_rpc, AcpBackendConfig, DaemonEngine, InProcessEngine, UnixSocketEngineClient,
};
use super::write_executable_script;
use crate::client::connect_to_daemon;
use crate::daemon::run_daemon_with_engine;

#[tokio::test]
async fn test_run_shell_command_and_json_rpc_new_methods() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("session.db");
    let engine =
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
            .expect("open");
    let mut rx = engine.subscribe_tui_events();

    let ensured = dispatch_json_rpc(
        &engine,
        r#"{"jsonrpc":"2.0","id":1,"method":"session.ensure_main_thread","params":{}}"#,
    )
    .await;
    let ensured_val: Value = serde_json::from_str(&ensured).expect("json");
    let main_id = ensured_val["result"]["thread_id"]
        .as_str()
        .expect("thread_id")
        .to_string();

    let shell_resp = dispatch_json_rpc(
        &engine,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session.shell",
            "params": {"thread_id": main_id, "command": "printf 'shell-ok'"}
        })
        .to_string(),
    )
    .await;
    let shell_val: Value = serde_json::from_str(&shell_resp).expect("shell json");
    assert_eq!(shell_val["result"]["stdout"], "shell-ok");
    assert_eq!(shell_val["result"]["exit_code"], 0);

    let ev = rx.recv().await.expect("shell event");
    assert!(matches!(
        ev,
        TuiEngineEvent::ShellOutput { stdout, exit_code, .. } if stdout == "shell-ok" && exit_code == 0
    ));

    let bb_list_resp = dispatch_json_rpc(
        &engine,
        r#"{"jsonrpc":"2.0","id":3,"method":"blackboard.list","params":{}}"#,
    )
    .await;
    assert_eq!(
        serde_json::from_str::<Value>(&bb_list_resp).unwrap()["error"]["code"],
        -32601
    );

    let upsert_resp = dispatch_json_rpc(
        &engine,
        r#"{"jsonrpc":"2.0","id":4,"method":"blackboard.upsert","params":{"key":"k","value":"v"}}"#,
    )
    .await;
    assert_eq!(
        serde_json::from_str::<Value>(&upsert_resp).unwrap()["error"]["code"],
        -32601
    );

    let del_resp = dispatch_json_rpc(
        &engine,
        r#"{"jsonrpc":"2.0","id":5,"method":"blackboard.delete","params":{"key":"k"}}"#,
    )
    .await;
    assert_eq!(
        serde_json::from_str::<Value>(&del_resp).unwrap()["error"]["code"],
        -32601
    );
}

#[tokio::test]
async fn test_unix_socket_client_stale_daemon_32601_hint() {
    let temp = tempfile::tempdir().expect("tempdir");
    let socket_path = temp.path().join("stale.sock");
    let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind");

    let server = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let req: Value = serde_json::from_str(&line).expect("json");
                    let id = req.get("id").cloned().unwrap_or(Value::Null);
                    let method = req
                        .get("method")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let resp = rpc_error(id, -32601, format!("method not found: {method}"));
                    if write.write_all(format!("{resp}\n").as_bytes()).await.is_err()
                        || write.flush().await.is_err()
                    {
                        break;
                    }
                }
            });
        }
    });

    let client = UnixSocketEngineClient::connect(&socket_path)
        .await
        .expect("connect");

    let err_create = client
        .create_terminal(None, None, None, 24, 80)
        .await
        .expect_err("create_terminal should fail");
    let err_write = client
        .pty_write("t-1", b"echo hi\n".to_vec())
        .await
        .expect_err("pty_write should fail");
    let err_resize = client
        .pty_resize("t-1", 30, 100)
        .await
        .expect_err("pty_resize should fail");
    let err_snap = client
        .pty_snapshot("t-1", 30, 100)
        .await
        .expect_err("pty_snapshot should fail");
    let err_wt = client
        .ensure_thread_worktree("t-1")
        .await
        .expect_err("ensure_thread_worktree should fail");
    let err_list_wt = client
        .list_worktrees()
        .await
        .expect_err("list_worktrees should fail");
    let err_switch_wt = client
        .switch_thread_worktree("t-1", "/tmp/wt")
        .await
        .expect_err("switch_thread_worktree should fail");
    let err_remove_wt = client
        .remove_worktree("/tmp/wt", false)
        .await
        .expect_err("remove_worktree should fail");
    let err_diff_wt = client
        .worktree_diff_preview("/tmp/wt")
        .await
        .expect_err("worktree_diff_preview should fail");
    let err_prune_wt = client
        .prune_worktrees()
        .await
        .expect_err("prune_worktrees should fail");
    let err_scroll = client
        .pty_scroll("t-1", 5)
        .await
        .expect_err("pty_scroll should fail");
    let err_restart = client
        .pty_restart("t-1", 24, 80)
        .await
        .expect_err("pty_restart should fail");

    for (name, err) in [
        ("create_terminal", err_create),
        ("pty_write", err_write),
        ("pty_resize", err_resize),
        ("pty_snapshot", err_snap),
        ("ensure_thread_worktree", err_wt),
        ("list_worktrees", err_list_wt),
        ("switch_thread_worktree", err_switch_wt),
        ("remove_worktree", err_remove_wt),
        ("worktree_diff_preview", err_diff_wt),
        ("prune_worktrees", err_prune_wt),
        ("pty_scroll", err_scroll),
        ("pty_restart", err_restart),
    ] {
        let msg = err.to_string().to_ascii_lowercase();
        assert!(
            msg.contains("grein daemon")
                && msg.contains("outdated")
                && msg.contains("restart"),
            "{name} -32601 error must explain stale grein daemon needs restart, got: {err}"
        );
    }

    let err_ping = client.ping().await.expect_err("ping -32601");
    let ping_msg = err_ping.to_string().to_ascii_lowercase();
    assert!(
        !ping_msg.contains("outdated"),
        "unrelated -32601 method should keep standard protocol error, got: {err_ping}"
    );

    let prompts = client
        .list_recent_user_prompts(200)
        .await
        .expect("stale -32601 list_recent_user_prompts must fallback to empty Vec");
    assert!(prompts.is_empty());

    let files = client
        .list_workspace_files(None)
        .await
        .expect("stale -32601 list_workspace_files must fallback to empty Vec");
    assert!(files.is_empty());

    server.abort();
}

#[tokio::test]
async fn test_rpc_and_socket_autocomplete_create_root_cwd_and_dedicated_dispatch() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo_dir = temp.path().join("repo");
    let alt_dir = temp.path().join("alt");
    std::fs::create_dir_all(&repo_dir).expect("mkdir repo");
    std::fs::create_dir_all(&alt_dir).expect("mkdir alt");

    let run_git = |dir: &std::path::Path, args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git");
        assert!(status.success(), "git failed: {:?}", args);
    };
    run_git(&repo_dir, &["init"]);
    run_git(&repo_dir, &["config", "user.name", "Grein Test"]);
    run_git(&repo_dir, &["config", "user.email", "test@grein.dev"]);
    std::fs::write(repo_dir.join("alpha.rs"), "pub fn alpha() {}\n").expect("alpha.rs");

    run_git(&alt_dir, &["init"]);
    run_git(&alt_dir, &["config", "user.name", "Grein Test"]);
    run_git(&alt_dir, &["config", "user.email", "test@grein.dev"]);
    std::fs::write(alt_dir.join("beta.rs"), "pub fn beta() {}\n").expect("beta.rs");

    let canonical_alt = std::fs::canonicalize(&alt_dir).expect("canonical alt");
    let canonical_alt_str = canonical_alt.to_string_lossy().to_string();

    let slow_script = r#"#!/bin/sh
while IFS= read -r line; do
    rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
    case "$line" in
        *'"method":"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"slow","version":"1.0"},"authMethods":[],"agentCapabilities":{}}}\n' "$id"
            ;;
        *'"method":"session/new"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sess-slow"}}\n' "$id"
            ;;
        *'"method":"session/prompt"'*)
            sleep 0.4
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-slow","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"slow ok"}}}}\n'
            printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            ;;
    esac
done
"#;
    let script_path = write_executable_script(temp.path(), "slow_acp.sh", slow_script);
    let cfg = AcpBackendConfig {
        command: script_path.to_string_lossy().into_owned(),
        args: Vec::new(),
        auth_method: None,
        cwd: repo_dir.clone(),
    };

    let db_path = temp.path().join("rpc_autocomplete.db");
    let engine = Arc::new(
        InProcessEngine::open_with_config(&db_path, repo_dir.clone(), Some(cfg)).expect("open"),
    );
    let main = engine.ensure_main_thread().await.expect("ensure main");
    let root_leaf = engine
        .runtime
        .storage
        .list_threads()
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
        .active_leaf_id;
    let p1 = ConversationNode::new(NodeId::new(), Some(root_leaf), AuthorRole::User, "first rpc prompt".into());
    let p2 = ConversationNode::new(NodeId::new(), Some(p1.id), AuthorRole::User, "second rpc prompt".into());
    engine.runtime.storage.insert_node(&p1).unwrap();
    engine.runtime.storage.insert_node(&p2).unwrap();

    // Direct dispatch_json_rpc routes.
    let prompts_rpc: Value = serde_json::from_str(
        &dispatch_json_rpc(
            engine.as_ref(),
            r#"{"jsonrpc":"2.0","id":1,"method":"session.list_recent_prompts","params":{}}"#,
        )
        .await,
    )
    .unwrap();
    assert_eq!(
        prompts_rpc["result"],
        json!(["second rpc prompt", "first rpc prompt"])
    );

    let prompts_limited: Value = serde_json::from_str(
        &dispatch_json_rpc(
            engine.as_ref(),
            r#"{"jsonrpc":"2.0","id":2,"method":"session.list_recent_prompts","params":{"limit":1}}"#,
        )
        .await,
    )
    .unwrap();
    assert_eq!(prompts_limited["result"], json!(["second rpc prompt"]));

    let files_default: Value = serde_json::from_str(
        &dispatch_json_rpc(
            engine.as_ref(),
            r#"{"jsonrpc":"2.0","id":3,"method":"session.list_workspace_files","params":{}}"#,
        )
        .await,
    )
    .unwrap();
    assert_eq!(files_default["result"], json!(["alpha.rs"]));

    let files_alt: Value = serde_json::from_str(
        &dispatch_json_rpc(
            engine.as_ref(),
            &json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "session.list_workspace_files",
                "params": {"root": canonical_alt_str}
            })
            .to_string(),
        )
        .await,
    )
    .unwrap();
    assert_eq!(files_alt["result"], json!(["beta.rs"]));

    let create_root_alias: Value = serde_json::from_str(
        &dispatch_json_rpc(
            engine.as_ref(),
            &json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "session.create_root",
                "params": {"title": "Alias Root", "cwd": canonical_alt_str}
            })
            .to_string(),
        )
        .await,
    )
    .unwrap();
    let alias_tid = create_root_alias["result"]["thread_id"]
        .as_str()
        .expect("alias thread_id")
        .to_string();
    assert_eq!(create_root_alias["result"]["custom_name"], "Alias Root");

    let create_root_bad: Value = serde_json::from_str(
        &dispatch_json_rpc(
            engine.as_ref(),
            r#"{"jsonrpc":"2.0","id":6,"method":"session.create_root","params":{"cwd":"/nonexistent/grein_bad_cwd"}}"#,
        )
        .await,
    )
    .unwrap();
    assert_eq!(create_root_bad["error"]["code"], -32602);

    // Socket server + UnixSocketEngineClient dedicated dispatch + DaemonClient helpers.
    let socket_path = temp.path().join("autocomplete.sock");
    let sock_clone = socket_path.clone();
    let engine_clone = Arc::clone(&engine);
    let server = tokio::spawn(async move {
        let _ = run_daemon_with_engine(sock_clone, engine_clone).await;
    });
    for _ in 0..50 {
        if socket_path.exists() && UnixStream::connect(&socket_path).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let client = Arc::new(
        UnixSocketEngineClient::connect(&socket_path)
            .await
            .expect("connect"),
    );

    // Hold shared socket io lock with a slow in-flight turn; dedicated calls must not block.
    let turn_client = Arc::clone(&client);
    let turn_tid = main.thread_id.clone();
    let turn_handle = tokio::spawn(async move {
        turn_client
            .send_turn_for_thread(&turn_tid, "hold shared lock")
            .await
    });
    tokio::time::sleep(Duration::from_millis(40)).await;

    let sock_prompts = timeout(
        Duration::from_millis(200),
        client.list_recent_user_prompts(200),
    )
    .await
    .expect("dedicated list_recent_user_prompts must not block behind in-flight turn")
    .expect("list_recent_user_prompts");
    assert!(
        sock_prompts == vec!["hold shared lock", "second rpc prompt", "first rpc prompt"]
            || sock_prompts == vec!["second rpc prompt", "first rpc prompt"],
        "unexpected prompts during in-flight turn: {sock_prompts:?}"
    );

    let sock_files = timeout(
        Duration::from_millis(200),
        client.list_workspace_files(Some(&canonical_alt_str)),
    )
    .await
    .expect("dedicated list_workspace_files must not block behind in-flight turn")
    .expect("list_workspace_files");
    assert_eq!(sock_files, vec!["beta.rs"]);

    let sock_root = timeout(
        Duration::from_millis(200),
        client.create_root_thread_in_cwd(Some("Socket Cwd Root"), Some(&canonical_alt_str)),
    )
    .await
    .expect("dedicated create_root_thread_in_cwd must not block behind in-flight turn")
    .expect("create_root_thread_in_cwd");
    assert_eq!(sock_root.custom_name.as_deref(), Some("Socket Cwd Root"));

    turn_handle.await.expect("join").expect("slow turn");

    let mut raw_client = connect_to_daemon(&socket_path)
        .await
        .expect("connect DaemonClient");
    let raw_prompts = raw_client
        .session_list_recent_prompts(Some(1))
        .await
        .expect("session_list_recent_prompts");
    assert_eq!(raw_prompts["result"], json!(["hold shared lock"]));

    let raw_files = raw_client
        .session_list_workspace_files(None)
        .await
        .expect("session_list_workspace_files");
    assert_eq!(raw_files["result"], json!(["alpha.rs"]));

    let raw_root = raw_client
        .session_create_root_thread_in_cwd(Some("Raw Cwd Root"), Some(&canonical_alt_str))
        .await
        .expect("session_create_root_thread_in_cwd");
    let raw_root_tid = raw_root["result"]["thread_id"]
        .as_str()
        .expect("raw root tid")
        .to_string();

    let meta = client.thread_metadata().await.expect("thread_metadata");
    for expected_tid in [alias_tid, sock_root.thread_id, raw_root_tid] {
        let entry = meta
            .iter()
            .find(|m| m.thread_id == expected_tid)
            .expect("created root meta");
        assert_eq!(entry.cwd.as_deref(), Some(canonical_alt_str.as_str()));
    }

    server.abort();
}
