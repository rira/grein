use grein_acp::{
    JsonRpcMessage, JsonRpcRequest, JsonRpcResponse, StdioProcessConfig, StdioProcessRunner,
    StdioRunnerError,
};
use std::time::Duration;
use tokio::time::timeout;

#[tokio::test]
async fn test_subprocess_startup_and_handshake() {
    let config = StdioProcessConfig::new("cat", Vec::<String>::new());
    let runner = StdioProcessRunner::spawn(config)
        .await
        .expect("Subprocess should start successfully");

    assert!(
        runner.is_running(),
        "Process must report as actively running"
    );
    assert!(runner.pid().is_some(), "Process must have an allocated PID");

    runner
        .terminate(Duration::from_millis(500))
        .await
        .expect("Process should terminate cleanly");
}

#[tokio::test]
async fn test_message_multiplexing_bidirectional() {
    let config = StdioProcessConfig::new("cat", Vec::<String>::new());
    let runner = StdioProcessRunner::spawn(config)
        .await
        .expect("Subprocess should start successfully");

    let (mut client, mut incoming) = runner.into_channels();

    let request = JsonRpcMessage::Request(JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: serde_json::json!(1),
        method: "echo".into(),
        params: Some(serde_json::json!({"data": "ping"})),
    });

    client
        .send(request.clone())
        .await
        .expect("Message send should succeed");

    let response = timeout(Duration::from_secs(2), incoming.recv())
        .await
        .expect("Timed out waiting for response")
        .expect("Channel should return echoed message");

    assert_eq!(
        response, request,
        "Cat echo must multiplex identical JSON-RPC frame"
    );
}

#[tokio::test]
async fn test_error_propagation_on_non_zero_exit() {
    let config = StdioProcessConfig::new("false", Vec::<String>::new());
    let runner = StdioProcessRunner::spawn(config)
        .await
        .expect("Spawn should succeed initially before immediate exit");

    let (mut client, mut incoming) = runner.into_channels();

    let request = JsonRpcMessage::Request(JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: serde_json::json!(42),
        method: "test".into(),
        params: None,
    });

    let send_result = client.send(request).await;
    let recv_result = incoming.recv().await;

    // When the process terminates unexpectedly with non-zero status, either send or recv returns the exit error
    let has_error = send_result.is_err()
        || recv_result.is_none()
        || matches!(recv_result, Some(JsonRpcMessage::Error(_)));
    assert!(
        has_error,
        "Unexpected non-zero subprocess termination must propagate error"
    );
}

#[tokio::test]
async fn test_graceful_process_termination() {
    let config = StdioProcessConfig::new("cat", Vec::<String>::new());
    let mut runner = StdioProcessRunner::spawn(config)
        .await
        .expect("Subprocess should start successfully");

    assert!(runner.is_running());

    let exit_status = runner
        .terminate(Duration::from_millis(500))
        .await
        .expect("Termination must succeed without timing out");

    assert!(
        !runner.is_running(),
        "Process must not be running after termination"
    );
    assert!(
        exit_status.success(),
        "Gracefully terminated process should yield success status"
    );
}
