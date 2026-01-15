//! Terminal UI with stable layout - command stays at bottom
//!
//! The command and prompt stay at a fixed position at the bottom.
//! Explanation appears ABOVE them without shifting.

use crossterm::{
    cursor::{MoveToColumn, MoveUp},
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal::{self, Clear, ClearType},
};
use std::io::{self, Write};
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::Duration;

use crate::edge::EdgeClient;
use crate::gemini::GeminiClient;
use crate::groq::GroqClient;
use crate::highlight::{format_safety, highlight};
use crate::ipc::{ExplainStyle, IpcClient, IpcRequest};
use crate::logs;
use crate::prompt::CommandResult;

pub enum TuiResult {
    Execute(String),
    Cancel,
}

/// Command source - either direct Groq API or edge proxy
pub enum CommandSource {
    Direct { groq_api_key: String },
    Edge { token: Option<String> },
}

pub fn run_interactive(
    query: String,
    groq_api_key: String,
    gemini_api_key: Option<String>,
    style: ExplainStyle,
) -> Result<TuiResult, String> {
    run_interactive_impl(query, CommandSource::Direct { groq_api_key }, gemini_api_key, style)
}

pub fn run_interactive_edge(
    query: String,
    gemini_api_key: Option<String>,
    style: ExplainStyle,
) -> Result<TuiResult, String> {
    run_interactive_impl(query, CommandSource::Edge { token: None }, gemini_api_key, style)
}

pub fn run_interactive_edge_auth(
    query: String,
    token: String,
    style: ExplainStyle,
) -> Result<TuiResult, String> {
    run_interactive_impl(query, CommandSource::Edge { token: Some(token) }, None, style)
}

