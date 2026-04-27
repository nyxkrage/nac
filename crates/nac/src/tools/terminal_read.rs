use serde_json::Value;

use crate::terminal::TerminalManager;
use crate::tools::{require_str, ToolResult, ToolRuntime};

/// Strip ANSI escape sequences from text
fn strip_ansi_codes(text: &str) -> String {
    // Regex to match ANSI escape sequences: \x1b[...m or \x1b[...H etc.
    // Matches \x1b followed by [ and any sequence of numbers and semicolons, ending with a letter
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // Check if next char is '[' (CSI sequence)
            if chars.peek() == Some(&'[') {
                chars.next(); // consume '['
                // Skip until we find a letter (end of sequence) or @, `, {, |, }, ~ (some valid terminators)
                while let Some(&next_ch) = chars.peek() {
                    chars.next();
                    if next_ch.is_ascii_alphabetic() || matches!(next_ch, '@' | '`' | '{' | '|' | '}' | '~') {
                        break;
                    }
                }
            }
            // Otherwise skip just the escape character (malformed sequence)
        } else {
            result.push(ch);
        }
    }
    
    result
}

/// Get the JSON schema definition for the terminal_read tool
pub fn definition() -> crate::types::ToolDefinition {
    use serde_json::json;

    crate::types::ToolDefinition {
        def_type: "function".to_string(),
        function: crate::types::FunctionDef {
            name: "terminal_read".to_string(),
            description: "Read the current content of a terminal session. Returns the visible screen content as plain text with ANSI codes stripped.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Terminal name"
                    },
                    "lines": {
                        "type": "integer",
                        "description": "Number of lines to read (default: terminal height)"
                    },
                    "offset_from_bottom": {
                        "type": "integer",
                        "description": "Lines from bottom to skip (default: 0)"
                    }
                },
                "required": ["name"]
            }),
        },
    }
}

/// Execute the terminal_read tool
pub async fn execute(
    args: Value,
    _runtime: &ToolRuntime,
    manager: &TerminalManager,
) -> ToolResult {
    // Extract required name
    let name = match require_str(&args, "name") {
        Ok(n) => n,
        Err(e) => return e,
    };

    // Check if terminal exists
    if !manager.contains(&name).await {
        return ToolResult {
            content: format!("Error: Terminal session '{}' does not exist", name),
            is_error: true,
        };
    }

    // Extract optional parameters with defaults
    let offset_from_bottom = args
        .get("offset_from_bottom")
        .and_then(|v| v.as_u64())
        .map(|v| v as u16)
        .unwrap_or(0);

    // Check if terminal is in alternate screen mode
    let is_alt_screen = match manager.is_alternate_screen(&name).await {
        Ok(Some(is_alt)) => is_alt,
        Ok(None) => {
            return ToolResult {
                content: format!("Error: Terminal session '{}' not found", name),
                is_error: true,
            };
        }
        Err(e) => {
            return ToolResult {
                content: format!("Error: Failed to check terminal screen mode: {}", e),
                is_error: true,
            };
        }
    };

    // If in alternate screen mode and trying to access scrollback, return error
    if is_alt_screen && offset_from_bottom > 0 {
        return ToolResult {
            content: "Error: Terminal is in alternate screen mode (e.g., vim, less). Scrollback is not available. Use offset_from_bottom=0 to read only the visible screen.".to_string(),
            is_error: true,
        };
    }

    // Get terminal size for defaults
    let size = match manager.get_terminal_size(&name).await {
        Ok(Some((cols, rows))) => (cols, rows),
        Ok(None) => {
            return ToolResult {
                content: format!("Error: Terminal session '{}' not found", name),
                is_error: true,
            };
        }
        Err(e) => {
            return ToolResult {
                content: format!("Error: Failed to get terminal size: {}", e),
                is_error: true,
            };
        }
    };

    // Extract lines parameter (default to terminal height)
    let lines = args
        .get("lines")
        .and_then(|v| v.as_u64())
        .map(|v| v as u16)
        .unwrap_or(size.1);

    // Read terminal content
    let content = match manager.read_terminal(&name).await {
        Ok(Some(content)) => content,
        Ok(None) => {
            return ToolResult {
                content: format!("Error: Terminal session '{}' not found", name),
                is_error: true,
            };
        }
        Err(e) => {
            return ToolResult {
                content: format!("Error: Failed to read terminal: {}", e),
                is_error: true,
            };
        }
    };

    // Parse content into lines
    let all_lines: Vec<&str> = content.lines().collect();
    let total_lines = all_lines.len() as u16;

    // Calculate which lines to return based on offset_from_bottom and lines
    // Start from the bottom, skip offset_from_bottom lines, then take 'lines' count
    let start_idx = if total_lines > offset_from_bottom + lines {
        (total_lines - offset_from_bottom - lines) as usize
    } else if total_lines > offset_from_bottom {
        0
    } else {
        0
    };

    let end_idx = if total_lines > offset_from_bottom {
        (total_lines - offset_from_bottom) as usize
    } else {
        0
    };

    // Extract selected lines, strip ANSI codes, and join with newlines
    let selected_text: String = all_lines[start_idx..end_idx]
        .iter()
        .map(|line| strip_ansi_codes(line))
        .collect::<Vec<String>>()
        .join("\n");

    ToolResult {
        content: selected_text,
        is_error: false,
    }
}
