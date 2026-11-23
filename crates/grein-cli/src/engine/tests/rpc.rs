use grein_tui::engine_port::TuiEngineEvent;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::super::rpc::rpc_error;
use super::super::{dispatch_json_rpc, DaemonEngine, InProcessEngine, UnixSocketEngineClient};

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

    server.abort();
}
