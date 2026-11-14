use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

pub struct DaemonClient {
    stream: UnixStream,
}

impl DaemonClient {
    pub fn new(stream: UnixStream) -> Self {
        Self { stream }
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
