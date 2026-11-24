use std::sync::Arc;
use std::time::Duration;

use grein_acp::{AcpSpawnConfig, AgentDriverError, AgentDriverFactory, StdioAgentDriverFactory};
use tokio::time::timeout;

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct EnvRestoreGuard {
    key: String,
    previous: Option<String>,
}

impl EnvRestoreGuard {
    fn set(key: &str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self {
            key: key.to_string(),
            previous,
        }
    }
}

impl Drop for EnvRestoreGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(val) => std::env::set_var(&self.key, val),
            None => std::env::remove_var(&self.key),
        }
    }
}

#[tokio::test]
async fn early_exit_surfaces_exit_status_and_stderr_on_initialize() {
    let factory: Arc<dyn AgentDriverFactory> = Arc::new(StdioAgentDriverFactory::new());
    let cfg = AcpSpawnConfig::new(
        "/bin/sh",
        [
            "-c",
            "printf \"error: unknown option '--acp'\\n\" >&2; exit 1",
        ],
    );

    let driver = factory.spawn(&cfg).await.expect("spawn shell");
    let err = timeout(Duration::from_secs(3), driver.initialize())
        .await
        .expect("initialize must not hang when child exits early")
        .expect_err("initialize must fail on early exit");

    assert!(
        matches!(err, AgentDriverError::Terminated(_)),
        "expected Terminated error, got {err:?}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("agent process closed its output"),
        "expected base termination prefix in `{msg}`"
    );
    assert!(
        msg.contains("exit status 1"),
        "expected exit status 1 in `{msg}`"
    );
    assert!(
        msg.contains("error: unknown option '--acp'"),
        "expected stderr line in `{msg}`"
    );
    assert_eq!(driver.exit_status().as_deref(), Some("exit status 1"));
}

#[tokio::test]
async fn without_env_strips_parent_claudecode_variable_from_child() {
    let _lock = ENV_LOCK.lock().await;
    let _guard = EnvRestoreGuard::set("CLAUDECODE", "1");

    let script = r#"
while IFS= read -r line; do
    rest=${line#*\"id\":}
    id=${rest%%,*}
    id=${id%%\}*}
    case "$line" in
        *'"method":"initialize"'*)
            if [ -n "${CLAUDECODE+x}" ]; then
                val="$CLAUDECODE"
            else
                val="__UNSET__"
            fi
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"%s","version":"1.0"}}}\n' "$id" "$val"
            ;;
    esac
