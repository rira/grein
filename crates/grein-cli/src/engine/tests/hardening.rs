use std::sync::Arc;
use std::time::Duration;

use grein_core::port::dto::TuiEngineEvent;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::time::timeout;

use super::super::rpc::rpc_error;
use super::super::{
    dispatch_json_rpc, AcpBackendConfig, EngineClient, EngineError, InProcessEngine,
    PermissionDecisionPayload, UnixSocketEngineClient,
};
use super::write_executable_script;

fn assert_rpc_err(raw: &str, expected_code: i64, context: &str) {
    let val: Value = serde_json::from_str(raw).expect("valid rpc error json");
    assert_eq!(
        val["error"]["code"].as_i64(),
        Some(expected_code),
        "{context}: unexpected response {val}"
    );
}

#[test]
fn test_engine_error_rpc_codes() {
    assert_eq!(EngineError::InvalidParams("x".into()).rpc_code(), -32602);
    assert_eq!(EngineError::NotFound("x".into()).rpc_code(), -32004);
    assert_eq!(EngineError::Protocol("x".into()).rpc_code(), -32600);
    assert_eq!(EngineError::Storage("x".into()).rpc_code(), -32000);
    assert_eq!(EngineError::Transport("x".into()).rpc_code(), -32000);
}

#[tokio::test]
async fn test_rpc_negative_and_edge_matrix() {
    let engine = InProcessEngine::in_memory().expect("in_memory");

    assert_rpc_err(
        &dispatch_json_rpc(&engine, "{not-valid-json").await,
        -32700,
        "malformed json",
    );
    assert_rpc_err(
        &dispatch_json_rpc(&engine, r#"{"jsonrpc":"2.0","id":1}"#).await,
        -32600,
        "missing method",
    );
    assert_rpc_err(
        &dispatch_json_rpc(&engine, r#"{"jsonrpc":"2.0","id":2,"method":42}"#).await,
        -32600,
        "non-string method",
    );
    assert_rpc_err(
        &dispatch_json_rpc(&engine, r#"{"jsonrpc":"2.0","id":3,"method":"unknown.route"}"#).await,
        -32601,
        "unknown method without params",
    );

    let invalid_param_cases = vec![
        ("session.send_turn missing prompt", json!({"method": "session.send_turn", "params": {}})),
        ("session.send_turn non-string prompt", json!({"method": "session.send_turn", "params": {"prompt": 123}})),
        ("session.send_thread_turn missing tid", json!({"method": "session.send_thread_turn", "params": {"prompt": "hi"}})),
        ("session.send_thread_turn missing prompt", json!({"method": "session.send_thread_turn", "params": {"thread_id": "t1"}})),
        ("session.rename_thread missing tid", json!({"method": "session.rename_thread", "params": {"title": "x"}})),
        ("session.rename_thread missing title", json!({"method": "session.rename_thread", "params": {"thread_id": "t1"}})),
        ("session.merge_thread missing tid", json!({"method": "session.merge_thread", "params": {}})),
        ("session.delete_thread missing tid", json!({"method": "session.delete_thread", "params": {}})),
        ("session.agent_options missing tid", json!({"method": "session.agent_options", "params": {}})),
        ("session.set_agent_config missing tid", json!({"method": "session.set_agent_config", "params": {"config_id": "m", "value": "v"}})),
        ("session.set_agent_config missing cfg", json!({"method": "session.set_agent_config", "params": {"thread_id": "t1", "value": "v"}})),
        ("session.set_agent_config missing val", json!({"method": "session.set_agent_config", "params": {"thread_id": "t1", "config_id": "m"}})),
        ("permissions.resolve invalid payload", json!({"method": "permissions.resolve", "params": {"request_id": 123}})),
        ("permissions.resolve wrong allow type", json!({"method": "permissions.resolve", "params": {"request_id": "r1", "allow": "yes"}})),
        ("session.cancel missing tid", json!({"method": "session.cancel", "params": {}})),
        ("session.shell missing tid", json!({"method": "session.shell", "params": {"command": "echo"}})),
        ("session.shell missing cmd", json!({"method": "session.shell", "params": {"thread_id": "t1"}})),
        ("create_terminal rows overflow", json!({"method": "create_terminal", "params": {"rows": 65536}})),
        ("create_terminal cols string", json!({"method": "create_terminal", "params": {"cols": "80"}})),
        ("pty_write missing tid", json!({"method": "pty_write", "params": {"bytes": [65]}})),
        ("pty_write missing bytes", json!({"method": "pty_write", "params": {"thread_id": "t1"}})),
        ("pty_write non-array bytes", json!({"method": "pty_write", "params": {"thread_id": "t1", "bytes": "abc"}})),
        ("pty_write u8 overflow bytes", json!({"method": "pty_write", "params": {"thread_id": "t1", "bytes": [256]}})),
        ("pty_resize missing tid", json!({"method": "pty_resize", "params": {"rows": 24, "cols": 80}})),
        ("pty_resize missing rows", json!({"method": "pty_resize", "params": {"thread_id": "t1", "cols": 80}})),
        ("pty_resize rows overflow", json!({"method": "pty_resize", "params": {"thread_id": "t1", "rows": 70000, "cols": 80}})),
        ("pty_resize negative cols", json!({"method": "pty_resize", "params": {"thread_id": "t1", "rows": 24, "cols": -1}})),
        ("pty_snapshot missing tid", json!({"method": "pty_snapshot", "params": {}})),
        ("pty_snapshot cols overflow", json!({"method": "pty_snapshot", "params": {"thread_id": "t1", "cols": 65536}})),
        ("ensure_thread_worktree missing tid", json!({"method": "ensure_thread_worktree", "params": {}})),
        ("switch_thread_worktree missing tid", json!({"method": "switch_thread_worktree", "params": {"path": "/tmp"}})),
        ("switch_thread_worktree missing target", json!({"method": "switch_thread_worktree", "params": {"thread_id": "t1"}})),
        ("remove_worktree missing path", json!({"method": "remove_worktree", "params": {}})),
        ("worktree_diff_preview missing path", json!({"method": "worktree_diff_preview", "params": {}})),
        ("pty_scroll missing tid", json!({"method": "pty_scroll", "params": {"delta": 1}})),
        ("pty_scroll missing delta", json!({"method": "pty_scroll", "params": {"thread_id": "t1"}})),
        ("pty_scroll invalid delta", json!({"method": "pty_scroll", "params": {"thread_id": "t1", "delta": "up"}})),
        ("pty_restart missing tid", json!({"method": "pty_restart", "params": {}})),
        ("pty_restart rows overflow", json!({"method": "pty_restart", "params": {"thread_id": "t1", "rows": 99999}})),
        ("list_recent_prompts invalid limit", json!({"method": "list_recent_prompts", "params": {"limit": "ten"}})),
        ("list_recent_prompts negative limit", json!({"method": "list_recent_prompts", "params": {"limit": -5}})),
    ];

    for (label, mut req) in invalid_param_cases {
        req.as_object_mut()
            .unwrap()
            .insert("jsonrpc".into(), json!("2.0"));
        req.as_object_mut().unwrap().insert("id".into(), json!(10));
        let resp = dispatch_json_rpc(&engine, &req.to_string()).await;
        assert_rpc_err(&resp, -32602, label);
    }
}

#[tokio::test]
async fn test_rpc_unprefixed_aliases_and_optional_null_defaults() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    for args in [
        vec!["init"],
        vec!["config", "user.name", "Grein Test"],
        vec!["config", "user.email", "test@grein.dev"],
    ] {
        assert!(std::process::Command::new("git")
            .args(&args)
            .current_dir(&repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git")
            .success());
    }
    std::fs::write(repo.join("seed.txt"), "hello\n").expect("write");
    for args in [vec!["add", "."], vec!["commit", "-m", "init"]] {
        assert!(std::process::Command::new("git")
            .args(&args)
            .current_dir(&repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git")
            .success());
    }
    let canonical_repo = std::fs::canonicalize(&repo).expect("canonical");
    let db_path = temp.path().join("rpc_aliases.db");
    let engine = InProcessEngine::open_with_config(&db_path, canonical_repo.clone(), None)
        .expect("open");

    let main_resp: Value = serde_json::from_str(
        &dispatch_json_rpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":1,"method":"session.ensure_main_thread","params":{}}"#,
        )
        .await,
    )
    .unwrap();
    let main_id = main_resp["result"]["thread_id"].as_str().unwrap().to_string();

    // session.send_turn without thread_id and with thread_id (offline) + session.get_transcript
    let offline_turn: Value = serde_json::from_str(
        &dispatch_json_rpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":2,"method":"session.send_turn","params":{"prompt":"offline root prompt"}}"#,
        )
        .await,
    )
    .unwrap();
    assert!(offline_turn["result"]["node_id"].is_string());

    let offline_thread_turn: Value = serde_json::from_str(
        &dispatch_json_rpc(
            &engine,
            &json!({"jsonrpc":"2.0","id":3,"method":"session.send_turn","params":{"thread_id": main_id, "prompt": "fails offline"}}).to_string(),
        )
        .await,
    )
    .unwrap();
    assert_eq!(offline_thread_turn["error"]["code"], -32000);

    let transcript_default: Value = serde_json::from_str(
        &dispatch_json_rpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":4,"method":"session.get_transcript","params":{}}"#,
        )
        .await,
    )
    .unwrap();
    assert_eq!(transcript_default["result"].as_array().unwrap().len(), 2);

    let transcript_tid: Value = serde_json::from_str(
        &dispatch_json_rpc(
            &engine,
            &json!({"jsonrpc":"2.0","id":5,"method":"session.get_transcript","params":{"thread_id": main_id}}).to_string(),
        )
        .await,
    )
    .unwrap();
    assert_eq!(transcript_tid["result"].as_array().unwrap().len(), 2);

    // source_thread_id alias in fork_thread + target_id alias in merge_thread
    let forked: Value = serde_json::from_str(
        &dispatch_json_rpc(
            &engine,
            &json!({"jsonrpc":"2.0","id":6,"method":"session.fork_thread","params":{"source_thread_id": main_id, "title": "alias-fork"}}).to_string(),
        )
        .await,
    )
    .unwrap();
    let fork_id = forked["result"]["thread_id"].as_str().unwrap().to_string();

    let merged: Value = serde_json::from_str(
        &dispatch_json_rpc(
            &engine,
            &json!({"jsonrpc":"2.0","id":7,"method":"session.merge_thread","params":{"thread_id": fork_id, "target_id": main_id, "summary": "joined via alias"}}).to_string(),
        )
        .await,
    )
    .unwrap();
    assert_eq!(merged["result"]["merged"], true);

    // agent_metadata, agent_options, cancel, permissions.resolve
    let agent_meta: Value = serde_json::from_str(
        &dispatch_json_rpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":8,"method":"session.agent_metadata","params":{}}"#,
        )
        .await,
    )
    .unwrap();
    assert!(agent_meta["result"].is_array());

    let agent_opts: Value = serde_json::from_str(
        &dispatch_json_rpc(
            &engine,
            &json!({"jsonrpc":"2.0","id":9,"method":"session.agent_options","params":{"thread_id": main_id}}).to_string(),
        )
        .await,
    )
    .unwrap();
    assert_eq!(agent_opts["result"]["thread_id"], main_id);

    let cancel_res: Value = serde_json::from_str(
        &dispatch_json_rpc(
            &engine,
            &json!({"jsonrpc":"2.0","id":10,"method":"session.cancel","params":{"thread_id": main_id}}).to_string(),
        )
        .await,
    )
    .unwrap();
    assert_eq!(cancel_res["result"]["cancelled"], true);

    let perm_res: Value = serde_json::from_str(
        &dispatch_json_rpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":11,"method":"permissions.resolve","params":{"request_id":"req-1","allow":true}}"#,
        )
        .await,
    )
    .unwrap();
    assert_eq!(perm_res["result"]["request_id"], "req-1");

    // Unprefixed PTY aliases with null defaults
    let term: Value = serde_json::from_str(
        &dispatch_json_rpc(
            &engine,
            &json!({"jsonrpc":"2.0","id":12,"method":"create_terminal","params":{"parent_thread_id": main_id, "rows": null, "cols": null}}).to_string(),
        )
        .await,
    )
    .unwrap();
    let term_id = term["result"]["thread_id"].as_str().unwrap().to_string();

    for (id, method, params) in [
        (13, "pty_resize", json!({"thread_id": term_id, "rows": 25, "cols": 90})),
        (14, "pty_write", json!({"thread_id": term_id, "bytes": [101, 99, 104, 111, 10]})),
        (15, "pty_snapshot", json!({"thread_id": term_id, "rows": null, "cols": null})),
        (16, "pty_scroll", json!({"thread_id": term_id, "delta": 2})),
        (17, "pty_restart", json!({"thread_id": term_id, "rows": null, "cols": null})),
        (18, "list_recent_prompts", json!({"limit": null})),
        (19, "list_workspace_files", json!({})),
    ] {
        let resp: Value = serde_json::from_str(
            &dispatch_json_rpc(
                &engine,
                &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}).to_string(),
            )
            .await,
        )
        .unwrap();
        assert!(resp.get("result").is_some(), "{method} failed: {resp}");
    }

    // Unprefixed worktree aliases (including switch `path` alias)
    let wt_res: Value = serde_json::from_str(
        &dispatch_json_rpc(
            &engine,
            &json!({"jsonrpc":"2.0","id":20,"method":"ensure_thread_worktree","params":{"thread_id": fork_id}}).to_string(),
        )
        .await,
    )
    .unwrap();
    let wt_path = wt_res["result"]["cwd"].as_str().unwrap().to_string();

    let wt_list: Value = serde_json::from_str(
        &dispatch_json_rpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":21,"method":"list_worktrees","params":{}}"#,
        )
        .await,
    )
    .unwrap();
    assert!(wt_list["result"].as_array().unwrap().len() >= 2);

    let wt_diff: Value = serde_json::from_str(
        &dispatch_json_rpc(
            &engine,
            &json!({"jsonrpc":"2.0","id":22,"method":"worktree_diff_preview","params":{"path": wt_path}}).to_string(),
        )
        .await,
    )
    .unwrap();
    assert!(wt_diff["result"]["diff"].is_string());

    let wt_switch: Value = serde_json::from_str(
        &dispatch_json_rpc(
            &engine,
            &json!({"jsonrpc":"2.0","id":23,"method":"switch_thread_worktree","params":{"thread_id": fork_id, "path": canonical_repo.to_string_lossy()}}).to_string(),
        )
        .await,
    )
    .unwrap();
    assert_eq!(wt_switch["result"]["cwd"], canonical_repo.to_string_lossy().as_ref());

    let wt_rm: Value = serde_json::from_str(
        &dispatch_json_rpc(
            &engine,
            &json!({"jsonrpc":"2.0","id":24,"method":"remove_worktree","params":{"path": wt_path, "force": true}}).to_string(),
        )
        .await,
    )
    .unwrap();
    assert_eq!(wt_rm["result"]["ok"], true);

    let wt_prune: Value = serde_json::from_str(
        &dispatch_json_rpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":25,"method":"prune_worktrees","params":{}}"#,
        )
        .await,
    )
    .unwrap();
    assert!(wt_prune["result"]["summary"].is_string());
}

