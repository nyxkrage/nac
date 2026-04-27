use serde_json::{json, Value};

use crate::terminal::TerminalManager;
use crate::tools::{require_str, require_string_array, ToolResult, ToolRuntime};

/// Get the JSON schema definition for the terminal_send tool
pub fn definition() -> crate::types::ToolDefinition {
    use serde_json::json;

    crate::types::ToolDefinition {
        def_type: "function".to_string(),
        function: crate::types::FunctionDef {
            name: "terminal_send".to_string(),
            description: "Send input to an interactive terminal session. Either provide literal text via 'input' or a special key with optional modifiers.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name of the terminal session to send input to"
                    },
                    "input": {
                        "type": "string",
                        "description": "Literal text to send to the terminal"
                    },
                    "special_key": {
                        "type": "string",
                        "enum": [
                            "enter", "tab", "escape", "backspace",
                            "up", "down", "left", "right",
                            "home", "end", "page_up", "page_down",
                            "f1", "f2", "f3", "f4", "f5", "f6",
                            "f7", "f8", "f9", "f10", "f11", "f12"
                        ],
                        "description": "Special key to send to the terminal"
                    },
                    "modifiers": {
                        "type": "array",
                        "items": {
                            "type": "string",
                            "enum": ["ctrl", "alt", "shift"]
                        },
                        "description": "Modifier keys to apply to the special_key"
                    }
                },
                "required": ["name"]
            }),
        },
    }
}

/// Execute the terminal_send tool
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

    // Check if the terminal exists
    if !manager.contains(&name).await {
        return ToolResult {
            content: format!("Error: Terminal session '{}' does not exist", name),
            is_error: true,
        };
    }

    // Get input text if provided
    let input = args.get("input").and_then(|v| v.as_str());

    // Get special key if provided
    let special_key = args.get("special_key").and_then(|v| v.as_str());

    // Get modifiers if provided
    let modifiers = match require_string_array(&args, "modifiers") {
        Ok(m) => m,
        Err(e) => return e,
    };

    // Validate that at least one of input or special_key is provided
    if input.is_none() && special_key.is_none() {
        return ToolResult {
            content: "Error: Either 'input' or 'special_key' must be provided".to_string(),
            is_error: true,
        };
    }

    // Build the data to send
    let data = if let Some(text) = input {
        // Send literal text
        text.as_bytes().to_vec()
    } else if let Some(key) = special_key {
        // Convert special key to VT sequence with modifiers
        match build_vt_sequence(key, &modifiers) {
            Ok(seq) => seq.into_bytes(),
            Err(e) => return e,
        }
    } else {
        // This shouldn't happen due to validation above
        return ToolResult {
            content: "Error: No input to send".to_string(),
            is_error: true,
        };
    };

    // Send the data to the terminal
    match manager.write_to_terminal(&name, data).await {
        Ok(true) => {
            let response = json!({
                "name": name,
                "status": "sent"
            });

            ToolResult {
                content: response.to_string(),
                is_error: false,
            }
        }
        Ok(false) => ToolResult {
            content: format!("Error: Terminal session '{}' no longer exists", name),
            is_error: true,
        },
        Err(e) => ToolResult {
            content: format!("Error: Failed to send input to terminal: {}", e),
            is_error: true,
        },
    }
}

/// Build a VT sequence for a special key with optional modifiers
fn build_vt_sequence(key: &str, modifiers: &[String]) -> Result<String, ToolResult> {
    let has_ctrl = modifiers.contains(&"ctrl".to_string());
    let has_alt = modifiers.contains(&"alt".to_string());
    let has_shift = modifiers.contains(&"shift".to_string());

    // Build the sequence based on key and modifiers
    let sequence = match key {
        // Basic keys
        "enter" => {
            if has_ctrl {
                "\n".to_string()
            } else {
                "\r".to_string()
            }
        }
        "tab" => {
            if has_shift {
                "\x1b[Z".to_string() // Shift+Tab
            } else {
                "\t".to_string()
            }
        }
        "escape" => "\x1b".to_string(),
        "backspace" => "\x7f".to_string(),

        // Arrow keys
        "up" => build_csi_sequence("A", has_ctrl, has_alt, has_shift),
        "down" => build_csi_sequence("B", has_ctrl, has_alt, has_shift),
        "right" => build_csi_sequence("C", has_ctrl, has_alt, has_shift),
        "left" => build_csi_sequence("D", has_ctrl, has_alt, has_shift),

        // Navigation keys
        "home" => build_csi_sequence("H", has_ctrl, has_alt, has_shift),
        "end" => build_csi_sequence("F", has_ctrl, has_alt, has_shift),
        "page_up" => build_csi_sequence("5~", has_ctrl, has_alt, has_shift),
        "page_down" => build_csi_sequence("6~", has_ctrl, has_alt, has_shift),

        // Function keys
        "f1" => build_csi_sequence("P", has_ctrl, has_alt, has_shift),
        "f2" => build_csi_sequence("Q", has_ctrl, has_alt, has_shift),
        "f3" => build_csi_sequence("R", has_ctrl, has_alt, has_shift),
        "f4" => build_csi_sequence("S", has_ctrl, has_alt, has_shift),
        "f5" => "\x1b[15~".to_string(),
        "f6" => "\x1b[17~".to_string(),
        "f7" => "\x1b[18~".to_string(),
        "f8" => "\x1b[19~".to_string(),
        "f9" => "\x1b[20~".to_string(),
        "f10" => "\x1b[21~".to_string(),
        "f11" => "\x1b[23~".to_string(),
        "f12" => "\x1b[24~".to_string(),

        _ => {
            return Err(ToolResult {
                content: format!("Error: Unknown special key '{}'", key),
                is_error: true,
            })
        }
    };

    Ok(sequence)
}

/// Build a CSI (Control Sequence Introducer) sequence with modifier support
/// Format: ESC [ <final_byte> or ESC [ 1 ; <modifier> <final_byte>
fn build_csi_sequence(final_byte: &str, ctrl: bool, alt: bool, shift: bool) -> String {
    // Calculate modifier parameter
    // Modifier values: 1=none, 2=shift, 3=alt, 4=alt+shift, 5=ctrl, 6=ctrl+shift, 7=ctrl+alt, 8=ctrl+alt+shift
    let modifier = match (ctrl, alt, shift) {
        (false, false, false) => 0, // No modifier, use simple form
        (false, false, true) => 2,
        (false, true, false) => 3,
        (false, true, true) => 4,
        (true, false, false) => 5,
        (true, false, true) => 6,
        (true, true, false) => 7,
        (true, true, true) => 8,
    };

    if modifier == 0 {
        // No modifiers - use simple form: ESC [ <final_byte>
        format!("\x1b[{}]", final_byte)
    } else {
        // With modifiers - include the modifier number
        // For keys like 5~ and 6~ (page up/down), we need to insert the modifier before the ~
        // Format: ESC [ <prefix> ; <modifier> ~
        if final_byte.ends_with('~') {
            let prefix = &final_byte[..final_byte.len() - 1];
            format!("\x1b[{};{}~", prefix, modifier)
        } else {
            // Format: ESC [ 1 ; <modifier> <final_byte>
            format!("\x1b[1;{}{}", modifier, final_byte)
        }
    }
}
