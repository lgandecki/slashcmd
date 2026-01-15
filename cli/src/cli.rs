use std::process::Command;

use crate::edge::EdgeClient;
use crate::gemini::GeminiClient;
use crate::groq::GroqClient;
use crate::highlight::{dim, highlight_explanation};
use crate::ipc::{ExplainStyle, IpcClient, IpcRequest};
use crate::logs;
use crate::prompt::CommandResult;

/// Command source for CLI mode
pub enum CliSource {
    Direct { groq_api_key: String },
    Edge { token: Option<String> },
}

/// Run CLI mode - for non-interactive/piped usage
pub fn run_cli(
    query: String,
    groq_api_key: String,
    gemini_api_key: Option<String>,
    style: ExplainStyle,
    quick: bool,
) -> Result<(), String> {
    run_cli_impl(query, CliSource::Direct { groq_api_key }, gemini_api_key, style, quick)
}

/// Run CLI mode with edge proxy (test JWT)
pub fn run_cli_edge(
    query: String,
    gemini_api_key: Option<String>,
    style: ExplainStyle,
    quick: bool,
) -> Result<(), String> {
    run_cli_impl(query, CliSource::Edge { token: None }, gemini_api_key, style, quick)
}

/// Run CLI mode with edge proxy (authenticated)
pub fn run_cli_edge_auth(
    query: String,
    token: String,
    style: ExplainStyle,
    quick: bool,
) -> Result<(), String> {
    run_cli_impl(query, CliSource::Edge { token: Some(token) }, None, style, quick)
}

fn run_cli_impl(
    query: String,
    source: CliSource,
    gemini_api_key: Option<String>,
    style: ExplainStyle,
    quick: bool,
) -> Result<(), String> {
    // Get the command
    let command = match &source {
        CliSource::Direct { groq_api_key } => get_command(&query, groq_api_key)?,
        CliSource::Edge { token } => {
            let edge = match token {
                Some(t) => EdgeClient::new(t.clone()),
                None => EdgeClient::with_test_jwt(),
            };
            edge.query(&query)?.command
        }
    };

    // Print command
    println!("{}", command);

    // If quick mode, we're done
    if quick {
        return Ok(());
    }

    // Otherwise get and print explanation
    if let Some(ref gemini_key) = gemini_api_key {
        match get_explanation(&command, gemini_key, style) {
            Ok(explanation) => {
                println!();
                println!("{}", highlight_explanation(&explanation, style));
            }
            Err(e) => {
                eprintln!("\n{}", dim(&format!("(explanation unavailable: {})", e)));
            }
        }
    }

    // Save to log
    let entry = logs::create_entry(&query, &command, None, style);
    let _ = logs::save_log(&entry);

    // Spawn daemon in background for future requests (only for direct mode)
    if matches!(&source, CliSource::Direct { .. }) {
        spawn_daemon_background();
    }

    Ok(())
}

/// Get the CLI command from natural language
fn get_command(query: &str, groq_api_key: &str) -> Result<String, String> {
    // Try daemon first (fast path)
    if let Some(mut stream) = IpcClient::try_connect() {
        let request = IpcRequest::Command {
            query: query.to_string(),
        };
        return IpcClient::send_request(&mut stream, &request);
    }

    // Daemon not running - make direct HTTP request
    let groq = GroqClient::new(groq_api_key.to_string());
    let result = groq.query(query)?;

    // Spawn daemon in background for future requests
    spawn_daemon_background();

    Ok(result.command)
}

/// Get explanation for the command
fn get_explanation(
    command: &str,
    gemini_api_key: &str,
    style: ExplainStyle,
) -> Result<String, String> {
    // Try daemon first
    if let Some(mut stream) = IpcClient::try_connect() {
        let request = IpcRequest::Explain {
            command: command.to_string(),
            style,
        };
        return IpcClient::send_request(&mut stream, &request);
    }

    // Daemon not running - make direct HTTP request
    let gemini = GeminiClient::new(gemini_api_key.to_string());
    gemini.explain(command, style)
}

/// Spawn the daemon as a detached background process
fn spawn_daemon_background() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = Command::new(&exe)
            .arg("--daemon")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}
