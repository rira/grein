use std::io::Read;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn grein_cmd() -> Command {
    if let Ok(bin) = std::env::var("CARGO_BIN_EXE_grein") {
        Command::new(bin)
    } else {
        Command::new(env!("CARGO_BIN_EXE_grein"))
    }
}

struct ProcessGuard(Child);

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn test_cli_help_displays_subcommands() {
    let output = grein_cmd()
        .arg("--help")
        .output()
        .expect("failed to execute grein --help");

    assert!(
        output.status.success(),
        "grein --help must exit successfully"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("standalone") || stdout.contains("Standalone"),
        "Help output must list standalone mode: {stdout}"
    );
    assert!(
        stdout.contains("daemon") || stdout.contains("Daemon"),
        "Help output must list daemon mode: {stdout}"
    );
    assert!(
        stdout.contains("attach") || stdout.contains("Attach"),
        "Help output must list attach mode: {stdout}"
    );
}

#[test]
fn test_cli_version_output() {
    let output = grein_cmd()
        .arg("--version")
        .output()
        .expect("failed to execute grein --version");

    assert!(
        output.status.success(),
        "grein --version must exit successfully"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("grein") || stdout.contains(env!("CARGO_PKG_VERSION")),
        "Version output must contain binary name or package version: {stdout}"
    );
}

#[test]
fn test_daemon_mode_binds_socket_cleanly() {
    let temp_dir = std::env::temp_dir();
    let unique_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let socket_path: PathBuf = temp_dir.join(format!(
        "grein-test-{}-{}.sock",
        std::process::id(),
        unique_id
    ));

    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }

    let child = grein_cmd()
        .arg("daemon")
        .arg("--socket")
        .arg(&socket_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn grein in daemon mode");

    let mut guard = ProcessGuard(child);

    let start = Instant::now();
    let timeout = Duration::from_secs(5);
    let mut bound = false;

    while start.elapsed() < timeout {
        if socket_path.exists() {
            match UnixStream::connect(&socket_path) {
                Ok(_) => {
                    bound = true;
                    break;
                }
                Err(_) => {
                    // Socket file created but listener not yet fully accepting
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        } else {
            if let Ok(Some(status)) = guard.0.try_wait() {
                let mut stderr = String::new();
                if let Some(mut err) = guard.0.stderr.take() {
                    let _ = err.read_to_string(&mut stderr);
                }
                panic!("Daemon process exited prematurely with status {status}: {stderr}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    assert!(
        bound,
        "Daemon must bind and accept connections on the specified socket within {:?}: {}",
        timeout,
        socket_path.display()
    );

    // Clean up socket file
    let _ = std::fs::remove_file(&socket_path);
}
