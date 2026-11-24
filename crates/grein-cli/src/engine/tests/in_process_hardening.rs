use std::process::Command;
use std::time::Duration;

use tokio::time::timeout;

use super::super::{AcpBackendConfig, EngineError, InProcessEngine};
use super::write_executable_script;

fn init_git_repo(dir: &std::path::Path) {
    let run = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git");
        assert!(status.success(), "git failed: {args:?}");
    };
    run(&["init"]);
    run(&["config", "user.name", "Grein Test"]);
    run(&["config", "user.email", "test@grein.dev"]);
    std::fs::write(dir.join("README.md"), "base\n").expect("write README");
    run(&["add", "."]);
    run(&["commit", "-m", "initial"]);
}

#[tokio::test]
async fn test_acp_turns_validation_offline_terminal_and_shell_guards() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("turns_guards.db");
    let engine = InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
        .expect("open");

    // Empty prompt rejected
    let main = engine.ensure_main_thread().await.expect("main");
    let err_empty = engine
        .send_turn_for_thread(&main.thread_id, "")
        .await
        .expect_err("empty prompt must fail");
    assert!(matches!(err_empty, EngineError::InvalidParams(_)));

    // Missing / invalid thread_id rejected
    assert!(matches!(
        engine.send_turn_for_thread("not-a-uuid", "hi").await,
        Err(EngineError::NotFound(_))
    ));
    assert!(matches!(
        engine
            .send_turn_for_thread("01930000-0000-7000-8000-000000000001", "hi")
            .await,
        Err(EngineError::NotFound(_))
    ));

    // Terminal thread prompt rejection
    let term = engine
        .create_terminal(Some(&main.thread_id), Some("term"), None, 24, 80)
        .await
        .expect("create terminal");
    let err_term = engine
        .send_turn_for_thread(&term.thread_id, "hi")
        .await
        .expect_err("terminal prompt must fail");
    assert!(matches!(err_term, EngineError::InvalidParams(msg) if msg.contains("terminal")));

    // Empty shell command rejected
    let err_shell = engine
        .run_shell_command(&main.thread_id, "   ")
        .await
        .expect_err("empty shell cmd must fail");
    assert!(matches!(err_shell, EngineError::InvalidParams(_)));

    // Offline send_turn when ONLY terminal thread exists -> rejects
    let term_only_engine = InProcessEngine::in_memory().expect("in_memory");
    let _only_term = term_only_engine
        .create_terminal(None, Some("only-term"), None, 24, 80)
        .await
        .expect("create terminal");
    let err_offline_term = term_only_engine
        .send_turn("hello")
        .await
        .expect_err("offline send_turn with only terminal must fail");
    assert!(matches!(
        err_offline_term,
        EngineError::InvalidParams(msg) if msg.contains("terminal")
    ));
}

#[tokio::test]
async fn test_acp_turns_submit_prompt_crash_and_driver_error_recovery() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("submit_crash.db");
    let flag_crash_prompt = temp.path().join("crash_prompt.flag");
    std::fs::write(&flag_crash_prompt, "1").expect("flag");

    let script = r#"#!/bin/sh
while IFS= read -r line; do
    rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
    case "$line" in
        *'"method":"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"mock","version":"1.0"},"authMethods":[],"agentCapabilities":{}}}\n' "$id"
            ;;
        *'"method":"session/new"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sess-p"}}\n' "$id"
            ;;
        *'"method":"session/prompt"'*)
            if [ -f "$GREIN_PROMPT_CRASH_FLAG" ]; then
                rm -f "$GREIN_PROMPT_CRASH_FLAG"
                exit 1
            fi
            if printf '%s' "$line" | grep -q "stream-err"; then
                printf '{"jsonrpc":"2.0","id":null,"error":{"code":-32000,"message":"async stream crash"}}\n'
                sleep 0.3
            else
                printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-p","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"recovered"}}}}\n'
                printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            fi
            ;;
    esac
