use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use super::{MAX_FRAME_BYTES, NetworkAddress, NetworkError, NetworkMessage, NetworkTransport, drain_length_prefixed};

/// Configuration for creating a TCP transport
#[derive(Debug, Clone)]
pub struct TcpTransportConfig {
    /// Server address to connect to
    pub server_addr: NetworkAddress,
    /// Connection timeout
    pub connect_timeout: Duration,
    /// Read timeout
    pub read_timeout: Duration,
}

impl TcpTransportConfig {
    /// Create a new TCP transport configuration with default timeouts
    pub fn new(server_addr: NetworkAddress) -> Self {
        Self {
            server_addr,
            connect_timeout: Duration::from_secs(5),
            read_timeout: Duration::from_secs(30),
        }
    }

    /// Create a new TCP transport configuration with custom timeouts
    pub fn with_timeouts(server_addr: NetworkAddress, connect_timeout: Duration, read_timeout: Duration) -> Self {
        Self {
            server_addr,
            connect_timeout,
            read_timeout,
        }
    }
}

/// TCP-based network transport
pub struct TcpTransport {
    stream: Option<TcpStream>,
    server_addr: NetworkAddress,
    connect_timeout: Duration,
    read_timeout: Duration,
    /// Bytes read from the socket that do not yet form a complete length-prefixed frame.
    /// Must persist across `receive_reliable` calls — a message that spans TCP segments
    /// would otherwise be half-consumed and dropped, desyncing the framing for good.
    rx_buf: Vec<u8>,
}

impl TcpTransport {
    pub fn new(server_addr: NetworkAddress, connect_timeout: Duration, read_timeout: Duration) -> Self {
        Self {
            stream: None,
            server_addr,
            connect_timeout,
            read_timeout,
            rx_buf: Vec::new(),
        }
    }

    fn ensure_stream_exists(&mut self) -> Result<(), NetworkError> {
        if self.stream.is_none() {
            self.connect()?;
        }
        Ok(())
    }

    fn get_tcp_addr(&self) -> Result<String, NetworkError> {
        match &self.server_addr {
            NetworkAddress::Tcp { host, port } => Ok(format!("{}:{}", host, port)),
            _ => Err(NetworkError::ConnectionFailed("Invalid address type for TcpTransport".to_string())),
        }
    }

    /// Closes the current TCP connection, if any.
    fn close_connection(&mut self) {
        // Buffered bytes belong to the old connection's frame stream — never carry them over.
        self.rx_buf.clear();
        if let Some(stream) = self.stream.take() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }

    /// Internal send implementation - does the actual I/O
    fn try_send(&mut self, payload: &[u8]) -> Result<(), NetworkError> {
        if let Some(ref mut stream) = self.stream {
            // Send message length first, then payload
            let len = payload.len() as u32;
            let len_bytes = len.to_be_bytes();

            stream
                .write_all(&len_bytes)
                .map_err(|e| NetworkError::SendFailed(format!("Failed to send length: {}", e)))?;

            stream
                .write_all(payload)
                .map_err(|e| NetworkError::SendFailed(format!("Failed to send payload: {}", e)))?;

            stream
                .flush()
                .map_err(|e| NetworkError::SendFailed(format!("Failed to flush: {}", e)))?;

            Ok(())
        } else {
            Err(NetworkError::SendFailed("No active connection".to_string()))
        }
    }
}

impl NetworkTransport for TcpTransport {
    fn connect(&mut self) -> Result<(), NetworkError> {
        tracing::debug!("TcpTransport connecting to {:?}", self.server_addr);
        // Close any existing connection gracefully
        self.close_connection();
        let addr = self.get_tcp_addr()?;

        match TcpStream::connect_timeout(
            &addr
                .parse()
                .map_err(|e| NetworkError::ConnectionFailed(format!("Invalid address: {}", e)))?,
            self.connect_timeout,
        ) {
            Ok(stream) => {
                // Set read timeout
                stream
                    .set_read_timeout(Some(self.read_timeout))
                    .map_err(|e| NetworkError::ConnectionFailed(format!("Failed to set timeout: {}", e)))?;

                self.stream = Some(stream);
                Ok(())
            }
            Err(e) => Err(NetworkError::ConnectionFailed(format!("TCP connect failed: {}", e))),
        }
    }

