use std::path::Path;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

pub struct DaemonClient {
    stream: UnixStream,
    next_id: u64,
}

impl DaemonClient {
    pub fn new(stream: UnixStream) -> Self {
        Self { stream, next_id: 1 }
    }

    pub async fn ping(&mut self) -> Result<String, std::io::Error> {
        self.send_command("ping").await
    }

    pub async fn send_turn(&mut self, prompt: &str) -> Result<String, std::io::Error> {
        self.send_command(&format!("send_turn:{prompt}")).await
    }

    pub async fn list_entries(&mut self) -> Result<String, std::io::Error> {
        self.send_command("list_entries").await
    }

    /// Sends a JSON-RPC 2.0 request and returns the full parsed response object.
    pub async fn json_rpc(&mut self, method: &str, params: Value) -> Result<Value, std::io::Error> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.stream
            .write_all(format!("{request}\n").as_bytes())
            .await?;
        self.stream.flush().await?;

        let mut collected: Vec<u8> = Vec::new();
        let mut buf = [0u8; 1024];
        while !collected.contains(&b'\n') {
            let n = self.stream.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            collected.extend_from_slice(&buf[..n]);
        }
        let text = String::from_utf8_lossy(&collected);
        serde_json::from_str(text.trim())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    pub async fn session_ping(&mut self) -> Result<Value, std::io::Error> {
        self.json_rpc("session.ping", json!({})).await
    }

    pub async fn session_list_threads(&mut self) -> Result<Value, std::io::Error> {
        self.json_rpc("session.list_threads", json!({})).await
    }

    async fn send_command(&mut self, cmd: &str) -> Result<String, std::io::Error> {
        self.stream.write_all(format!("{cmd}\n").as_bytes()).await?;
        self.stream.flush().await?;

        let mut buf = [0u8; 1024];
        let n = self.stream.read(&mut buf).await?;
        let response = String::from_utf8_lossy(&buf[..n]).trim().to_string();
        Ok(response)
    }
}

pub async fn connect_to_daemon<P: AsRef<Path>>(
    socket_path: P,
) -> Result<DaemonClient, std::io::Error> {
    let stream = UnixStream::connect(socket_path.as_ref()).await?;
    Ok(DaemonClient::new(stream))
}
