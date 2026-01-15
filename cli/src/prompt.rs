/// Build the prompt for the Groq API - returns JSON with command and safety
pub fn build_prompt(user_query: &str) -> String {
    format!(
        r#"You are a macOS CLI assistant. Convert the user's request to a shell command.

User request: "{}"

Return JSON with:
- "command": the shell command
- "safe": true if READ-ONLY (ls, find, grep, cat, ps, docker ps, git status), false if has SIDE EFFECTS (writes files, deletes, sends data, installs packages)

Examples:
{{"command": "find . -type f -size +100M", "safe": true}}
{{"command": "rm -rf *.tmp", "safe": false}}
{{"command": "git status", "safe": true}}
{{"command": "npm install", "safe": false}}

Respond with ONLY the JSON object, no markdown:"#,
        user_query
    )
}

use serde::Deserialize;

/// Result from Groq: command + safety assessment
#[derive(Debug, Clone, Deserialize)]
pub struct CommandResult {
    pub command: String,
    pub safe: bool,
}

/// Parse the JSON response from Groq
pub fn parse_response(response: &str) -> Result<CommandResult, String> {
    let s = response.trim();

    // Strip markdown if present
    let json_str = if s.starts_with("```") {
        s.trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim()
    } else {
        s
    };

    // Try to parse as JSON
    if let Ok(result) = serde_json::from_str::<CommandResult>(json_str) {
        return Ok(result);
    }

    // Fallback: extract command from plain text (backwards compatibility)
    let command = clean_response_legacy(response);
    Ok(CommandResult {
        command,
        safe: false, // Conservative default if JSON parsing fails
    })
}

/// Legacy cleanup for non-JSON responses
fn clean_response_legacy(response: &str) -> String {
    let mut s = response.trim().to_string();

    // Remove markdown code block prefixes
    for prefix in ["```bash\n", "```bash", "```sh\n", "```sh", "```\n", "```"] {
        let lower = s.to_lowercase();
        if lower.starts_with(prefix) {
            s = s[prefix.len()..].to_string();
            break;
        }
    }

    // Remove trailing code block
    if s.ends_with("\n```") {
        s = s[..s.len() - 4].to_string();
    } else if s.ends_with("```") {
        s = s[..s.len() - 3].to_string();
    }

    // Remove command prefixes
    let lower = s.to_lowercase();
    if lower.starts_with("command:") {
        s = s[8..].trim_start().to_string();
    } else if lower.starts_with("the command is:") {
        s = s[15..].trim_start().to_string();
    }

    s.trim().to_string()
}

/// Backwards-compatible function (returns just the command string)
pub fn clean_response(response: &str) -> String {
    parse_response(response)
        .map(|r| r.command)
        .unwrap_or_else(|_| response.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_markdown_bash() {
        assert_eq!(clean_response("```bash\nls -la\n```"), "ls -la");
    }

    #[test]
    fn test_clean_markdown_sh() {
        assert_eq!(clean_response("```sh\nfind . -name '*.rs'\n```"), "find . -name '*.rs'");
    }

    #[test]
    fn test_clean_command_prefix() {
        assert_eq!(clean_response("Command: ls -la"), "ls -la");
    }

    #[test]
    fn test_clean_the_command_is() {
        assert_eq!(clean_response("The command is: pwd"), "pwd");
    }

    #[test]
    fn test_clean_already_clean() {
        assert_eq!(clean_response("ls -la"), "ls -la");
    }

    #[test]
    fn test_build_prompt_contains_query() {
        let prompt = build_prompt("list files");
        assert!(prompt.contains("list files"));
        assert!(prompt.contains("macOS CLI assistant"));
    }
}
