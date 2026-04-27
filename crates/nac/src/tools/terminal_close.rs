use serde_json::{json, Value};

use crate::terminal::TerminalManager;
use crate::tools::{require_str, ToolResult, ToolRuntime};

/// Get the JSON schema definition for the terminal_close tool
pub fn definition() -> crate::types::ToolDefinition {
    use serde_json::json;

    crate::types::ToolDefinition {
        def_type: "function".to_string(),
        function: crate::types::FunctionDef {
            name: "terminal_close".to_string(),
            description: "Close a terminal session and optionally archive its final content.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Terminal name"
                    },
                    "archive": {
                        "type": "boolean",
                        "description": "Whether to save final screen content",
                        "default": false
                    }
                },
                "required": ["name"]
            }),
        },
    }
}

/// Execute the terminal_close tool
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

    // Extract optional archive parameter (default: false)
    let archive = args.get("archive").and_then(|v| v.as_bool()).unwrap_or(false);

    // If archiving is requested, read final content before closing
    let archived_content = if archive {
        match manager.read_terminal(&name).await {
            Ok(Some(content)) => Some(content),
            Ok(None) => None,
            Err(e) => {
                return ToolResult {
                    content: format!("Error: Failed to read terminal content before closing: {}", e),
                    is_error: true,
                };
            }
        }
    } else {
        None
    };

    // Shutdown the terminal session
    match manager.shutdown_terminal(&name).await {
        Ok(true) => {
            // Terminal was found and shut down
            let response = if let Some(content) = archived_content {
                json!({
                    "name": name,
                    "status": "closed",
                    "archived": true,
                    "final_content": content
                })
            } else {
                json!({
                    "name": name,
                    "status": "closed",
                    "archived": false
                })
            };

            ToolResult {
                content: response.to_string(),
                is_error: false,
            }
        }
        Ok(false) => {
            // Terminal was not found
            ToolResult {
                content: format!("Error: Terminal session '{}' not found", name),
                is_error: true,
            }
        }
        Err(e) => ToolResult {
            content: format!("Error: Failed to close terminal session: {}", e),
            is_error: true,
        },
    }
}