done
"#;
    let wrapper = format!(
        "#!/bin/sh\nexport GREIN_PROMPT_CRASH_FLAG={:?}\n{}\n",
        flag_crash_prompt.to_string_lossy(),
        script.trim_start_matches("#!/bin/sh\n")
    );
    let script_path = write_executable_script(temp.path(), "prompt_crash.sh", &wrapper);
    let cfg = AcpBackendConfig {
        command: script_path.to_string_lossy().into_owned(),
        args: Vec::new(),
        auth_method: None,
        cwd: temp.path().to_path_buf(),
    };

    let engine = InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), Some(cfg))
        .expect("open");
    let main = engine.ensure_main_thread().await.expect("main");

    // 1. Crashes during session/prompt -> submit_prompt fails or stream closes; dead driver invalidated.
    let err1 = engine
        .send_turn_for_thread(&main.thread_id, "first crashes")
        .await
        .expect_err("crash during prompt must fail");
    assert!(matches!(err1, EngineError::Transport(_)));

    // 2. Driver recovers and emits unsolicited DriverEvent::Error while session/prompt is open -> done_rx resolves early
    let err2 = timeout(
        Duration::from_secs(3),
        engine.send_turn_for_thread(&main.thread_id, "stream-err"),
    )
    .await
    .expect("timeout")
    .expect_err("stream error must fail turn");
    assert!(err2.to_string().contains("async stream crash"));

    // 3. Subsequent turn succeeds cleanly
    engine
        .send_turn_for_thread(&main.thread_id, "healthy turn")
        .await
        .expect("recovers cleanly");
    let transcript = engine
        .get_thread_transcript(&main.thread_id)
        .await
        .expect("transcript");
    assert_eq!(transcript.last().map(|t| t.content.as_str()), Some("recovered"));
}

#[tokio::test]
async fn test_acp_authenticate_failure_new_session_crash_and_config_guards() {
    let temp = tempfile::tempdir().expect("tempdir");

    // 1. Authenticate failure tears down driver
    let auth_fail_script = r#"#!/bin/sh
while IFS= read -r line; do
    rest=${line#*\"id\":}; id=${rest%%,*}; id=${id%%\}*}
    case "$line" in
        *'"method":"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"mock","version":"1.0"},"authMethods":[{"id":"oauth-personal","name":"OAuth","description":""}],"agentCapabilities":{}}}\n' "$id"
            ;;
        *'"method":"authenticate"'*)
            printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32000,"message":"bad token"}}\n' "$id"
            ;;
    esac
done
"#;
    let auth_script = write_executable_script(temp.path(), "auth_fail.sh", auth_fail_script);
    let auth_cfg = AcpBackendConfig {
        command: auth_script.to_string_lossy().into_owned(),
        args: Vec::new(),
        auth_method: Some("oauth-personal".into()),
        cwd: temp.path().to_path_buf(),
    };
    let auth_engine = InProcessEngine::open_with_config(
        &temp.path().join("auth.db"),
        temp.path().to_path_buf(),
        Some(auth_cfg),
    )
    .expect("open auth");
    let auth_main = auth_engine.ensure_main_thread().await.expect("main");
    let auth_err = auth_engine
        .send_turn_for_thread(&auth_main.thread_id, "hi")
        .await
        .expect_err("auth failure must fail");
    assert!(auth_err.to_string().contains("ACP authenticate"));

    // 2. new_session crash invalidates dead driver + unsupported custom config option
    let new_sess_flag = temp.path().join("crash_new_sess.flag");
    std::fs::write(&new_sess_flag, "1").expect("flag");
    let cfg_script_body = format!(
        r#"#!/bin/sh
FLAG={:?}
while IFS= read -r line; do
    rest=${{line#*\"id\":}}; id=${{rest%%,*}}; id=${{id%%\}}*}}
    case "$line" in
        *'"method":"initialize"'*)
            printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":1,"agentInfo":{{"name":"mock","version":"1.0"}},"authMethods":[],"agentCapabilities":{{}}}}}}\n' "$id"
            ;;
        *'"method":"session/new"'*)
            if [ -f "$FLAG" ]; then
                rm -f "$FLAG"
                exit 1
            fi
            printf '{{"jsonrpc":"2.0","id":%s,"result":{{"sessionId":"sess-cfg","models":{{"currentModelId":"m1","availableModels":[{{"modelId":"m1","name":"M1"}},{{"modelId":"m2","name":"M2"}}]}},"modes":{{"currentModeId":"code","availableModes":[{{"id":"code","name":"Code"}},{{"id":"plan","name":"Plan"}}]}}}}}}\n' "$id"
            ;;
        *'"method":"session/set_config_option"'*)
            printf '{{"jsonrpc":"2.0","id":%s,"error":{{"code":-32601,"message":"method not found"}}}}\n' "$id"
            ;;
        *'"method":"session/set_model"'*)
            printf '{{"jsonrpc":"2.0","id":%s,"result":null}}\n' "$id"
            ;;
        *'"method":"session/set_mode"'*)
            printf '{{"jsonrpc":"2.0","id":%s,"result":null}}\n' "$id"
            ;;
    esac