async fn exercise_engine_client_methods(client: &EngineClient, repo_path: &str) {
    client.ping().await.expect("ping");
    let main = client.ensure_main_thread().await.expect("ensure_main_thread");
    let t1 = client.send_turn("first via client").await.expect("send_turn");
    assert!(!t1.node_id.is_empty());
    let t2 = client
        .send_turn_for_thread(&main.thread_id, "second via client")
        .await
        .expect("send_turn_for_thread");
    assert!(!t2.node_id.is_empty());

    let forked1 = client.fork_thread(Some(&main.thread_id)).await.expect("fork_thread");
    let forked2 = client
        .fork_thread_with_title(Some(&main.thread_id), Some("Client Branch"))
        .await
        .expect("fork_thread_with_title");
    let root2 = client.create_root_thread(Some("Client Root")).await.expect("create_root_thread");
    let root3 = client
        .create_root_thread_in_cwd(Some("Scoped Client Root"), Some(repo_path))
        .await
        .expect("create_root_thread_in_cwd");
    let renamed = client
        .rename_thread(&root2.thread_id, "Renamed Client Root")
        .await
        .expect("rename_thread");
    assert_eq!(renamed.custom_name.as_deref(), Some("Renamed Client Root"));

    assert!(client.list_threads().await.expect("list_threads").len() >= 4);
    assert!(!client.get_transcript().await.expect("get_transcript").is_empty());
    assert!(!client
        .get_thread_transcript(&main.thread_id)
        .await
        .expect("get_thread_transcript")
        .is_empty());

    let merged1 = client.merge_thread(&forked1.thread_id).await.expect("merge_thread");
    assert!(merged1.merged);
    let merged2 = client
        .merge_thread_with_summary(&forked2.thread_id, Some(&main.thread_id), Some("Client summary"))
        .await
        .expect("merge_thread_with_summary");
    assert!(merged2.merged);

    let perm = client
        .resolve_permission(PermissionDecisionPayload {
            request_id: "none".into(),
            allow: true,
        })
        .await
        .expect("resolve_permission");
    assert!(matches!(
        perm,
        super::super::EngineEvent::PermissionResolved { .. }
    ));
    client.cancel_turn(&main.thread_id).await.expect("cancel_turn");

    let shell = client
        .run_shell_command(&main.thread_id, "printf 'client-sh'")
        .await
        .expect("run_shell_command");
    assert_eq!(shell.stdout, "client-sh");

    let snap = client.snapshot().await.expect("snapshot");
    assert!(snap.thread_count >= 4);
    let _rx = client.subscribe_events().await.expect("subscribe_events");
    assert!(!client.thread_metadata().await.expect("thread_metadata").is_empty());
    assert!(!client.agent_metadata().await.expect("agent_metadata").is_empty());

    let opts = client.agent_options(&main.thread_id).await.expect("agent_options");
    assert_eq!(opts.thread_id, main.thread_id);
    let updated_cfg = client
        .set_agent_config(&main.thread_id, "model", "m2", false)
        .await
        .expect("set_agent_config");
    assert_eq!(updated_cfg.thread_id, main.thread_id);

    let term = client
        .create_terminal(Some(&main.thread_id), Some("Client Term"), Some(repo_path), 24, 80)
        .await
        .expect("create_terminal");
    client.pty_write(&term.thread_id, b"echo ok\n".to_vec()).await.expect("pty_write");
    client.pty_resize(&term.thread_id, 25, 90).await.expect("pty_resize");
    let pty_snap = client.pty_snapshot(&term.thread_id, 25, 90).await.expect("pty_snapshot");
    assert_eq!((pty_snap.rows, pty_snap.cols), (25, 90));
    let scrolled = client.pty_scroll(&term.thread_id, 1).await.expect("pty_scroll");
    assert_eq!(scrolled.rows, 25);
    let restarted = client.pty_restart(&term.thread_id, 24, 80).await.expect("pty_restart");
    assert_eq!((restarted.rows, restarted.cols), (24, 80));

    let wt_cwd = client
        .ensure_thread_worktree(&root3.thread_id)
        .await
        .expect("ensure_thread_worktree");
    assert!(client.list_worktrees().await.expect("list_worktrees").len() >= 2);
    let _diff = client
        .worktree_diff_preview(&wt_cwd)
        .await
        .expect("worktree_diff_preview");
    let switched = client
        .switch_thread_worktree(&root3.thread_id, repo_path)
        .await
        .expect("switch_thread_worktree");
    assert_eq!(switched, repo_path);
    client.remove_worktree(&wt_cwd, true).await.expect("remove_worktree");
    let _pruned = client.prune_worktrees().await.expect("prune_worktrees");

    assert!(!client
        .list_recent_user_prompts(10)
        .await
        .expect("list_recent_user_prompts")
        .is_empty());
    assert!(client
        .list_workspace_files(Some(repo_path))
        .await
        .expect("list_workspace_files")
        .contains(&"seed.txt".to_string()));

    let next_tid = client.delete_thread(&term.thread_id).await.expect("delete_thread");
    assert!(!next_tid.is_empty());
}

