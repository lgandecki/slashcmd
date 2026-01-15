use serde::{Deserialize, Serialize};
use std::time::Duration;
use ureq::{Agent, AgentBuilder};

use crate::ipc::ExplainStyle;

const GEMINI_API_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models/gemini-3-flash-preview:generateContent";
const HTTP_TIMEOUT_SECS: u64 = 30;

#[derive(Serialize)]
struct GeminiRequest {
    contents: Vec<Content>,
    #[serde(rename = "generationConfig")]
    generation_config: GenerationConfig,
}

#[derive(Serialize)]
struct Content {
    parts: Vec<Part>,
}

#[derive(Serialize)]
struct Part {
    text: String,
}

#[derive(Serialize)]
struct GenerationConfig {
    temperature: f32,
    #[serde(rename = "maxOutputTokens")]
    max_output_tokens: u32,
}

#[derive(Deserialize)]
struct GeminiResponse {
    candidates: Option<Vec<Candidate>>,
}

#[derive(Deserialize)]
struct Candidate {
    content: CandidateContent,
}

#[derive(Deserialize)]
struct CandidateContent {
    parts: Vec<ResponsePart>,
}

#[derive(Deserialize)]
struct ResponsePart {
    text: String,
}

/// Gemini API client for command explanations
pub struct GeminiClient {
    agent: Agent,
    api_key: String,
}

impl GeminiClient {
    pub fn new(api_key: String) -> Self {
        let agent = AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout_read(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .build();

        Self { agent, api_key }
    }

    /// Explain a command with safety assessment
    pub fn explain(&self, command: &str, style: ExplainStyle) -> Result<String, String> {
        let prompt = build_explain_prompt(command, style);

        let request = GeminiRequest {
            contents: vec![Content {
                parts: vec![Part { text: prompt }],
            }],
            generation_config: GenerationConfig {
                temperature: 0.3,
                max_output_tokens: 500,
            },
        };

        let url = format!("{}?key={}", GEMINI_API_URL, self.api_key);

        let response = self
            .agent
            .post(&url)
            .set("Content-Type", "application/json")
            .send_json(&request)
            .map_err(|e| format!("Gemini HTTP error: {}", e))?;

        let gemini_response: GeminiResponse = response
            .into_json()
            .map_err(|e| format!("Gemini JSON parse error: {}", e))?;

        let text = gemini_response
            .candidates
            .and_then(|c| c.into_iter().next())
            .map(|c| {
                c.content
                    .parts
                    .into_iter()
                    .map(|p| p.text)
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();

        Ok(text.trim().to_string())
    }

    /// Warmup TLS connection
    pub fn warmup(&self) -> Result<(), String> {
        // Simple request to establish connection
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models?key={}",
            self.api_key
        );
        self.agent
            .get(&url)
            .call()
            .map_err(|e| format!("Gemini warmup error: {}", e))?;
        Ok(())
    }
}

fn build_explain_prompt(command: &str, style: ExplainStyle) -> String {
    let style_instruction = match style {
        ExplainStyle::Typescript => r#"Explain it as TypeScript-like pseudo-code. Use familiar programming constructs like:
- `for (const file of files)` for loops
- `if (condition)` for conditionals
- `pipe(output).to(nextCommand)` for pipes
- Use camelCase variable names"#,
        ExplainStyle::Python => r#"Explain it as Python-like pseudo-code. Use familiar programming constructs like:
- `for file in files:` for loops
- `if condition:` for conditionals
- Comments with `#`
- Use snake_case variable names"#,
        ExplainStyle::Ruby => r#"Explain it as Ruby-like pseudo-code. Use familiar programming constructs like:
- `files.each do |file|` for loops
- `if condition` / `end` blocks
- Use snake_case variable names"#,
        ExplainStyle::Human => r#"Explain it in plain English, step by step.
- Use simple, clear language
- Number each step
- Avoid jargon where possible"#,
    };

    format!(
        r#"Analyze this shell command for an experienced developer.

SAFETY LEVEL (be practical, not paranoid):

[SAFE] - Default for read-only operations:
- ls, find, grep, cat, head, tail, wc, du, df
- git status, git log, git diff
- docker ps, kubectl get
- Any command that only READS data

[CAUTION] - Only for commands with SIDE EFFECTS:
- Writes or modifies files (>, >>, tee, sed -i)
- Git commits, pushes
- Sends data over network (curl -X POST, wget --post)
- Installs packages
- Explicitly reads secret files (.env, credentials.json, ~/.ssh/*)

[DANGER] - Destructive/irreversible:
- rm, rm -rf (deletes files)
- DROP TABLE, DELETE FROM
- git push --force, git reset --hard
- Format/wipe operations

IMPORTANT: Assume the developer knows what they asked for.
- "find large files" showing file names is SAFE (that's the point)
- "list processes" showing process info is SAFE
- "show git history" is SAFE
- Only use CAUTION for actual side effects or explicit secret file access

{style_instruction}

Command: `{command}`

Format (keep pseudo-code to 3-6 lines):
[SAFETY_LEVEL] One brief sentence.
```
pseudo-code
```"#,
        style_instruction = style_instruction,
        command = command
    )
}