done
"#,
        new_sess_flag.to_string_lossy()
    );
    let cfg_script = write_executable_script(temp.path(), "cfg_fail.sh", &cfg_script_body);
    let cfg = AcpBackendConfig {
        command: cfg_script.to_string_lossy().into_owned(),
        args: Vec::new(),
        auth_method: None,
        cwd: temp.path().to_path_buf(),
    };
    let engine = InProcessEngine::open_with_config(
        &temp.path().join("cfg.db"),
        temp.path().to_path_buf(),
        Some(cfg),
    )
    .expect("open cfg");
    let main = engine.ensure_main_thread().await.expect("main");

    // First call crashes during session/new -> invalidates dead driver slot
    let new_err = engine
        .agent_options(&main.thread_id)
        .await
        .expect_err("new_session crash must fail");
    assert!(new_err.to_string().contains("new_session"));

    // Second call spawns fresh driver and returns options
    let opts = engine
        .agent_options(&main.thread_id)
        .await
        .expect("recovers after new_session crash");
    assert_eq!(opts.config_options.len(), 2);

    // Unsupported custom option returns Protocol error (-32600)
    let custom_err = engine
        .set_agent_config(&main.thread_id, "thought_level", "high", false)
        .await
        .expect_err("unsupported custom option must error");
    assert!(
        matches!(custom_err, EngineError::Protocol(ref m) if m.contains("not supported")),
        "expected unsupported Protocol error, got {custom_err:?}"
    );

    // Legacy fallback for model & mode succeeds when set_config_option returns -32601
    let m_snap = engine
        .set_agent_config(&main.thread_id, "model", "m2", true)
        .await
        .expect("legacy set_model fallback");
    assert!(m_snap
        .config_options
        .iter()
        .any(|o| o.id == "model" && o.current_value == "m2"));
    let mode_snap = engine
        .set_agent_config(&main.thread_id, "mode", "plan", false)
        .await
        .expect("legacy set_mode fallback");
    assert!(mode_snap
        .config_options
        .iter()
        .any(|o| o.id == "mode" && o.current_value == "plan"));

    // Guards on agent_options / set_agent_config
    assert!(matches!(
        engine.agent_options("bad-tid").await,
        Err(EngineError::NotFound(_))
    ));
    let term = engine
        .create_terminal(Some(&main.thread_id), Some("term"), None, 24, 80)
        .await
        .expect("term");
    let term_opts = engine.agent_options(&term.thread_id).await.expect("term opts");
    assert!(term_opts.config_options.is_empty());

    assert!(matches!(
        engine.set_agent_config(&main.thread_id, " ", "val", false).await,
        Err(EngineError::InvalidParams(_))
    ));
    assert!(matches!(
        engine.set_agent_config(&main.thread_id, "model", "   ", false).await,
        Err(EngineError::InvalidParams(_))
    ));
    assert!(matches!(
        engine.set_agent_config("bad-tid", "model", "m1", false).await,
        Err(EngineError::NotFound(_))
    ));
    assert!(matches!(
        engine.set_agent_config(&term.thread_id, "model", "m1", false).await,
        Err(EngineError::InvalidParams(m)) if m.contains("terminal")
    ));

    // Offline engine agent_options & set_agent_config guards
    let offline = InProcessEngine::in_memory().expect("offline");
    let off_main = offline.ensure_main_thread().await.expect("off_main");
    let off_opts = offline.agent_options(&off_main.thread_id).await.expect("off opts");
    assert!(off_opts.config_options.is_empty());
    assert!(matches!(
        offline.set_agent_config(&off_main.thread_id, "model", "m1", false).await,
        Err(EngineError::Transport(_))
    ));
}