#[tokio::test]
async fn test_engine_client_both_in_process_and_socket_variants() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    for args in [
        vec!["init"],
        vec!["config", "user.name", "Grein Test"],
        vec!["config", "user.email", "test@grein.dev"],
    ] {
        assert!(std::process::Command::new("git")
            .args(&args)
            .current_dir(&repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git")
            .success());
    }
    std::fs::write(repo.join("seed.txt"), "seed\n").expect("write");
    for args in [vec!["add", "."], vec!["commit", "-m", "init"]] {
        assert!(std::process::Command::new("git")
            .args(&args)
            .current_dir(&repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git")
            .success());
    }
    let canonical_repo = std::fs::canonicalize(&repo).expect("canonical");
    let repo_str = canonical_repo.to_string_lossy().to_string();

    let mock_script = r#"#!/bin/sh
sess=0
while IFS= read -r line; do
    rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
    case "$line" in
        *'"method":"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"mock","version":"1.0"},"authMethods":[],"agentCapabilities":{}}}\n' "$id"
            ;;
        *'"method":"session/new"'*)
            sess=$((sess+1))
            printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sess-%s","configOptions":[{"id":"model","name":"Model","category":"model","currentValue":"m1","options":[{"value":"m1","name":"M1"},{"value":"m2","name":"M2"}]}]}}\n' "$id" "$sess"
            ;;
        *'"method":"session/set_config_option"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"configOptions":[{"id":"model","name":"Model","category":"model","currentValue":"m2","options":[{"value":"m1","name":"M1"},{"value":"m2","name":"M2"}]}]}}\n' "$id"
            ;;
        *'"method":"session/close"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":null}\n' "$id"
            ;;
        *'"method":"session/prompt"'*)
            sid=${line#*\"sessionId\":\"}; sid=${sid%%\"*}
            printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"ok"}}}}\n' "$sid"
            printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            ;;
    esac
