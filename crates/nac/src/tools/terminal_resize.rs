use serde_json::{json, Value};

use crate::terminal::TerminalManager;
use crate::tools::{require_str, ToolResult, ToolRuntime};

/// Get the JSON schema definition for the terminal_resize tool
pub fn definition() -> crate::types::ToolDefinition {
    use serde_json::json;

    crate::types::ToolDefinition {
        def_type: "function".to_string(),
        function: crate::types::FunctionDef {
            name: "terminal_resize".to_string(),
            description: "Resize an existing terminal session to new dimensions.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Terminal name"
                    },
                    "cols": {
                        "type": "integer",
                        "description": "New width in columns"
                    },
                    "rows": {
                        "type": "integer",
                        "description": "New height in rows"
                    }
                },
                "required": ["name", "cols", "rows"]
            }),
        },
    }
}

/// Execute the terminal_resize tool
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

    // Extract required cols
    let cols = match args.get("cols").and_then(|v| v.as_u64()) {
        Some(v) => v as u16,
        None => {
            return ToolResult {
                content: "Error: 'cols' argument required".to_string(),
                is_error: true,
            }
        }
    };

    // Extract required rows
    let rows = match args.get("rows").and_then(|v| v.as_u64()) {
        Some(v) => v as u16,
        None => {
            return ToolResult {
                content: "Error: 'rows' argument required".to_string(),
                is_error: true,
            }
        }
    };

    // Check if terminal exists
    if !manager.contains(&name).await {
        return ToolResult {
            content: format!("Error: Terminal session '{}' not found", name),
            is_error: true,
        };
    }

    // Resize the terminal using the manager
    match manager.resize_terminal(&name, cols, rows).await {
        Ok(true) => {
            // Return success response with new size
            let response = json!({
                "name": name,
                "cols": cols,
                "rows": rows,
                "status": "resized"
            });

            ToolResult {
                content: response.to_string(),
                is_error: false,
            }
        }
        Ok(false) => ToolResult {
            content: format!("Error: Terminal session '{}' not found", name),
            is_error: true,
        },
        Err(e) => ToolResult {
            content: format!("Error: Failed to resize terminal session: {}", e),
            is_error: true,
        },
    }
}