#[tokio::test]
async fn test_worktree_guards_non_git_legacy_reuse_and_subdir_cleanup() {
    let temp = tempfile::tempdir().expect("tempdir");

    // 1. Non-git workspace error on ensure_thread_worktree
    let non_git = temp.path().join("plain_dir");
    std::fs::create_dir_all(&non_git).expect("mkdir non_git");
    let non_git_engine = InProcessEngine::open_with_config(
        &temp.path().join("nongit.db"),
        non_git,
        None,
    )
    .expect("open non_git");
    let ng_main = non_git_engine.ensure_main_thread().await.expect("main");
    let err_ng = non_git_engine
        .ensure_thread_worktree(&ng_main.thread_id)
        .await
        .expect_err("non-git workspace must reject ensure_thread_worktree");
    assert!(matches!(
        err_ng,
        EngineError::InvalidParams(ref m) if m.contains("not inside a git repository")
    ));

    // 2. Git repo: legacy `.grein/worktrees/<tid>` reuse + guards + subdir cleanup
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    init_git_repo(&repo);
    let canonical_repo = std::fs::canonicalize(&repo).expect("canonical");

    let engine = InProcessEngine::open_with_config(
        &temp.path().join("wt.db"),
        canonical_repo.clone(),
        None,
    )
    .expect("open");
    let main = engine.ensure_main_thread().await.expect("main");
    let branch = engine
        .fork_thread_with_title(Some(&main.thread_id), Some("legacy-branch"))
        .await
        .expect("fork");

    // Create a git worktree manually at legacy path `.grein/worktrees/<tid>`
    let legacy_dir = canonical_repo
        .join(".grein")
        .join("worktrees")
        .join(&branch.thread_id);
    std::fs::create_dir_all(legacy_dir.parent().unwrap()).expect("mkdir worktrees");
    assert!(Command::new("git")
        .args(["worktree", "add", "-b", "legacy-wt"])
        .arg(&legacy_dir)
        .current_dir(&canonical_repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .status()
        .expect("git worktree add")
        .success());
    let reused_path = engine
        .ensure_thread_worktree(&branch.thread_id)
        .await
        .expect("should reuse legacy worktree");
    assert_eq!(reused_path, std::fs::canonicalize(&legacy_dir).unwrap().to_string_lossy());

    // Create a subdir inside the worktree and attach a terminal thread whose cwd is that subdir
    let sub_dir = std::path::PathBuf::from(&reused_path).join("nested/sub");
    std::fs::create_dir_all(&sub_dir).expect("mkdir subdir");
    let term_sub = engine
        .create_terminal(
            Some(&branch.thread_id),
            Some("Subdir Term"),
            Some(&sub_dir.to_string_lossy()),
            24,
            80,
        )
        .await
        .expect("term in subdir");

    // Also create a plain file & unregistered directory for switch/remove guard tests
    let plain_file = temp.path().join("plain.txt");
    std::fs::write(&plain_file, "x").expect("write file");
    let unregistered_dir = temp.path().join("unregistered");
    std::fs::create_dir_all(&unregistered_dir).expect("mkdir unregistered");

    // ensure_thread_worktree unknown thread
    assert!(matches!(
        engine.ensure_thread_worktree("bad-tid").await,
        Err(EngineError::NotFound(_))
    ));

    // switch_thread_worktree guards
    assert!(matches!(
        engine.switch_thread_worktree(&branch.thread_id, "  ").await,
        Err(EngineError::InvalidParams(_))
    ));
    assert!(matches!(
        engine.switch_thread_worktree("bad-tid", &reused_path).await,
        Err(EngineError::NotFound(_))
    ));
    assert!(matches!(
        engine
            .switch_thread_worktree(&branch.thread_id, "/nonexistent/grein_missing_wt")
            .await,
        Err(EngineError::InvalidParams(m)) if m.contains("does not exist")
    ));
    assert!(matches!(
        engine
            .switch_thread_worktree(&branch.thread_id, &plain_file.to_string_lossy())
            .await,
        Err(EngineError::InvalidParams(m)) if m.contains("not a directory")
    ));
    assert!(matches!(
        engine
            .switch_thread_worktree(&branch.thread_id, &unregistered_dir.to_string_lossy())
            .await,
        Err(EngineError::InvalidParams(m)) if m.contains("not a registered worktree")
    ));

    // worktree_diff_preview empty path guard
    assert!(matches!(
        engine.worktree_diff_preview("   ").await,
        Err(EngineError::InvalidParams(_))
    ));

    // remove_worktree guards
    assert!(matches!(
        engine.remove_worktree("  ", false).await,
        Err(EngineError::InvalidParams(_))
    ));
    assert!(matches!(
        engine.remove_worktree("/nonexistent/grein_missing_wt", false).await,
        Err(EngineError::InvalidParams(m)) if m.contains("unknown or missing")
    ));
    assert!(matches!(
        engine.remove_worktree(&unregistered_dir.to_string_lossy(), false).await,
        Err(EngineError::InvalidParams(m)) if m.contains("not a registered worktree")
    ));
    assert!(matches!(
        engine.remove_worktree(&canonical_repo.to_string_lossy(), false).await,
        Err(EngineError::InvalidParams(m)) if m.contains("primary")
    ));

    // Remove worktree -> resets both agent thread & subdir terminal thread back to repo_root
    let mut rx = engine.subscribe_tui_events();
    engine
        .remove_worktree(&reused_path, true)
        .await
        .expect("remove worktree with subdir terminal");
    assert!(!std::path::Path::new(&reused_path).exists());

    let meta = engine.thread_metadata().await.expect("meta");
    let term_meta = meta
        .iter()
        .find(|m| m.thread_id == term_sub.thread_id)
        .expect("term meta");
    assert_eq!(
        term_meta.cwd.as_deref(),
        Some(canonical_repo.to_string_lossy().as_ref())
    );
    let _ = timeout(Duration::from_millis(100), rx.recv()).await;
}
