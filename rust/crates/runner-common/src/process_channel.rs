// ProcessChannel mapping `ProcessChannel.cs`.
// Provides IPC between the listener and worker processes using pipes or streams.

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use std::path::PathBuf;

/// Message types for listener ↔ worker communication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum MessageType {
    NotInitialized = -1,
    NewJobRequest = 1,
    CancelRequest = 2,
    RunnerShutdown = 3,
    OperatingSystemShutdown = 4,
}

impl MessageType {
    /// Convert from an integer value.
    pub fn from_i32(value: i32) -> Self {
        match value {
            1 => MessageType::NewJobRequest,
            2 => MessageType::CancelRequest,
            3 => MessageType::RunnerShutdown,
            4 => MessageType::OperatingSystemShutdown,
            _ => MessageType::NotInitialized,
        }
    }
}

impl std::fmt::Display for MessageType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageType::NotInitialized => write!(f, "NotInitialized"),
            MessageType::NewJobRequest => write!(f, "NewJobRequest"),
            MessageType::CancelRequest => write!(f, "CancelRequest"),
            MessageType::RunnerShutdown => write!(f, "RunnerShutdown"),
            MessageType::OperatingSystemShutdown => write!(f, "OperatingSystemShutdown"),
        }
    }
}

/// A message exchanged between listener and worker.
#[derive(Debug, Clone)]
pub struct WorkerMessage {
    pub message_type: MessageType,
    pub body: String,
}

impl WorkerMessage {
    pub fn new(message_type: MessageType, body: impl Into<String>) -> Self {
        Self {
            message_type,
            body: body.into(),
        }
    }
}

/// IPC channel between the listener and worker processes.
///
/// On Unix this uses a Unix domain socket pair. The listener creates a socket
/// at a temp path and the worker connects to it.
///
/// The wire protocol is simple:
/// - 4 bytes: message type as little-endian i32
/// - 4 bytes: body length as little-endian u32
/// - N bytes: body as UTF-8 string
pub struct ProcessChannel {
    /// For the server side (listener), the socket path.
    socket_path: Option<PathBuf>,
    /// The connected stream for reading/writing messages.
    stream: Option<UnixStream>,
    /// The listener (only set on the server side before accepting).
    listener: Option<UnixListener>,
}

impl ProcessChannel {
    /// Create a new, uninitialized `ProcessChannel`.
    pub fn new() -> Self {
        Self {
            socket_path: None,
            stream: None,
            listener: None,
        }
    }

    /// Start the server side (used by the listener process).
    ///
    /// Creates a Unix domain socket at the given path. Returns the socket path
    /// that the worker process should connect to.
    pub fn start_server(&mut self, socket_dir: &std::path::Path) -> Result<String> {
        let socket_path = socket_dir.join(format!("runner_ipc_{}", uuid::Uuid::new_v4()));

        let listener = UnixListener::bind(&socket_path)
            .with_context(|| format!("Failed to bind Unix socket at {:?}", socket_path))?;

        let path_str = socket_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Socket path is not valid UTF-8"))?
            .to_string();

        self.socket_path = Some(socket_path);
        self.listener = Some(listener);

        Ok(path_str)
    }

    /// Accept a connection from the worker (server side).
    pub async fn accept(&mut self) -> Result<()> {
        let listener = self
            .listener
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Server not started; call start_server first"))?;

        let (stream, _addr) = listener
            .accept()
            .await
            .context("Failed to accept connection on IPC socket")?;

        self.stream = Some(stream);
        Ok(())
    }

    /// Accept a second connection from the worker (server side).
    /// Returns the raw stream without storing it (the first connection is kept).
    pub async fn accept_second(&mut self) -> Result<UnixStream> {
        let listener = self
            .listener
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Server not started; call start_server first"))?;

        let (stream, _addr) = listener
            .accept()
            .await
            .context("Failed to accept second connection on IPC socket")?;

        Ok(stream)
    }

    /// Start the client side (used by the worker process).
    ///
    /// Connects to the Unix domain socket at the given path.
    pub async fn start_client(&mut self, socket_path: &str) -> Result<()> {
        let stream = UnixStream::connect(socket_path)
            .await
            .with_context(|| format!("Failed to connect to IPC socket at {}", socket_path))?;

        self.stream = Some(stream);
        Ok(())
    }

    /// Send a message through the channel.
    pub async fn send_async(
        &mut self,
        message_type: MessageType,
        body: &str,
    ) -> Result<()> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Channel not connected"))?;

        // Write message type as i32 LE
        stream
            .write_all(&(message_type as i32).to_le_bytes())
            .await?;

        // Write body length as u32 LE
        let body_bytes = body.as_bytes();
        stream
            .write_all(&(body_bytes.len() as u32).to_le_bytes())
            .await?;

        // Write body
        stream.write_all(body_bytes).await?;
        stream.flush().await?;

        Ok(())
    }

    /// Receive a message from the channel.
    pub async fn receive_async(&mut self) -> Result<WorkerMessage> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Channel not connected"))?;

        // Read message type
        let mut type_buf = [0u8; 4];
        stream.read_exact(&mut type_buf).await?;
        let message_type = MessageType::from_i32(i32::from_le_bytes(type_buf));

        // Read body length
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let body_len = u32::from_le_bytes(len_buf) as usize;

        // Read body
        let mut body_buf = vec![0u8; body_len];
        stream.read_exact(&mut body_buf).await?;
        let body = String::from_utf8(body_buf)
            .context("IPC message body is not valid UTF-8")?;

        Ok(WorkerMessage::new(message_type, body))
    }
}

impl Default for ProcessChannel {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ProcessChannel {
    fn drop(&mut self) {
        // Clean up the socket file
        if let Some(ref path) = self.socket_path {
            let _ = std::fs::remove_file(path);
        }
    }
}