done
"#;

    let factory: Arc<dyn AgentDriverFactory> = Arc::new(StdioAgentDriverFactory::new());

    let inherited_cfg = AcpSpawnConfig::new("/bin/sh", ["-c", script]);
    let inherited_driver = factory
        .spawn(&inherited_cfg)
        .await
        .expect("spawn inherited");
    let inherited_init = timeout(Duration::from_secs(3), inherited_driver.initialize())
        .await
        .expect("init timeout")
        .expect("init succeeds");
    assert_eq!(inherited_init.agent_name.as_deref(), Some("1"));
    inherited_driver.shutdown().await.expect("shutdown");

    let stripped_cfg = AcpSpawnConfig::new("/bin/sh", ["-c", script]).without_env("CLAUDECODE");
    let stripped_driver = factory.spawn(&stripped_cfg).await.expect("spawn stripped");
    let stripped_init = timeout(Duration::from_secs(3), stripped_driver.initialize())
        .await
        .expect("init timeout")
        .expect("init succeeds");
    assert_eq!(stripped_init.agent_name.as_deref(), Some("__UNSET__"));
    stripped_driver.shutdown().await.expect("shutdown");

    let override_cfg = AcpSpawnConfig::new("/bin/sh", ["-c", script])
        .without_env("CLAUDECODE")
        .with_env([("CLAUDECODE".into(), "explicit-override".into())]);
    let override_driver = factory.spawn(&override_cfg).await.expect("spawn override");
    let override_init = timeout(Duration::from_secs(3), override_driver.initialize())
        .await
        .expect("init timeout")
        .expect("init succeeds");
    assert_eq!(
        override_init.agent_name.as_deref(),
        Some("explicit-override")
    );
    override_driver.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn invalid_utf8_on_stdout_and_stderr_does_not_break_initialize() {
    let script = r#"
printf '\377\376non-utf8-stdout-noise\n'
printf '\200\377non-utf8-stderr-line\n' >&2
printf 'valid-stderr-after-binary\n' >&2
while IFS= read -r line; do
    rest=${line#*\"id\":}
    id=${rest%%,*}
    id=${id%%\}*}
    case "$line" in
        *'"method":"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"utf8-resilient","version":"1.0"}}}\n' "$id"
            ;;
    esac
done
"#;

    let factory: Arc<dyn AgentDriverFactory> = Arc::new(StdioAgentDriverFactory::new());
    let cfg = AcpSpawnConfig::new("/bin/sh", ["-c", script]);
    let driver = factory.spawn(&cfg).await.expect("spawn");

    let init = timeout(Duration::from_secs(3), driver.initialize())
        .await
        .expect("initialize timeout")
        .expect("initialize must survive prior non-UTF-8 stdout and stderr bytes");
    assert_eq!(init.agent_name.as_deref(), Some("utf8-resilient"));

    // Give stderr task a brief moment to catch up if needed.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let stderr = driver.recent_stderr();
    assert!(
        stderr
            .iter()
            .any(|line| line.contains("valid-stderr-after-binary")),
        "stderr drain must continue past invalid UTF-8 bytes; got: {stderr:?}"
    );

    driver.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn rapid_subprocess_exit_at_initialize_never_hangs() {
    let factory: Arc<dyn AgentDriverFactory> = Arc::new(StdioAgentDriverFactory::new());

    timeout(Duration::from_secs(10), async {
        for i in 0..25 {
            let script = if i % 2 == 0 {
                "printf 'boom-fast\\n' >&2; exit 1"
            } else {
                "IFS= read -r _line; printf 'boom-after-read\\n' >&2; exit 2"
            };
            let cfg = AcpSpawnConfig::new("/bin/sh", ["-c", script]);
            let driver = factory.spawn(&cfg).await.expect("spawn");
            let res = timeout(Duration::from_secs(2), driver.initialize())
                .await
                .unwrap_or_else(|_| panic!("iteration {i} hung in initialize()"));
            let err = res.expect_err("rapid exit must fail initialize");
            let msg = err.to_string();
            assert!(
                msg.contains("agent process closed its output"),
                "iteration {i} unexpected error: {msg}"
            );
        }
    })
    .await
    .expect("rapid exit stress loop must complete well within timeout");
}

#[tokio::test]
async fn signal_exit_reports_unix_signal_summary() {
    let factory: Arc<dyn AgentDriverFactory> = Arc::new(StdioAgentDriverFactory::new());
    let cfg = AcpSpawnConfig::new("/bin/sh", ["-c", "kill -9 $$"]);
    let driver = factory.spawn(&cfg).await.expect("spawn");

    let err = timeout(Duration::from_secs(3), driver.initialize())
        .await
        .expect("initialize must not hang on signaled child")
        .expect_err("signaled child must fail initialize");
    let msg = err.to_string();
    assert!(
        msg.contains("signal 9"),
        "expected `signal 9` in error: {msg}"
    );
    assert_eq!(driver.exit_status().as_deref(), Some("signal 9"));
}

#[tokio::test]
async fn delayed_stderr_after_stdout_eof_is_captured_before_returning_error() {
    let factory: Arc<dyn AgentDriverFactory> = Arc::new(StdioAgentDriverFactory::new());
    let script = "exec >&-; sleep 0.04; printf 'delayed-fatal-stderr\\n' >&2; exit 2";
    let cfg = AcpSpawnConfig::new("/bin/sh", ["-c", script]);
    let driver = factory.spawn(&cfg).await.expect("spawn");

    let err = timeout(Duration::from_secs(3), driver.initialize())
        .await
        .expect("initialize must not hang")
        .expect_err("initialize must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("agent process closed its output"),
        "got: {msg}"
    );
    assert!(msg.contains("exit status 2"), "got: {msg}");
    assert!(msg.contains("delayed-fatal-stderr"), "got: {msg}");
    assert_eq!(driver.exit_status().as_deref(), Some("exit status 2"));
}

#[tokio::test]
async fn rapid_subprocess_exit_at_send_prompt_never_hangs() {
    let factory: Arc<dyn AgentDriverFactory> = Arc::new(StdioAgentDriverFactory::new());

    timeout(Duration::from_secs(10), async {
        for i in 0..20 {
            let cfg = AcpSpawnConfig::new("/bin/sh", ["-c", "printf 'prompt-boom\\n' >&2; exit 1"]);
            let driver = factory.spawn(&cfg).await.expect("spawn");
            match timeout(Duration::from_secs(2), driver.send_prompt("s1", "hello"))
                .await
                .unwrap_or_else(|_| panic!("iteration {i} hung in send_prompt()"))
            {
                Ok(mut rx) => {
                    let ev = timeout(Duration::from_secs(2), rx.recv())
                        .await
                        .unwrap_or_else(|_| panic!("iteration {i} hung waiting for error event"))
                        .expect("expected DriverEvent::Error");
                    assert!(matches!(ev, grein_acp::DriverEvent::Error { .. }));
                }
                Err(err) => {
                    assert!(err.to_string().contains("agent process closed its output"));
                }
            }
        }
    })
    .await
    .expect("rapid send_prompt exit stress loop must finish promptly");
}
