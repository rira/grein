use std::path::PathBuf;
use tokio::net::UnixListener;

#[allow(dead_code)]
pub fn default_socket_path() -> PathBuf {
    std::env::temp_dir().join("grein.sock")
}

pub async fn run_daemon_with_socket(
    socket_path: PathBuf,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let listener = UnixListener::bind(&socket_path)?;

    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((_stream, _addr)) => {}
                    Err(e) => {
                        eprintln!("Socket error: {e}");
                        break;
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                break;
            }
        }
    }

    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }

    Ok(())
}
