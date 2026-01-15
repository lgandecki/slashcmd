use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::edge::EdgeClient;
use crate::gemini::GeminiClient;
use crate::groq::GroqClient;
use crate::ipc::{IpcRequest, IpcResponse, IpcServer, SOCKET_PATH};

/// Daemon idle timeout in seconds (5 minutes)
const DAEMON_IDLE_TIMEOUT_SECS: u64 = 300;

/// Keep-alive interval in seconds (refresh TLS connection before it times out)
const KEEP_ALIVE_INTERVAL_SECS: u64 = 30;

/// Lazy-initialized Gemini client (warmed up on first explain request)
struct LazyGemini {
    client: Option<GeminiClient>,
    api_key: Option<String>,
    warmed_up: bool,
}

impl LazyGemini {
    fn new(api_key: Option<String>) -> Self {
        Self {
            client: None,
            api_key,
            warmed_up: false,
        }
    }

    fn get_or_init(&mut self) -> Result<&GeminiClient, String> {
        if self.client.is_none() {
            let api_key = self.api_key.clone().ok_or_else(|| {
                "GEMINI_API_KEY not set. Set it to enable command explanations.".to_string()
            })?;
            self.client = Some(GeminiClient::new(api_key));
        }

        let client = self.client.as_ref().unwrap();

        // Warmup on first use
        if !self.warmed_up {
            eprintln!("Warming up Gemini TLS connection...");
            if let Err(e) = client.warmup() {
                eprintln!("Gemini warmup warning: {}", e);
            } else {
                eprintln!("Gemini connection ready");
            }
            self.warmed_up = true;
        }

        Ok(client)
    }
}

/// Run the background daemon that maintains warm connections
pub fn run_daemon(groq_api_key: String, gemini_api_key: Option<String>) -> Result<(), String> {
    eprintln!("Starting cmd daemon...");

    let server = IpcServer::new()?;
    let groq = Arc::new(GroqClient::new(groq_api_key));
    let gemini = Arc::new(Mutex::new(LazyGemini::new(gemini_api_key)));
    let start = Instant::now();
    let last_activity = Arc::new(AtomicU64::new(0));
    let shutdown = Arc::new(AtomicBool::new(false));

    // Warmup Groq TLS connection immediately (free /models call)
    eprintln!("Warming up Groq TLS connection...");
    if let Err(e) = groq.warmup() {
        eprintln!("Warning: Groq warmup failed: {}", e);
    } else {
        eprintln!("Groq connection ready");
    }

    // Spawn keep-alive thread for Groq (every 30 seconds)
    let groq_keepalive = Arc::clone(&groq);
    let shutdown_keepalive = Arc::clone(&shutdown);
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(KEEP_ALIVE_INTERVAL_SECS));

            if shutdown_keepalive.load(Ordering::Relaxed) {
                break;
            }

            if let Err(e) = groq_keepalive.warmup() {
                eprintln!("Groq keep-alive failed: {}", e);
            }
        }
    });

    // Spawn keep-alive thread for Edge proxy (keeps Worker + Groq connections warm)
    let shutdown_edge = Arc::clone(&shutdown);
    thread::spawn(move || {
        let edge = EdgeClient::with_test_jwt();
        // Initial warmup
        if let Err(e) = edge.warmup() {
            eprintln!("Edge warmup failed: {}", e);
        } else {
            eprintln!("Edge proxy connection ready");
        }

        loop {
            thread::sleep(Duration::from_secs(KEEP_ALIVE_INTERVAL_SECS));

            if shutdown_edge.load(Ordering::Relaxed) {
                break;
            }

            if let Err(e) = edge.warmup() {
                eprintln!("Edge keep-alive failed: {}", e);
            }
        }
    });

    eprintln!("Daemon listening on {}", SOCKET_PATH);

    loop {
        // Check for idle timeout
        let elapsed = start.elapsed().as_secs();
        let last = last_activity.load(Ordering::Relaxed);
        if elapsed > 0 && elapsed - last > DAEMON_IDLE_TIMEOUT_SECS {
            eprintln!(
                "Daemon idle timeout ({} seconds), shutting down",
                DAEMON_IDLE_TIMEOUT_SECS
            );
            shutdown.store(true, Ordering::Relaxed);
            break;
        }

        // Poll for connections (non-blocking)
        if let Some(mut stream) = server.accept() {
            // Update activity timestamp
            last_activity.store(start.elapsed().as_secs(), Ordering::Relaxed);

            // Handle request and send response
            let response = handle_request(&mut stream, &groq, &gemini);
            send_response(&mut stream, &response);
        }

        // Small sleep to avoid busy-waiting (10ms = 100 polls/sec)
        thread::sleep(Duration::from_millis(10));
    }

    Ok(())
}

fn handle_request(
    stream: &mut UnixStream,
    groq: &GroqClient,
    gemini: &Arc<Mutex<LazyGemini>>,
) -> IpcResponse {
    let mut reader = BufReader::new(&*stream);
    let mut line = String::new();

    if reader.read_line(&mut line).is_err() {
        return IpcResponse {
            success: false,
            result: None,
            error: Some("Failed to read request".to_string()),
        };
    }

    let request: IpcRequest = match serde_json::from_str(&line) {
        Ok(r) => r,
        Err(e) => {
            return IpcResponse {
                success: false,
                result: None,
                error: Some(format!("Invalid request: {}", e)),
            }
        }
    };

    match request {
        IpcRequest::Command { query } => match groq.query(&query) {
            Ok(cmd_result) => IpcResponse {
                success: true,
                result: Some(cmd_result.command), // For now, daemon returns just command
                error: None,
            },
            Err(e) => IpcResponse {
                success: false,
                result: None,
                error: Some(e),
            },
        },
        IpcRequest::Explain { command, style } => {
            let mut gemini_guard = gemini.lock().unwrap();
            match gemini_guard.get_or_init() {
                Ok(client) => match client.explain(&command, style) {
                    Ok(result) => IpcResponse {
                        success: true,
                        result: Some(result),
                        error: None,
                    },
                    Err(e) => IpcResponse {
                        success: false,
                        result: None,
                        error: Some(e),
                    },
                },
                Err(e) => IpcResponse {
                    success: false,
                    result: None,
                    error: Some(e),
                },
            }
        }
    }
}

fn send_response(stream: &mut UnixStream, response: &IpcResponse) {
    let mut json = serde_json::to_string(response)
        .unwrap_or_else(|_| r#"{"success":false,"error":"Serialize error"}"#.to_string());
    json.push('\n');
    let _ = stream.write_all(json.as_bytes());
    let _ = stream.flush();
}
