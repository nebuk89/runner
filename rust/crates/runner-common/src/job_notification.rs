// JobNotification mapping `JobNotification.cs`.
// Socket-based notification channel between runner and the supervisor/monitor.

use std::io::Write;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::Mutex;
use uuid::Uuid;

/// Provides TCP socket-based notifications to an external monitor process.
///
/// The monitor (e.g. a systemd-based supervisor) listens on a local TCP socket
/// and the runner sends `Start`/`End` messages to coordinate job lifecycle.
///
/// Maps `JobNotification` in the C# runner.
pub struct JobNotification {
    monitor_socket: Mutex<Option<TcpStream>>,
    is_monitor_configured: bool,
}

impl JobNotification {
    /// Create a new, unconfigured `JobNotification`.
    pub fn new() -> Self {
        Self {
            monitor_socket: Mutex::new(None),
            is_monitor_configured: false,
        }
    }

    /// Connect to the monitor at the given address (format: "host:port").
    pub fn start_client(&mut self, monitor_socket_address: &str) {
        if self.is_monitor_configured || monitor_socket_address.is_empty() {
            return;
        }

        let parts: Vec<&str> = monitor_socket_address.split(':').collect();
        if parts.len() != 2 {
            tracing::error!(
                "Invalid socket address {}. Unable to connect to monitor.",
                monitor_socket_address
            );
            return;
        }

        let address: Ipv4Addr = match parts[0].parse() {
            Ok(addr) => addr,
            Err(e) => {
                tracing::error!(
                    "Invalid socket IP address {}. Unable to connect to monitor: {}",
                    parts[0],
                    e
                );
                return;
            }
        };

        let port: u16 = match parts[1].parse() {
            Ok(p) => p,
            Err(_) => {
                tracing::error!(
                    "Invalid TCP socket port {}. Unable to connect to monitor.",
                    parts[1]
                );
                return;
            }
        };

        let socket_addr = SocketAddr::new(address.into(), port);

        match TcpStream::connect(socket_addr) {
            Ok(stream) => {
                tracing::info!("Connection successful to local port {}", port);
                *self.monitor_socket.lock().unwrap() = Some(stream);
                self.is_monitor_configured = true;
            }
            Err(e) => {
                tracing::error!("Connection to monitor port {} failed: {}", port, e);
            }
        }
    }

    /// Notify the monitor that a job has started.
    pub fn job_started(&self, job_id: Uuid, access_token: &str, server_url: &str) {
        tracing::info!("Entering JobStarted Notification");

        if access_token.is_empty() {
            tracing::info!("No access token could be retrieved to start the monitor.");
            return;
        }

        if !self.is_monitor_configured {
            return;
        }

        let message = format!(
            "Start {} {} {} {}",
            job_id,
            access_token,
            server_url,
            std::process::id()
        );

        self.send_message(&message);
    }

    /// Notify the monitor that a job has completed.
    pub async fn job_completed(&self, _job_id: Uuid) {
        tracing::info!("Entering JobCompleted Notification");

        if !self.is_monitor_configured {
            return;
        }

        let message = format!("End {}", std::process::id());
        self.send_message(&message);

        // Brief delay to allow the monitor to process the message
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }

    /// Send a message to the monitor socket.
    fn send_message(&self, message: &str) {
        if let Ok(mut guard) = self.monitor_socket.lock() {
            if let Some(ref mut stream) = *guard {
                match stream.write_all(message.as_bytes()) {
                    Ok(_) => {
                        tracing::info!("Successfully sent message to monitor");
                    }
                    Err(e) => {
                        tracing::error!("Failed sending message on socket: {}", e);
                    }
                }
            }
        }
    }
}

impl Default for JobNotification {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for JobNotification {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.monitor_socket.lock() {
            if let Some(ref mut stream) = *guard {
                let _ = stream.write_all(b"<EOF>");
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
            *guard = None;
        }
    }
}
