use serde::{Deserialize, Serialize};
use std::time::Duration;
use ureq::{Agent, AgentBuilder};

use crate::prompt::{build_prompt, parse_response, CommandResult};

const GROQ_API_URL: &str = "https://api.groq.com/openai/v1/chat/completions";
const GROQ_MODELS_URL: &str = "https://api.groq.com/openai/v1/models";
const GROQ_MODEL: &str = "moonshotai/kimi-k2-instruct-0905";
const HTTP_TIMEOUT_SECS: u64 = 30;
const MAX_TOKENS: u32 = 500;
const TEMPERATURE: f32 = 0.3;

#[derive(Serialize)]
struct ChatRequest {
    messages: Vec<Message>,
    model: String,
    stream: bool,
    max_tokens: u32,
    temperature: f32,
}

#[derive(Serialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Deserialize)]
struct ResponseMessage {
    content: String,
}

/// Groq API client with connection pooling via ureq Agent
pub struct GroqClient {
    agent: Agent,
    api_key: String,
}

impl GroqClient {
    /// Create a new client. The Agent maintains a connection pool for keep-alive.
    pub fn new(api_key: String) -> Self {
        let agent = AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout_read(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .build();

        Self { agent, api_key }
    }

    /// Query Groq API with a natural language request, returns command + safety
    pub fn query(&self, user_query: &str) -> Result<CommandResult, String> {
        let request = ChatRequest {
            messages: vec![Message {
                role: "user".to_string(),
                content: build_prompt(user_query),
            }],
            model: GROQ_MODEL.to_string(),
            stream: false,
            max_tokens: MAX_TOKENS,
            temperature: TEMPERATURE,
        };

        let response = self
            .agent
            .post(GROQ_API_URL)
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .set("Content-Type", "application/json")
            .send_json(&request)
            .map_err(|e| format!("HTTP error: {}", e))?;

        let chat_response: ChatResponse = response
            .into_json()
            .map_err(|e| format!("JSON parse error: {}", e))?;

        let content = chat_response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();

        parse_response(&content)
    }

    /// Warm up the TLS connection by calling the free /models endpoint.
    /// This establishes the HTTPS connection without using any tokens.
    pub fn warmup(&self) -> Result<(), String> {
        self.agent
            .get(GROQ_MODELS_URL)
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .call()
            .map_err(|e| format!("Warmup error: {}", e))?;
        Ok(())
    }
}