done
"#;
    let script_path = write_executable_script(temp.path(), "client_acp.sh", mock_script);
    let cfg = AcpBackendConfig {
        command: script_path.to_string_lossy().into_owned(),
        args: Vec::new(),
        auth_method: None,
        cwd: canonical_repo.clone(),
    };

    let in_proc_db = temp.path().join("client_inproc.db");
    let in_proc_engine =
        InProcessEngine::open_with_config(&in_proc_db, canonical_repo.clone(), Some(cfg.clone()))
            .expect("open inproc");
    let in_proc_client = EngineClient::in_process(in_proc_engine);
    exercise_engine_client_methods(&in_proc_client, &repo_str).await;
    drop(in_proc_client);

    let sock_db = temp.path().join("client_sock.db");
    let sock_path = temp.path().join("client_sock.sock");
    let daemon_engine = Arc::new(
        InProcessEngine::open_with_config(&sock_db, canonical_repo.clone(), Some(cfg))
            .expect("open daemon"),
    );
    let sock_clone = sock_path.clone();
    let server = tokio::spawn(async move {
        let _ = crate::daemon::run_daemon_with_engine(sock_clone, daemon_engine).await;
    });
    for _ in 0..50 {
        if sock_path.exists() && tokio::net::UnixStream::connect(&sock_path).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let sock_client = EngineClient::socket(
        UnixSocketEngineClient::connect(&sock_path)
            .await
            .expect("connect socket client"),
    );
    exercise_engine_client_methods(&sock_client, &repo_str).await;
    server.abort();
}

