mod auth;
mod cli;
mod daemon;
mod edge;
mod gemini;
mod groq;
mod highlight;
mod ipc;
mod logs;
mod prompt;
mod tui;

use clap::{Parser, Subcommand};
use ipc::ExplainStyle;
use std::io::IsTerminal;
use std::process::Command;

#[derive(Parser)]
#[command(name = "slashcmd")]
#[command(about = "Natural language to shell commands")]
#[command(version)]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Run as background daemon (internal use)
    #[arg(long, hide = true, global = true)]
    daemon: bool,

    /// Skip the explanation (just show the command)
    #[arg(short = 'q', long, global = true)]
    quick: bool,

    /// Non-interactive mode (just print command, don't wait for input)
    #[arg(short = 'n', long, global = true)]
    non_interactive: bool,

    /// Print command only (for shell integration)
    #[arg(long, hide = true, global = true)]
    print_only: bool,

    /// Explanation style: typescript (default), python, ruby, human
    #[arg(short, long, default_value = "typescript", global = true)]
    style: String,

    /// Use local API keys instead of edge proxy (requires GROQ_API_KEY)
    #[arg(short, long, global = true)]
    local: bool,

    /// Natural language query (all remaining arguments joined)
    #[arg(trailing_var_arg = true)]
    query: Vec<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Login with GitHub via browser
    Login,
    /// Logout and clear stored credentials
    Logout,
    /// Show usage and tier status
    Status,
}

