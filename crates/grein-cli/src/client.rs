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
        self.stream.write_all(b"ping\n").await?;
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