#[tokio::test]
async fn test_unix_socket_client_error_eof_and_malformed_responses() {
    let temp = tempfile::tempdir().expect("tempdir");
    let missing_sock = temp.path().join("missing.sock");
    assert!(matches!(
        UnixSocketEngineClient::connect(&missing_sock).await,
        Err(EngineError::Transport(_))
    ));

    let sock_path = temp.path().join("weird.sock");
    let listener = UnixListener::bind(&sock_path).expect("bind");
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
                    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
                    let out = match method {
                        "session.ping" => {
                            // Immediate EOF on shared socket
                            break;
                        }
                        "session.create_root_thread" => {
                            // Immediate EOF on dedicated socket
                            break;
                        }
                        "session.list_threads" => "{bad-json\n".to_string(),
                        "session.get_transcript" => format!("{}\n", json!({"jsonrpc":"2.0","id":id})),
                        "session.agent_metadata" => {
                            format!("{}\n", json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000}}))
                        }
                        "session.delete_thread"
                        | "session.ensure_thread_worktree"
                        | "session.switch_thread_worktree"
                        | "session.worktree_diff_preview"
                        | "session.prune_worktrees" => {
                            format!("{}\n", json!({"jsonrpc":"2.0","id":id,"result":{}}))
                        }
                        "session.subscribe_events" => {
                            let ack = json!({"jsonrpc":"2.0","id":id,"result":{"subscribed":true}});
                            let _ = write.write_all(format!("{ack}\n").as_bytes()).await;
                            let _ = write.write_all(b"not-valid-event-json\n\n").await;
                            let ev = serde_json::to_string(&TuiEngineEvent::TurnCompleted {
                                thread_id: "t-ev".into(),
                            })
                            .unwrap();
                            let _ = write.write_all(format!("{ev}\n").as_bytes()).await;
                            let _ = write.flush().await;
                            break;
                        }
                        _ => format!("{}\n", rpc_error(id, -32601, "nope".into())),
                    };
                    if write.write_all(out.as_bytes()).await.is_err() || write.flush().await.is_err()
                    {
                        break;
                    }
                }
            });
        }
    });

    let client = UnixSocketEngineClient::connect(&sock_path)
        .await
        .expect("connect");

    assert!(matches!(
        client.list_threads().await,
        Err(EngineError::Protocol(_))
    ));
    assert!(matches!(
        client.get_transcript().await,
        Err(EngineError::Protocol(msg)) if msg.contains("missing result")
    ));
    assert!(matches!(
        client.agent_metadata().await,
        Err(EngineError::Protocol(msg)) if msg.contains("unknown rpc error")
    ));
    assert!(matches!(
        client.delete_thread("t-1").await,
        Err(EngineError::Protocol(msg)) if msg.contains("next_active_thread_id")
    ));
    assert!(matches!(
        client.ensure_thread_worktree("t-1").await,
        Err(EngineError::Protocol(msg)) if msg.contains("missing cwd")
    ));
    assert!(matches!(
        client.switch_thread_worktree("t-1", "/tmp").await,
        Err(EngineError::Protocol(msg)) if msg.contains("missing cwd")
    ));
    assert!(matches!(
        client.worktree_diff_preview("/tmp").await,
        Err(EngineError::Protocol(msg)) if msg.contains("missing diff")
    ));
    assert!(matches!(
        client.prune_worktrees().await,
        Err(EngineError::Protocol(msg)) if msg.contains("missing summary")
    ));
    assert!(matches!(
        client.create_root_thread(None).await,
        Err(EngineError::Transport(msg)) if msg.contains("closed connection")
    ));

    let mut sub_rx = client
        .subscribe_events()
        .await
        .expect("subscribe")
        .expect("receiver");
    let ev = timeout(Duration::from_millis(300), sub_rx.recv())
        .await
        .expect("recv timeout")
        .expect("event");
    assert!(matches!(ev, TuiEngineEvent::TurnCompleted { thread_id } if thread_id == "t-ev"));
    // Next recv sees channel closed after server EOF
    assert!(timeout(Duration::from_millis(300), sub_rx.recv())
        .await
        .expect("eof timeout")
        .is_none());

    // Shared socket EOF
    assert!(matches!(
        client.ping().await,
        Err(EngineError::Transport(msg)) if msg.contains("closed connection")
    ));

    // subscribe_events RPC error & EOF cases
    let err_sock = temp.path().join("sub_err.sock");
    let err_listener = UnixListener::bind(&err_sock).expect("bind err");
    let sub_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let sub_calls_srv = Arc::clone(&sub_calls);
    let err_server = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = err_listener.accept().await else {
                break;
            };
            let calls = Arc::clone(&sub_calls_srv);
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();
                if let Ok(Some(line)) = lines.next_line().await {
                    if line.contains("session.subscribe_events")
                        && calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0
                    {
                        let _ = write
                            .write_all(
                                format!("{}\n", rpc_error(json!(1), -32601, "nope".into()))
                                    .as_bytes(),
                            )
                            .await;
                        let _ = write.flush().await;
                    }
                }
            });
        }
    });
    let err_client = UnixSocketEngineClient::connect(&err_sock).await.unwrap();
    assert!(err_client.subscribe_events().await.unwrap().is_none());
    assert!(err_client.subscribe_events().await.unwrap().is_none());

    server.abort();
    err_server.abort();
}
