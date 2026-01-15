//! Authentication module for slashcmd
//!
//! Handles login via browser flow, token storage, and status checking.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

const API_URL: &str = "https://groq-warm-proxy.gozdak.workers.dev";
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const POLL_TIMEOUT: Duration = Duration::from_secs(300); // 5 minutes

/// Stored authentication data
#[derive(Serialize, Deserialize, Debug)]
pub struct StoredAuth {
    pub token: String,
    pub user: String,
    pub github_id: String,
}

/// User status from API
#[derive(Deserialize, Debug)]
pub struct UserStatus {
    pub user: String,
    pub tier: String,
    pub usage: i32,
    pub limit: i32,
    pub remaining: i32,
}

/// Auth start response
#[derive(Deserialize)]
struct AuthStartResponse {
    session_id: String,
    auth_url: String,
}

/// Auth poll response
#[derive(Deserialize)]
struct AuthPollResponse {
    #[serde(default)]
    pending: bool,
    token: Option<String>,
    user: Option<String>,
    github_id: Option<String>,
    error: Option<String>,
}

/// Get the config directory for slashcmd
fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("slashcmd")
}

/// Get the auth file path
fn auth_file() -> PathBuf {
    config_dir().join("auth.json")
}

/// Load stored authentication
pub fn load_auth() -> Option<StoredAuth> {
    let path = auth_file();
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Save authentication to file
fn save_auth(auth: &StoredAuth) -> Result<(), String> {
    let dir = config_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create config dir: {}", e))?;

    let path = auth_file();
    let json = serde_json::to_string_pretty(auth).unwrap();
    fs::write(&path, json).map_err(|e| format!("Failed to save auth: {}", e))?;

    // Set restrictive permissions on the auth file (Unix only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = fs::set_permissions(&path, perms);
    }

    Ok(())
}

/// Delete stored authentication
pub fn delete_auth() {
    let path = auth_file();
    let _ = fs::remove_file(path);
}

/// Start the login flow
pub fn login() -> Result<(), String> {
    // Check if already logged in
    if let Some(auth) = load_auth() {
        println!("Already logged in as {}.", auth.user);
        println!("Use 'slashcmd logout' to sign out first.");
        return Ok(());
    }

    println!("Starting authentication...\n");

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .build();

    // Step 1: Start auth flow
    let start_resp: AuthStartResponse = agent
        .post(&format!("{}/auth/start", API_URL))
        .call()
        .map_err(|e| format!("Failed to start auth: {}", e))?
        .into_json()
        .map_err(|e| format!("Invalid response: {}", e))?;

    // Step 2: Open browser
    println!("Opening browser for authentication...");
    println!("If browser doesn't open, visit:");
    println!("  {}\n", start_resp.auth_url);

    // Try to open browser
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg(&start_resp.auth_url)
            .spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open")
            .arg(&start_resp.auth_url)
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/c", "start", &start_resp.auth_url])
            .spawn();
    }

    // Step 3: Poll for completion
    print!("Waiting for authentication");
    io::stdout().flush().ok();

    let start_time = std::time::Instant::now();
    loop {
        if start_time.elapsed() > POLL_TIMEOUT {
            println!("\n\nAuthentication timed out. Please try again.");
            return Err("Timeout".to_string());
        }

        std::thread::sleep(POLL_INTERVAL);
        print!(".");
        io::stdout().flush().ok();

        let poll_resp: AuthPollResponse = match agent
            .get(&format!("{}/auth/poll?session={}", API_URL, start_resp.session_id))
            .call()
        {
            Ok(resp) => resp.into_json().unwrap_or(AuthPollResponse {
                pending: true,
                token: None,
                user: None,
                github_id: None,
                error: None,
            }),
            Err(_) => continue, // Network error, keep polling
        };

        if let Some(error) = poll_resp.error {
            println!("\n\nAuthentication failed: {}", error);
            return Err(error);
        }

        if poll_resp.pending {
            continue;
        }

        // Auth complete!
        if let (Some(token), Some(user), Some(github_id)) =
            (poll_resp.token, poll_resp.user, poll_resp.github_id)
        {
            let auth = StoredAuth {
                token,
                user: user.clone(),
                github_id,
            };
            save_auth(&auth)?;

            println!("\n\n✓ Logged in as {}", user);
            println!("  Token saved to {:?}", auth_file());

            // Show usage status
            if let Ok(status) = get_status_with_auth(&auth) {
                println!(
                    "  Usage: {}/{} ({} tier)",
                    status.usage,
                    if status.limit < 0 { "∞".to_string() } else { status.limit.to_string() },
                    status.tier
                );
            }

            return Ok(());
        }
    }
}

/// Logout - delete stored credentials
pub fn logout() -> Result<(), String> {
    if load_auth().is_none() {
        println!("Not logged in.");
        return Ok(());
    }

    delete_auth();
    println!("Logged out successfully.");
    Ok(())
}

/// Get user status
pub fn status() -> Result<(), String> {
    let auth = load_auth().ok_or_else(|| {
        "Not logged in. Run 'slashcmd login' to authenticate.".to_string()
    })?;

    let status = get_status_with_auth(&auth)?;

    println!("User: {}", auth.user);
    println!("Tier: {}", status.tier);

    if status.tier == "pro" {
        println!("Usage: {} (unlimited)", status.usage);
    } else {
        println!("Usage: {}/{}", status.usage, status.limit);
        if status.remaining <= 10 && status.remaining > 0 {
            println!("\n⚠️  Only {} requests remaining!", status.remaining);
            println!("   Upgrade: https://slashcmd.lgandecki.net/upgrade");
        } else if status.remaining <= 0 {
            println!("\n❌ Free tier limit reached!");
            println!("   Upgrade: https://slashcmd.lgandecki.net/upgrade");
        }
    }

    Ok(())
}

/// Get status from API with given auth
fn get_status_with_auth(auth: &StoredAuth) -> Result<UserStatus, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(10))
        .build();

    let resp = agent
        .get(&format!("{}/status", API_URL))
        .set("Authorization", &format!("Bearer {}", auth.token))
        .call()
        .map_err(|e| format!("Failed to get status: {}", e))?;

    resp.into_json()
        .map_err(|e| format!("Invalid response: {}", e))
}

/// Get the stored token if available
pub fn get_token() -> Option<String> {
    load_auth().map(|a| a.token)
}
