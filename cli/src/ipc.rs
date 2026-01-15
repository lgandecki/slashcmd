use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};

pub const SOCKET_PATH: &str = "/tmp/cmd.sock";

/// Explanation style for command breakdown
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ExplainStyle {
    #[default]
    Typescript,
    Python,
    Ruby,
    Human,
}

impl std::str::FromStr for ExplainStyle {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "typescript" | "ts" => Ok(ExplainStyle::Typescript),
            "python" | "py" => Ok(ExplainStyle::Python),
            "ruby" | "rb" => Ok(ExplainStyle::Ruby),
            "human" | "plain" => Ok(ExplainStyle::Human),
            _ => Err(format!("Unknown style: {}. Use: typescript, python, ruby, human", s)),
        }
    }
}

/// Request types for IPC
#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IpcRequest {
    /// Get CLI command from natural language
    #[serde(rename = "command")]
    Command { query: String },

    /// Explain a command with safety assessment
    #[serde(rename = "explain")]
    Explain { command: String, style: ExplainStyle },
}

#[derive(Serialize, Deserialize)]
pub struct IpcResponse {
    pub success: bool,
    pub result: Option<String>,
    pub error: Option<String>,
}

/// Client-side IPC operations
pub struct IpcClient;

impl IpcClient {
    /// Try to connect to the daemon socket. Returns None if daemon isn't running.
    pub fn try_connect() -> Option<UnixStream> {
        UnixStream::connect(SOCKET_PATH).ok()
    }

    /// Send a request to the daemon and wait for response
    pub fn send_request(stream: &mut UnixStream, request: &IpcRequest) -> Result<String, String> {
        let mut json =
            serde_json::to_string(request).map_err(|e| format!("Serialize error: {}", e))?;
        json.push('\n');

        stream
            .write_all(json.as_bytes())
            .map_err(|e| format!("Write error: {}", e))?;
        stream
            .flush()
            .map_err(|e| format!("Flush error: {}", e))?;

        let mut reader = BufReader::new(stream);
        let mut response_line = String::new();
        reader
            .read_line(&mut response_line)
            .map_err(|e| format!("Read error: {}", e))?;

        let response: IpcResponse = serde_json::from_str(&response_line)
            .map_err(|e| format!("Parse error: {}", e))?;

        if response.success {
            Ok(response.result.unwrap_or_default())
        } else {
            Err(response.error.unwrap_or_else(|| "Unknown error".to_string()))
        }
    }
}

/// Server-side IPC operations
pub struct IpcServer {
    listener: UnixListener,
}

impl IpcServer {
    /// Create a new Unix socket server. Removes existing socket if present.
    pub fn new() -> Result<Self, String> {
        // Remove existing socket if present
        let _ = std::fs::remove_file(SOCKET_PATH);

        let listener =
            UnixListener::bind(SOCKET_PATH).map_err(|e| format!("Failed to bind socket: {}", e))?;

        // Set non-blocking for timeout handling in event loop
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("Failed to set non-blocking: {}", e))?;

        Ok(Self { listener })
    }

    /// Try to accept a connection. Returns None if no connection is pending.
    pub fn accept(&self) -> Option<UnixStream> {
        match self.listener.accept() {
            Ok((stream, _)) => Some(stream),
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => None,
            Err(_) => None,
        }
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        // Clean up socket file on shutdown
        let _ = std::fs::remove_file(SOCKET_PATH);
    }
}
