use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ipc::ExplainStyle;

/// Log entry for a command execution
#[derive(Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: u64,
    pub query: String,
    pub command: String,
    pub explanation: Option<String>,
    pub style: String,
    pub executed: bool,
    pub exit_code: Option<i32>,
}

/// Get the logs directory path
pub fn logs_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".cmd").join("logs")
}

/// Ensure logs directory exists
pub fn ensure_logs_dir() -> std::io::Result<()> {
    fs::create_dir_all(logs_dir())
}

/// Save a log entry
pub fn save_log(entry: &LogEntry) -> std::io::Result<PathBuf> {
    ensure_logs_dir()?;

    // Filename: timestamp_first-few-words.json
    let query_slug: String = entry
        .query
        .split_whitespace()
        .take(3)
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .take(30)
        .collect();

    let filename = format!("{}_{}.json", entry.timestamp, query_slug);
    let path = logs_dir().join(&filename);

    let json = serde_json::to_string_pretty(entry)?;
    let mut file = fs::File::create(&path)?;
    file.write_all(json.as_bytes())?;

    Ok(path)
}

/// Get current unix timestamp
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Create a log entry
pub fn create_entry(
    query: &str,
    command: &str,
    explanation: Option<String>,
    style: ExplainStyle,
) -> LogEntry {
    LogEntry {
        timestamp: now(),
        query: query.to_string(),
        command: command.to_string(),
        explanation,
        style: format!("{:?}", style).to_lowercase(),
        executed: false,
        exit_code: None,
    }
}

/// List recent log entries
pub fn list_logs(limit: usize) -> std::io::Result<Vec<PathBuf>> {
    let dir = logs_dir();
    if !dir.exists() {
        return Ok(vec![]);
    }

    let mut entries: Vec<_> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "json").unwrap_or(false))
        .collect();

    // Sort by filename (which starts with timestamp) descending
    entries.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    entries.truncate(limit);

    Ok(entries)
}

/// Load a log entry from file
pub fn load_log(path: &PathBuf) -> std::io::Result<LogEntry> {
    let content = fs::read_to_string(path)?;
    serde_json::from_str(&content).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}
