use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::terminal::TerminalManager;
use crate::tools::{require_str, ToolResult, ToolRuntime};

/// Get the JSON schema definition for the terminal_create tool
pub fn definition() -> crate::types::ToolDefinition {
    use serde_json::json;

    crate::types::ToolDefinition {
        def_type: "function".to_string(),
        function: crate::types::FunctionDef {
            name: "terminal_create".to_string(),
            description: "Create a new interactive terminal session with a PTY.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Unique name for the terminal session"
                    },
                    "cols": {
                        "type": "integer",
                        "description": "Terminal width in columns (default: 140)"
                    },
                    "rows": {
                        "type": "integer",
                        "description": "Terminal height in rows (default: 50)"
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Working directory for the shell (default: current directory)"
                    },
                    "env": {
                        "type": "object",
                        "description": "Additional environment variables as key-value pairs",
                        "additionalProperties": { "type": "string" }
                    }
                },
                "required": ["name"]
            }),
        },
    }
}

/// Execute the terminal_create tool
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

    // Check if a terminal with this name already exists
    if manager.contains(&name).await {
        return ToolResult {
            content: format!("Error: Terminal session '{}' already exists", name),
            is_error: true,
        };
    }

    // Extract optional parameters
    let cols = args.get("cols").and_then(|v| v.as_u64()).map(|v| v as usize);
    let rows = args.get("rows").and_then(|v| v.as_u64()).map(|v| v as usize);

    // Convert cwd to owned PathBuf
    let cwd: Option<PathBuf> = args.get("cwd").and_then(|v| v.as_str()).map(PathBuf::from);

    // Parse environment variables
    let env: Option<HashMap<String, String>> = args.get("env").and_then(|v| {
        v.as_object().map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
    });

    // Create the terminal session using the manager
    // This spawns a runner thread and stores the command sender
    match manager.create(name.clone(), cols, rows, cwd, env).await {
        Ok(_) => {
            // Return success response
            let response = json!({
                "name": name,
                "cols": cols.unwrap_or(140),
                "rows": rows.unwrap_or(50),
                "status": "created"
            });

            ToolResult {
                content: response.to_string(),
                is_error: false,
            }
        }
        Err(e) => ToolResult {
            content: format!("Error: Failed to create terminal session: {}", e),
            is_error: true,
        },
    }
}
