use std::path::PathBuf;
use tempfile::TempDir;
use tokio::net::UnixStream;
use tokio::time::{Duration, sleep};

#[tokio::test]
async fn test_daemon_socket_lifecycle_and_client_connect() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let socket_path = temp_dir.path().join("grein.sock");

    // Verify client connection fails cleanly when daemon is not running
    let connect_err = grein_cli::client::connect_to_daemon(&socket_path).await;
    assert!(
        connect_err.is_err(),
        "client connection should fail when daemon is offline"
    );

    // Start daemon listening on the custom socket path
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let daemon_socket = socket_path.clone();
    let daemon_handle = tokio::spawn(async move {
        grein_cli::daemon::run_daemon_with_socket(daemon_socket, shutdown_rx)
            .await
            .expect("daemon exited with error");
    });

    // Poll until socket is bound
    let mut connected = false;
    for _ in 0..50 {
        if socket_path.exists() {
            if let Ok(mut client) = grein_cli::client::connect_to_daemon(&socket_path).await {
                // Test IPC ping/handshake
                let response = client.ping().await.expect("failed to send ping");
                assert_eq!(response, "pong");
                connected = true;
                break;
            }
        }
        sleep(Duration::from_millis(20)).await;
    }
    assert!(
        connected,
        "client failed to connect to daemon socket within timeout"
    );

    // Trigger clean daemon shutdown
    let _ = shutdown_tx.send(());
    let shutdown_res = tokio::time::timeout(Duration::from_secs(2), daemon_handle).await;
    assert!(shutdown_res.is_ok(), "daemon should gracefully shut down");

    // Verify socket file is cleaned up after shutdown
    assert!(
        !socket_path.exists(),
        "daemon socket file must be removed on clean shutdown"
    );
}

#[tokio::test]
async fn test_default_socket_path_resolution() {
    let path = grein_cli::daemon::default_socket_path();
    assert!(
        path.ends_with("grein/grein.sock") || path.ends_with(".local/share/grein/grein.sock"),
        "default socket path must target ~/.local/share/grein/grein.sock, got: {}",
        path.display()
    );
}