fn main() {
    let args = Args::parse();

    // Handle subcommands first
    if let Some(cmd) = &args.command {
        match cmd {
            Commands::Login => {
                if let Err(e) = auth::login() {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
                return;
            }
            Commands::Logout => {
                if let Err(e) = auth::logout() {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
                return;
            }
            Commands::Status => {
                if let Err(e) = auth::status() {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
                return;
            }
        }
    }

    // Local mode uses direct API calls (requires GROQ_API_KEY)
    if args.local {
        run_local_mode(&args);
        return;
    }

    // Default: Edge mode (uses proxy, requires login)
    run_edge_mode(&args);
}

/// Parse style keyword from first or last word of query
/// e.g., "human list files" → (ExplainStyle::Human, "list files")
/// e.g., "list files ts" → (ExplainStyle::Typescript, "list files")
fn parse_style_from_query(words: &[String], default: ExplainStyle) -> (String, ExplainStyle) {
    if words.is_empty() {
        return (String::new(), default);
    }

    let style_keywords = [
        ("human", ExplainStyle::Human),
        ("ruby", ExplainStyle::Ruby),
        ("ts", ExplainStyle::Typescript),
        ("typescript", ExplainStyle::Typescript),
        ("py", ExplainStyle::Python),
        ("python", ExplainStyle::Python),
    ];

    // Check first word
    let first = words[0].to_lowercase();
    for (keyword, style) in &style_keywords {
        if first == *keyword {
            let remaining = words[1..].join(" ");
            return (remaining, *style);
        }
    }

    // Check last word
    let last = words[words.len() - 1].to_lowercase();
    for (keyword, style) in &style_keywords {
        if last == *keyword {
            let remaining = words[..words.len() - 1].join(" ");
            return (remaining, *style);
        }
    }

    // No style keyword found, use default
    (words.join(" "), default)
}

fn print_usage() {
    eprintln!("Usage: slashcmd [OPTIONS] <your natural language request>");
    eprintln!("       slashcmd <COMMAND>");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  login    Login with GitHub via browser");
    eprintln!("  logout   Logout and clear stored credentials");
    eprintln!("  status   Show usage and tier status");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  -q, --quick           Skip explanation (just show command)");
    eprintln!("  -n, --non-interactive Don't wait for Enter, just print and exit");
    eprintln!("  -s, --style <STYLE>   Explanation style: typescript, python, ruby, human");
    eprintln!("  -l, --local           Use local API keys (requires GROQ_API_KEY)");
    eprintln!();
    eprintln!("Style keywords (first or last word):");
    eprintln!("  human, ruby, ts, py   Override explanation style inline");
    eprintln!();
    eprintln!("Examples:");
    eprintln!("  slashcmd login                       # Authenticate with GitHub");
    eprintln!("  slashcmd find five largest files     # TypeScript-style explanation");
    eprintln!("  slashcmd human list docker containers# Plain English explanation");
    eprintln!("  slashcmd -q list files               # Just the command, no explanation");
    eprintln!("  slashcmd status                      # Check usage (47/100 free tier)");
    eprintln!();
    eprintln!("Shell integration (add to .zshrc):");
    eprintln!("  /cmd() {{ slashcmd \"$@\" }}");
    eprintln!();
    eprintln!("Pricing:");
    eprintln!("  Free: 100 commands (lifetime)");
    eprintln!("  Pro:  $5/month unlimited - https://slashcmd.lgandecki.net/upgrade");
}

/// Run in local mode - uses direct API calls (requires GROQ_API_KEY)
fn run_local_mode(args: &Args) {
    // Get API keys from environment
    let groq_api_key = match std::env::var("GROQ_API_KEY") {
        Ok(key) if !key.is_empty() => key,
        _ => {
            eprintln!("Error: GROQ_API_KEY environment variable is not set");
            eprintln!("Hint: Remove --local flag to use the edge proxy instead");
            std::process::exit(1);
        }
    };

    let gemini_api_key = std::env::var("GEMINI_API_KEY").ok().filter(|k| !k.is_empty());

    if args.daemon {
        // Daemon mode - run background server
        if let Err(e) = daemon::run_daemon(groq_api_key, gemini_api_key) {
            eprintln!("Daemon error: {}", e);
            std::process::exit(1);
        }
        return;
    }

    // CLI mode - process user query
    if args.query.is_empty() {
        print_usage();
        std::process::exit(1);
    }

    // Parse style from -s flag as default
    let default_style: ExplainStyle = args.style.parse().unwrap_or_else(|e| {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    });

    // Check for style keywords in query (first or last word)
    let (query, style) = parse_style_from_query(&args.query, default_style);

    // Determine mode: interactive TUI vs non-interactive
    let is_tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let use_tui = is_tty && !args.non_interactive && !args.quick && !args.print_only;

    if use_tui {
        // Interactive TUI mode
        match tui::run_interactive(query, groq_api_key, gemini_api_key, style) {
            Ok(tui::TuiResult::Execute(command)) => {
                // Execute the command
                let status = Command::new("sh")
                    .arg("-c")
                    .arg(&command)
                    .status();

                match status {
                    Ok(s) => std::process::exit(s.code().unwrap_or(0)),
                    Err(e) => {
                        eprintln!("Failed to execute: {}", e);
                        std::process::exit(1);
                    }
                }
            }
            Ok(tui::TuiResult::Cancel) => {
                // User cancelled
                std::process::exit(130); // Standard Ctrl+C exit code
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        // Non-interactive mode (piped input, -q flag, or -n flag)
        if let Err(e) = cli::run_cli(query, groq_api_key, gemini_api_key, style, args.quick) {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}

/// Run in edge mode - uses Cloudflare Worker proxy (requires login)
fn run_edge_mode(args: &Args) {
    if args.query.is_empty() {
        print_usage();
        std::process::exit(1);
    }

    // Check for auth token
    let token = match auth::get_token() {
        Some(t) => t,
        None => {
            eprintln!("Not logged in. Please run 'slashcmd login' first.");
            eprintln!();
            eprintln!("Or use --local flag with GROQ_API_KEY for direct API access.");
            std::process::exit(1);
        }
    };

    // Parse style
    let default_style: ExplainStyle = args.style.parse().unwrap_or_else(|e| {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    });

    let (query, style) = parse_style_from_query(&args.query, default_style);

    // Determine mode
    let is_tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let use_tui = is_tty && !args.non_interactive && !args.quick && !args.print_only;

    if use_tui {
        // Interactive TUI mode with edge
        match tui::run_interactive_edge_auth(query, token, style) {
            Ok(tui::TuiResult::Execute(command)) => {
                let status = Command::new("sh")
                    .arg("-c")
                    .arg(&command)
                    .status();

                match status {
                    Ok(s) => std::process::exit(s.code().unwrap_or(0)),
                    Err(e) => {
                        eprintln!("Failed to execute: {}", e);
                        std::process::exit(1);
                    }
                }
            }
            Ok(tui::TuiResult::Cancel) => {
                std::process::exit(130);
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        // Non-interactive mode with edge
        if let Err(e) = cli::run_cli_edge_auth(query, token, style, args.quick) {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}
