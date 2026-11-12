use grein_acp::runner::{
    JsonRpcRequest, JsonRpcResponse, RunnerError, SubprocessConfig, SubprocessRunner,
};
use std::time::Duration;
use tokio::time::timeout;

#[tokio::test]
async fn test_subprocess_runner_lifecycle_and_duplex_correlation(
) -> Result<(), Box<dyn std::error::Error>> {
    // Configure subprocess runner executing an interactive line-based echo script
    let config = SubprocessConfig::new(
        "sh",
        vec![
            "-c".to_string(),
            "while IFS= read -r line; do echo \"$line\"; done".to_string(),
        ],
    );

    let mut runner = SubprocessRunner::spawn(config)
        .await
        .map_err(|e| format!("Failed to spawn subprocess runner: {:?}", e))?;

    assert!(
        runner.is_running().await,
        "Subprocess runner must be in running state after spawn"
    );

    // Send correlated JSON-RPC request 1
    let req1 = JsonRpcRequest::new("1", "ping", serde_json::json!({"seq": 1}));
    let response1_rx = runner
        .send_request(req1.clone())
        .await
        .map_err(|e| format!("Failed to send request 1: {:?}", e))?;

    let res1: JsonRpcResponse = timeout(Duration::from_secs(3), response1_rx)
        .await
        .map_err(|_| "Timeout awaiting response 1")?
        .map_err(|e| format!("Channel error for response 1: {:?}", e))?;

    assert_eq!(res1.id, "1", "Response ID must correlate with request ID 1");
    assert_eq!(res1.method, "ping");

    // Send correlated JSON-RPC request 2
    let req2 = JsonRpcRequest::new("2", "evaluate", serde_json::json!({"param": "value"}));
    let response2_rx = runner
        .send_request(req2.clone())
        .await
        .map_err(|e| format!("Failed to send request 2: {:?}", e))?;

    let res2: JsonRpcResponse = timeout(Duration::from_secs(3), response2_rx)
        .await
        .map_err(|_| "Timeout awaiting response 2")?
        .map_err(|e| format!("Channel error for response 2: {:?}", e))?;

    assert_eq!(res2.id, "2", "Response ID must correlate with request ID 2");
    assert_eq!(res2.method, "evaluate");

    // Verify graceful child process shutdown
    runner
        .shutdown()
        .await
        .map_err(|e| format!("Failed to gracefully shutdown subprocess: {:?}", e))?;

    assert!(
        !runner.is_running().await,
        "Subprocess must not be running after shutdown"
    );
    Ok(())
}

#[tokio::test]
async fn test_subprocess_runner_handles_invalid_process() {
    let config = SubprocessConfig::new("non_existent_binary_xyz_12345", vec![]);
    let result = SubprocessRunner::spawn(config).await;

    match result {
        Err(RunnerError::SpawnFailed(_)) => {
            // Expected red-phase domain error variant
        }
        Err(other) => panic!("Expected RunnerError::SpawnFailed, got: {:?}", other),
        Ok(_) => panic!("Spawning non-existent binary should fail"),
    }
}
