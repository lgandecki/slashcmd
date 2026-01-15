/// Simple ANSI syntax highlighting for pseudo-code
/// Keeps binary small - no heavy dependencies like syntect

// ANSI color codes
const RESET: &str = "\x1b[0m";
const KEYWORD: &str = "\x1b[38;5;198m";    // Pink/magenta for keywords
const STRING: &str = "\x1b[38;5;114m";     // Green for strings
const COMMENT: &str = "\x1b[38;5;245m";    // Gray for comments
const FUNCTION: &str = "\x1b[38;5;81m";    // Cyan for functions
const NUMBER: &str = "\x1b[38;5;208m";     // Orange for numbers
const TYPE: &str = "\x1b[38;5;81m";        // Cyan for types
const DIM: &str = "\x1b[2m";               // Dim for less important

/// TypeScript keywords
const TS_KEYWORDS: &[&str] = &[
    "const", "let", "var", "function", "return", "if", "else", "for", "while",
    "of", "in", "async", "await", "import", "export", "from", "class", "new",
    "try", "catch", "throw", "true", "false", "null", "undefined",
];

/// Python keywords
const PY_KEYWORDS: &[&str] = &[
    "def", "return", "if", "else", "elif", "for", "while", "in", "import",
    "from", "class", "try", "except", "raise", "True", "False", "None",
    "with", "as", "pass", "break", "continue", "and", "or", "not",
];

/// Ruby keywords
const RB_KEYWORDS: &[&str] = &[
    "def", "end", "return", "if", "else", "elsif", "unless", "case", "when",
    "for", "while", "do", "class", "module", "begin", "rescue", "raise",
    "true", "false", "nil", "require", "include", "attr_accessor",
];

use crate::ipc::ExplainStyle;

/// Highlight code based on style
pub fn highlight(code: &str, style: ExplainStyle) -> String {
    match style {
        ExplainStyle::Typescript => highlight_typescript(code),
        ExplainStyle::Python => highlight_python(code),
        ExplainStyle::Ruby => highlight_ruby(code),
        ExplainStyle::Human => code.to_string(), // No highlighting for human
    }
}

fn highlight_typescript(code: &str) -> String {
    let mut result = String::new();

    for line in code.lines() {
        let highlighted = highlight_line(line, TS_KEYWORDS, "//");
        result.push_str(&highlighted);
        result.push('\n');
    }

    result.trim_end().to_string()
}

fn highlight_python(code: &str) -> String {
    let mut result = String::new();

    for line in code.lines() {
        let highlighted = highlight_line(line, PY_KEYWORDS, "#");
        result.push_str(&highlighted);
        result.push('\n');
    }

    result.trim_end().to_string()
}

fn highlight_ruby(code: &str) -> String {
    let mut result = String::new();

    for line in code.lines() {
        let highlighted = highlight_line(line, RB_KEYWORDS, "#");
        result.push_str(&highlighted);
        result.push('\n');
    }

    result.trim_end().to_string()
}

fn highlight_line(line: &str, keywords: &[&str], comment_prefix: &str) -> String {
    // Handle full-line comments
    let trimmed = line.trim_start();
    if trimmed.starts_with(comment_prefix) {
        return format!("{}{}{}", COMMENT, line, RESET);
    }

    let mut result = String::new();
    let mut chars = line.chars().peekable();
    let mut current_word = String::new();

    while let Some(c) = chars.next() {
        if c.is_alphanumeric() || c == '_' {
            current_word.push(c);
        } else {
            // Flush current word
            if !current_word.is_empty() {
                result.push_str(&colorize_word(&current_word, keywords));
                current_word.clear();
            }

            // Handle strings
            if c == '"' || c == '\'' {
                result.push_str(STRING);
                result.push(c);
                let quote = c;
                while let Some(sc) = chars.next() {
                    result.push(sc);
                    if sc == quote {
                        break;
                    }
                }
                result.push_str(RESET);
            }
            // Handle inline comments
            else if c == '/' && chars.peek() == Some(&'/') {
                result.push_str(COMMENT);
                result.push(c);
                for remaining in chars.by_ref() {
                    result.push(remaining);
                }
                result.push_str(RESET);
            }
            else if c == '#' && comment_prefix == "#" {
                result.push_str(COMMENT);
                result.push(c);
                for remaining in chars.by_ref() {
                    result.push(remaining);
                }
                result.push_str(RESET);
            }
            else {
                result.push(c);
            }
        }
    }

    // Flush remaining word
    if !current_word.is_empty() {
        result.push_str(&colorize_word(&current_word, keywords));
    }

    result
}

fn colorize_word(word: &str, keywords: &[&str]) -> String {
    // Keywords
    if keywords.contains(&word) {
        return format!("{}{}{}", KEYWORD, word, RESET);
    }

    // Numbers
    if word.chars().all(|c| c.is_ascii_digit()) {
        return format!("{}{}{}", NUMBER, word, RESET);
    }

    // Function calls (word followed by paren - handled by context)
    // For simplicity, color camelCase/snake_case that look like functions
    if word.contains('(') || word.ends_with("()") {
        return format!("{}{}{}", FUNCTION, word, RESET);
    }

    word.to_string()
}

/// Format safety level with color
pub fn format_safety(text: &str) -> String {
    if text.starts_with("[SAFE]") {
        text.replacen("[SAFE]", "\x1b[32m[SAFE]\x1b[0m", 1)
    } else if text.starts_with("[CAUTION]") {
        text.replacen("[CAUTION]", "\x1b[33m[CAUTION]\x1b[0m", 1)
    } else if text.starts_with("[DANGER]") {
        text.replacen("[DANGER]", "\x1b[31m[DANGER]\x1b[0m", 1)
    } else {
        text.to_string()
    }
}

/// Highlight the full explanation (safety line + code block)
pub fn highlight_explanation(explanation: &str, style: ExplainStyle) -> String {
    let mut result = String::new();
    let mut in_code_block = false;
    let mut code_buffer = String::new();

    for line in explanation.lines() {
        if line.starts_with("```") {
            if in_code_block {
                // End of code block - highlight and add
                let highlighted = highlight(&code_buffer, style);
                result.push_str(&highlighted);
                result.push('\n');
                code_buffer.clear();
            }
            in_code_block = !in_code_block;
            // Skip the ``` lines themselves
        } else if in_code_block {
            code_buffer.push_str(line);
            code_buffer.push('\n');
        } else {
            // Regular text - format safety if present
            result.push_str(&format_safety(line));
            result.push('\n');
        }
    }

    result.trim_end().to_string()
}

/// Dim text for secondary information
pub fn dim(text: &str) -> String {
    format!("{}{}{}", DIM, text, RESET)
}

/// Bold cyan for commands
pub fn command_style(text: &str) -> String {
    format!("\x1b[1;36m{}\x1b[0m", text)
}
