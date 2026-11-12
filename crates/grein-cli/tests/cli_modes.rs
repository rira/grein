use std::path::Path;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::timeout;

#[tokio::test]
async fn test_standalone_runs_embedded_daemon_in_process_without_socket() {
    // Standalone mode must run the embedded daemon service as an in-process Tokio task.
    // Invariant: Zero external socket dependencies (no Unix socket file created).
    let socket_path = Path::new("/tmp/grein-test-nonexistent.sock");
    assert!(
        !socket_path.exists(),
        "Precondition: socket path must not exist"
    );

    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    // Launch standalone session with an explicit cancellation token/channel
    let standalone_handle =
        tokio::spawn(async move { grein_cli::standalone::run_standalone(shutdown_rx).await });

    // Yield control to ensure in-process daemon task initializes
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Verify no socket file was created on the filesystem
    assert!(
        !socket_path.exists(),
        "Standalone mode must not create or bind an external Unix domain socket"
    );

    // Signal graceful shutdown to the embedded in-process daemon
    shutdown_tx
        .send(())
        .expect("Failed to send shutdown signal to standalone runner");

    // Verify that the standalone task completes cleanly within timeout
    let result = timeout(Duration::from_secs(2), standalone_handle).await;
    assert!(result.is_ok(), "Standalone task timed out during shutdown");

    let run_result = result.expect("Join failed").expect("Task panicked");
    assert!(
        run_result.is_ok(),
        "Standalone runner returned an error: {:?}",
        run_result.err()
    );
}

#[tokio::test]
async fn test_standalone_embedded_daemon_client_interaction() {
    // Verify that the embedded in-process daemon accepts commands/queries without IPC socket.
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    let (client_handle, server_result) = tokio::join!(
        async {
            // Allow embedded server to spin up
            tokio::time::sleep(Duration::from_millis(30)).await;
            let client = grein_cli::standalone::create_in_process_client().await;
            assert!(client.is_ok(), "Failed to acquire in-process client handle");
            let mut client = client.unwrap();
            let ping_res = client.ping().await;
            assert!(ping_res.is_ok(), "In-process ping must succeed");
            let _ = shutdown_tx.send(());
        },
        async { grein_cli::standalone::run_standalone(shutdown_rx).await }
    );

    assert!(
        server_result.is_ok(),
        "Embedded daemon service exited with error"
    );
}