    fn send_reliable(&mut self, payload: &[u8]) -> Result<(), NetworkError> {
        self.ensure_stream_exists()?;

        // Try to send, and retry once after reconnect if the connection was stale
        match self.try_send(payload) {
            Ok(()) => Ok(()),
            Err(e) => {
                // Connection may have been closed by server (idle timeout, etc.)
                tracing::trace!("Send failed, attempting reconnect: {}", e);
                self.connect()?;
                self.try_send(payload)
            }
        }
    }

    fn send_unreliable(&mut self, _payload: &[u8]) -> Result<(), NetworkError> {
        unimplemented!("TCP transport does not support unreliable messaging")
    }

    fn receive_reliable(&mut self) -> Vec<NetworkMessage> {
        let mut messages = Vec::new();

        let Some(stream) = self.stream.as_mut() else {
            return messages;
        };

        tracing::debug!("TCP receive: checking for messages on connection");
        // Set non-blocking mode for receiving
        if let Err(e) = stream.set_nonblocking(true) {
            tracing::error!("Failed to set non-blocking mode: {}", e);
            return messages;
        }

        // Drain whatever the socket has into rx_buf; framing is decoded afterwards so a
        // message split across segments is completed on a later call instead of being lost.
        let mut chunk = [0u8; 8192];
        let mut closed = false;
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => {
                    tracing::debug!("TCP receive: peer closed the connection");
                    closed = true;
                    break;
                }
                Ok(n) => {
                    self.rx_buf.extend_from_slice(&chunk[..n]);
                    if self.rx_buf.len() > 2 * MAX_FRAME_BYTES {
                        tracing::warn!("TCP receive: buffered {} bytes without a complete frame, resetting", self.rx_buf.len());
                        self.rx_buf.clear();
                        closed = true;
                        break;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    tracing::debug!("TCP receive: no data available (would block)");
                    break;
                }
                Err(e) => {
                    tracing::debug!("TCP receive: read error: {}", e);
                    closed = true;
                    break;
                }
            }
        }

        // Restore blocking mode
        let _ = stream.set_nonblocking(false);

        match drain_length_prefixed(&mut self.rx_buf) {
            Ok(frames) => {
                for payload in frames {
                    messages.push(NetworkMessage {
                        source: self.server_addr.clone(),
                        payload,
                        timestamp: Instant::now(),
                    });
                }
            }
            Err(e) => {
                tracing::warn!("TCP receive: {} — dropping connection", e);
                self.close_connection();
                return messages;
            }
        }

        if closed {
            self.close_connection();
        }

        messages
    }

    fn receive_unreliable(&mut self) -> Vec<NetworkMessage> {
        unimplemented!("TCP transport does not support unreliable messaging")
    }

    /// Wait for a single response message with blocking read and timeout
    /// Used for request-response patterns where we expect a reply
    fn wait_for_response_reliable(&mut self) -> Result<NetworkMessage, NetworkError> {
        if let Some(ref mut stream) = self.stream {
            // Ensure blocking mode (should already be set from reconnect)
            stream
                .set_nonblocking(false)
                .map_err(|e| NetworkError::ReceiveFailed(format!("Failed to set blocking mode: {}", e)))?;

            // Read message length (blocking with timeout)
            let mut len_bytes = [0u8; 4];
            stream
                .read_exact(&mut len_bytes)
                .map_err(|e| NetworkError::ReceiveFailed(format!("Failed to read length header: {}", e)))?;

            let payload_len = u32::from_be_bytes(len_bytes) as usize;
            tracing::info!("Received message length: {} bytes", payload_len);

            // Reasonable message size limit
            if payload_len > 1024 * 1024 {
                return Err(NetworkError::ReceiveFailed(format!("Message too large: {} bytes", payload_len)));
            }

            // Read payload (blocking with timeout)
            let mut payload = vec![0u8; payload_len];
            stream
                .read_exact(&mut payload)
                .map_err(|e| NetworkError::ReceiveFailed(format!("Failed to read payload: {}", e)))?;

            Ok(NetworkMessage {
                source: self.server_addr.clone(),
                payload,
                timestamp: Instant::now(),
            })
        } else {
            Err(NetworkError::ReceiveFailed("No active connection".to_string()))
        }
    }
}