fn run_interactive_impl(
    query: String,
    source: CommandSource,
    _gemini_api_key: Option<String>,
    style: ExplainStyle,
) -> Result<TuiResult, String> {
    // If user explicitly asked for explanation, always wait for confirmation
    let force_wait = query.to_lowercase().contains("explain");

    // Channels for command (both modes) and explanation (edge mode only initially)
    let (cmd_tx, cmd_rx) = mpsc::channel::<Result<CommandResult, String>>();

    let query_clone = query.clone();

    // Track if we're in edge mode and extract token
    let (is_edge_mode, edge_token) = match &source {
        CommandSource::Edge { token } => (true, token.clone()),
        _ => (false, None),
    };

    // For edge mode: create explanation channel upfront (SSE sends to it)
    // For direct mode: we'll create it later when spawning Gemini thread
    let edge_exp_rx = if is_edge_mode {
        let (exp_tx, exp_rx) = mpsc::channel::<Result<String, String>>();

        let style_str = match style {
            ExplainStyle::Typescript => "typescript",
            ExplainStyle::Python => "python",
            ExplainStyle::Ruby => "ruby",
            ExplainStyle::Human => "human",
        };
        let style_owned = style_str.to_string();
        let token_for_thread = edge_token.clone();

        thread::spawn(move || {
            let client = match token_for_thread {
                Some(t) => EdgeClient::new(t),
                None => EdgeClient::with_test_jwt(),
            };
            match client.query_streaming(&query_clone, &style_owned, cmd_tx, exp_tx) {
                Ok(_) => {}
                Err(e) => eprintln!("Edge stream error: {}", e),
            }
        });

        Some(exp_rx)
    } else {
        // Direct mode: spawn Groq call
        if let CommandSource::Direct { groq_api_key } = source {
            thread::spawn(move || {
                let _ = cmd_tx.send(get_command(&query_clone, &groq_api_key));
            });
        }
        None
    };

    let mut stdout = io::stdout();
    terminal::enable_raw_mode().map_err(|e| format!("Terminal error: {}", e))?;

    // Show loading
    execute!(
        stdout,
        MoveToColumn(0),
        Clear(ClearType::CurrentLine),
        SetForegroundColor(Color::DarkGrey),
        Print("Generating command..."),
        ResetColor,
    ).ok();
    stdout.flush().ok();

    // Wait for command + safety from Groq
    let cmd_result = match cmd_rx.recv_timeout(Duration::from_secs(30)) {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => {
            terminal::disable_raw_mode().ok();
            execute!(stdout, Print("\r\n")).ok();
            return Err(e);
        }
        Err(_) => {
            terminal::disable_raw_mode().ok();
            execute!(stdout, Print("\r\n")).ok();
            return Err("Timeout".to_string());
        }
    };

    let command = cmd_result.command;
    let is_safe = cmd_result.safe;

    // Auto-execute safe commands immediately (unless user asked to explain)
    if is_safe && !force_wait {
        execute!(
            stdout,
            MoveToColumn(0),
            Clear(ClearType::CurrentLine),
            SetForegroundColor(Color::Cyan),
            Print(&command),
            ResetColor,
            Print("\r\n"),
        ).ok();
        stdout.flush().ok();
        terminal::disable_raw_mode().ok();
        save_log(&query, &command, None, style);
        return Ok(TuiResult::Execute(command));
    }

    // Set up explanation channel
    // For edge mode: already have edge_exp_rx from SSE stream
    // For direct mode: spawn Gemini thread if we have API key
    let explanation_rx: Option<mpsc::Receiver<Result<String, String>>> = if is_edge_mode {
        edge_exp_rx
    } else if let Some(ref gemini_key) = _gemini_api_key {
        let (exp_tx, exp_rx) = mpsc::channel();
        let cmd = command.clone();
        let key = gemini_key.clone();
        let s = style;
        thread::spawn(move || {
            let _ = exp_tx.send(get_explanation(&cmd, &key, s));
        });
        Some(exp_rx)
    } else {
        None
    };

    let has_explanation = explanation_rx.is_some();

    // Pre-allocate space for explanation (only if we're fetching one)
    const RESERVED_LINES: u16 = 15;

    execute!(stdout, MoveToColumn(0), Clear(ClearType::CurrentLine)).ok();

    if has_explanation {
        // Print placeholder lines (dim dots to show space is reserved)
        for _ in 0..RESERVED_LINES {
            execute!(
                stdout,
                SetForegroundColor(Color::DarkGrey),
                Print("·"),
                ResetColor,
                Print("\r\n"),
            ).ok();
        }
        // Blank line before command
        execute!(stdout, Print("\r\n")).ok();
    }

    // Print command + prompt
    let loading_text = if has_explanation {
        "Loading explanation..."
    } else {
        "Press Enter to run, Ctrl+C to cancel... "
    };
    execute!(
        stdout,
        SetForegroundColor(Color::Cyan),
        Print(&command),
        ResetColor,
        Print("\r\n"),
        SetForegroundColor(Color::DarkGrey),
        Print(loading_text),
        ResetColor,
    ).ok();
    stdout.flush().ok();

    let mut explanation_text: Option<String> = None;
    let mut explanation_printed = false;

    loop {
        // Check for explanation (only for non-safe commands that need confirmation)
        if let Some(ref rx) = explanation_rx {
            if !explanation_printed {
                match rx.try_recv() {
                    Ok(Ok(exp)) => {
                        let is_danger = exp.contains("[DANGER]");
                        let formatted = format_explanation(&exp, style);
                        let exp_lines: Vec<&str> = formatted.lines().collect();

                        // Move cursor up to the reserved space
                        // (current position is after prompt, so go up: 1 prompt + 1 command + 1 blank + RESERVED_LINES)
                        let lines_to_go_up = 2 + 1 + RESERVED_LINES;
                        execute!(stdout, MoveUp(lines_to_go_up), MoveToColumn(0)).ok();

                        // Fill in explanation (overwrite placeholder lines)
                        for line in exp_lines.iter().take(RESERVED_LINES as usize) {
                            execute!(
                                stdout,
                                Clear(ClearType::CurrentLine),
                                Print(*line),
                                Print("\r\n"),
                            ).ok();
                        }

                        // Clear any remaining placeholder lines
                        for _ in exp_lines.len()..RESERVED_LINES as usize {
                            execute!(stdout, Clear(ClearType::CurrentLine), Print("\r\n")).ok();
                        }

                        // Skip blank line, move to command line
                        execute!(stdout, Print("\r\n")).ok();

                        // DANGER: Show command and wait for Enter to copy to clipboard
                        if is_danger {
                            execute!(
                                stdout,
                                Clear(ClearType::CurrentLine),
                                SetForegroundColor(Color::Red),
                                Print(&command),
                                ResetColor,
                                Print("\r\n"),
                                Clear(ClearType::CurrentLine),
                                SetForegroundColor(Color::Red),
                                Print("⚠️  DANGER: "),
                                ResetColor,
                                SetForegroundColor(Color::DarkGrey),
                                Print("Press Enter to copy to clipboard, Ctrl+C to cancel... "),
                                ResetColor,
                            ).ok();
                            stdout.flush().ok();

                            // Wait for Enter key
                            loop {
                                if let Ok(true) = event::poll(std::time::Duration::from_millis(100)) {
                                    if let Ok(Event::Key(key_event)) = event::read() {
                                        match key_event.code {
                                            KeyCode::Enter => {
                                                // Copy to clipboard (macOS)
                                                if let Ok(mut child) = std::process::Command::new("pbcopy")
                                                    .stdin(std::process::Stdio::piped())
                                                    .spawn()
                                                {
                                                    if let Some(stdin) = child.stdin.as_mut() {
                                                        let _ = stdin.write_all(command.as_bytes());
                                                    }
                                                    let _ = child.wait();
                                                }

                                                execute!(
                                                    stdout,
                                                    MoveToColumn(0),
                                                    Clear(ClearType::CurrentLine),
                                                    SetForegroundColor(Color::Red),
                                                    Print("⚠️  Copied to clipboard. Paste to run.\r\n"),
                                                    ResetColor,
                                                ).ok();
                                                stdout.flush().ok();
                                                break;
                                            }
                                            KeyCode::Char('c') if key_event.modifiers.contains(event::KeyModifiers::CONTROL) => {
                                                execute!(
                                                    stdout,
                                                    MoveToColumn(0),
                                                    Clear(ClearType::CurrentLine),
                                                    SetForegroundColor(Color::DarkGrey),
                                                    Print("Cancelled.\r\n"),
                                                    ResetColor,
                                                ).ok();
                                                stdout.flush().ok();
                                                break;
                                            }
                                            KeyCode::Esc => {
                                                execute!(
                                                    stdout,
                                                    MoveToColumn(0),
                                                    Clear(ClearType::CurrentLine),
                                                    SetForegroundColor(Color::DarkGrey),
                                                    Print("Cancelled.\r\n"),
                                                    ResetColor,
                                                ).ok();
                                                stdout.flush().ok();
                                                break;
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            }

                            terminal::disable_raw_mode().ok();
                            save_log(&query, &command, Some(exp), style);
                            return Ok(TuiResult::Cancel);
                        }

                        // CAUTION: Show command and wait for confirmation
                        execute!(
                            stdout,
                            Clear(ClearType::CurrentLine),
                            SetForegroundColor(Color::Cyan),
                            Print(&command),
                            ResetColor,
                            Print("\r\n"),
                            Clear(ClearType::CurrentLine),
                            SetForegroundColor(Color::DarkGrey),
                            Print("Press Enter to run, Ctrl+C to cancel... "),
                            ResetColor,
                        ).ok();
                        stdout.flush().ok();

                        explanation_text = Some(exp);
                        explanation_printed = true;
                    }
                    Ok(Err(_)) => {
                        // Explanation failed - clear placeholder and show simple prompt
                        let lines_to_go_up = 2 + 1 + RESERVED_LINES;
                        execute!(stdout, MoveUp(lines_to_go_up), MoveToColumn(0)).ok();
                        for _ in 0..RESERVED_LINES {
                            execute!(stdout, Clear(ClearType::CurrentLine), Print("\r\n")).ok();
                        }
                        execute!(
                            stdout,
                            Print("\r\n"),
                            Clear(ClearType::CurrentLine),
                            SetForegroundColor(Color::Cyan),
                            Print(&command),
                            ResetColor,
                            Print("\r\n"),
                            Clear(ClearType::CurrentLine),
                            SetForegroundColor(Color::DarkGrey),
                            Print("Press Enter to run, Ctrl+C to cancel... "),
                            ResetColor,
                        ).ok();
                        stdout.flush().ok();
                        explanation_printed = true;
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => {
                        explanation_printed = true;
                    }
                }
            }
        }

        // Poll for keys
        if event::poll(Duration::from_millis(100)).unwrap_or(false) {
            if let Ok(Event::Key(key_event)) = event::read() {
                match key_event {
                    KeyEvent { code: KeyCode::Enter, .. } => {
                        terminal::disable_raw_mode().ok();
                        execute!(stdout, Print("\r\n")).ok();
                        save_log(&query, &command, explanation_text, style);
                        return Ok(TuiResult::Execute(command));
                    }
                    KeyEvent { code: KeyCode::Char('c'), modifiers: KeyModifiers::CONTROL, .. } |
                    KeyEvent { code: KeyCode::Esc, .. } => {
                        terminal::disable_raw_mode().ok();
                        execute!(stdout, Print("\r\n")).ok();
                        return Ok(TuiResult::Cancel);
                    }
                    _ => {}
                }
            }
        }
    }
}

fn format_explanation(exp: &str, style: ExplainStyle) -> String {
    let mut result = String::new();
    let mut in_code_block = false;
    let mut code_buffer = String::new();

    for line in exp.lines() {
        if line.starts_with("```") {
            if in_code_block {
                result.push_str(&highlight(&code_buffer, style));
                code_buffer.clear();
            }
            in_code_block = !in_code_block;
        } else if in_code_block {
            code_buffer.push_str(line);
            code_buffer.push('\n');
        } else {
            let cleaned = line
                .replace("**[SAFE]**", "[SAFE]")
                .replace("**[CAUTION]**", "[CAUTION]")
                .replace("**[DANGER]**", "[DANGER]");
            result.push_str(&format_safety(&cleaned));
            result.push('\n');
        }
    }
    result.trim_end().to_string()
}

fn get_command(query: &str, api_key: &str) -> Result<CommandResult, String> {
    if let Some(mut s) = IpcClient::try_connect() {
        let cmd = IpcClient::send_request(&mut s, &IpcRequest::Command { query: query.into() })?;
        // Daemon returns just command string for now, assume safe=false (conservative)
        return Ok(CommandResult { command: cmd, safe: false });
    }
    GroqClient::new(api_key.into()).query(query)
}

fn get_explanation(cmd: &str, api_key: &str, style: ExplainStyle) -> Result<String, String> {
    if let Some(mut s) = IpcClient::try_connect() {
        return IpcClient::send_request(&mut s, &IpcRequest::Explain { command: cmd.into(), style });
    }
    GeminiClient::new(api_key.into()).explain(cmd, style)
}

fn save_log(query: &str, command: &str, explanation: Option<String>, style: ExplainStyle) {
    let entry = logs::create_entry(query, command, explanation, style);
    let _ = logs::save_log(&entry);
}

/// Get command via edge proxy
fn get_command_edge(query: &str) -> Result<CommandResult, String> {
    EdgeClient::with_test_jwt().query(query)
}

/// Get command and explanation via edge proxy (SSE)
fn get_command_and_explanation_edge(query: &str, style: ExplainStyle) -> Result<(CommandResult, Option<String>), String> {
    let style_str = match style {
        ExplainStyle::Typescript => "typescript",
        ExplainStyle::Python => "python",
        ExplainStyle::Ruby => "ruby",
        ExplainStyle::Human => "human",
    };
    let response = EdgeClient::with_test_jwt().query_with_explanation(query, style_str)?;
    Ok((response.command, response.explanation))
}
