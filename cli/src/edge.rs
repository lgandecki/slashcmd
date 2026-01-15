use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::time::Duration;
use ureq::{Agent, AgentBuilder};

use crate::prompt::CommandResult;

const WORKER_URL: &str = "https://groq-warm-proxy.gozdak.workers.dev";
const HTTP_TIMEOUT_SECS: u64 = 30;

#[derive(Serialize)]
struct CommandRequest {
    query: String,
    style: String,
}

#[derive(Deserialize)]
struct ExplanationData {
    text: String,
}

/// SSE response containing command and explanation
pub struct EdgeResponse {
    pub command: CommandResult,
    pub explanation: Option<String>,
}

/// Edge proxy client - routes through Cloudflare Worker
pub struct EdgeClient {
    agent: Agent,
    jwt: String,
}

impl EdgeClient {
    /// Create a new edge client with a JWT token
    pub fn new(jwt: String) -> Self {
        let agent = AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout_read(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .build();

        Self { agent, jwt }
    }

    /// Create client with a test JWT (for development)
    pub fn with_test_jwt() -> Self {
        let jwt = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJzdWIiOiJ0ZXN0LXVzZXIiLCJ0aWVyIjoicHJvIiwiZXhwIjoxODAwMDAwMDAwfQ.".to_string();
        Self::new(jwt)
    }

    /// Query via edge proxy - returns command only (legacy compatibility)
    pub fn query(&self, user_query: &str) -> Result<CommandResult, String> {
        let response = self.query_with_explanation(user_query, "typescript")?;
        Ok(response.command)
    }

    /// Query via edge proxy with SSE - returns command and explanation
    pub fn query_with_explanation(&self, user_query: &str, style: &str) -> Result<EdgeResponse, String> {
        let request = CommandRequest {
            query: user_query.to_string(),
            style: style.to_string(),
        };

        let response = self
            .agent
            .post(&format!("{}/command", WORKER_URL))
            .set("Authorization", &format!("Bearer {}", self.jwt))
            .set("Content-Type", "application/json")
            .set("Accept", "text/event-stream")
            .send_json(&request)
            .map_err(|e| format!("Edge proxy error: {}", e))?;

        // Parse SSE response
        let reader = BufReader::new(response.into_reader());
        let mut command: Option<CommandResult> = None;
        let mut explanation: Option<String> = None;
        let mut current_event = String::new();

        for line in reader.lines() {
            let line = line.map_err(|e| format!("Read error: {}", e))?;

            if line.starts_with("event: ") {
                current_event = line[7..].to_string();
            } else if line.starts_with("data: ") {
                let data = &line[6..];

                match current_event.as_str() {
                    "command" => {
                        command = serde_json::from_str(data).ok();
                    }
                    "explanation" => {
                        if let Ok(exp_data) = serde_json::from_str::<ExplanationData>(data) {
                            explanation = Some(exp_data.text);
                        }
                    }
                    "done" => break,
                    "error" => {
                        return Err(format!("Server error: {}", data));
                    }
                    _ => {}
                }
            }
        }

        let command = command.ok_or_else(|| "No command received".to_string())?;

        Ok(EdgeResponse {
            command,
            explanation,
        })
    }

    /// Query via edge proxy with streaming - sends command and explanation through channels
    pub fn query_streaming(
        &self,
        user_query: &str,
        style: &str,
        cmd_tx: std::sync::mpsc::Sender<Result<CommandResult, String>>,
        exp_tx: std::sync::mpsc::Sender<Result<String, String>>,
    ) -> Result<(), String> {
        let request = CommandRequest {
            query: user_query.to_string(),
            style: style.to_string(),
        };

        let response = self
            .agent
            .post(&format!("{}/command", WORKER_URL))
            .set("Authorization", &format!("Bearer {}", self.jwt))
            .set("Content-Type", "application/json")
            .set("Accept", "text/event-stream")
            .send_json(&request)
            .map_err(|e| format!("Edge proxy error: {}", e))?;

        // Parse SSE response and send events through channels as they arrive
        let reader = BufReader::new(response.into_reader());
        let mut current_event = String::new();

        for line in reader.lines() {
            let line = line.map_err(|e| format!("Read error: {}", e))?;

            if line.starts_with("event: ") {
                current_event = line[7..].to_string();
            } else if line.starts_with("data: ") {
                let data = &line[6..];

                match current_event.as_str() {
                    "command" => {
                        let result: Result<CommandResult, String> = serde_json::from_str(data)
                            .map_err(|e| format!("Parse error: {}", e));
                        let _ = cmd_tx.send(result);
                    }
                    "explanation" => {
                        if let Ok(exp_data) = serde_json::from_str::<ExplanationData>(data) {
                            let _ = exp_tx.send(Ok(exp_data.text));
                        }
                    }
                    "done" => break,
                    "error" => {
                        let _ = cmd_tx.send(Err(format!("Server error: {}", data)));
                        break;
                    }
                    _ => {}
                }
            }
        }

        Ok(())
    }

    /// Ping the edge proxy to keep connection warm
    pub fn warmup(&self) -> Result<(), String> {
        self.agent
            .get(&format!("{}/ping", WORKER_URL))
            .call()
            .map_err(|e| format!("Edge warmup error: {}", e))?;
        Ok(())
    }
}
