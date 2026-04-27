use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::time::sleep;

use crate::terminal::TerminalManager;
use crate::tools::{require_str, ToolResult, ToolRuntime};

/// Get the JSON schema definition for the unified terminal tool
pub fn definition() -> crate::types::ToolDefinition {
    use serde_json::json;

    crate::types::ToolDefinition {
        def_type: "function".to_string(),
        function: crate::types::FunctionDef {
            name: "terminal".to_string(),
            description: "Perform operations on terminal sessions: create, send input, read output, resize, wait for conditions, or close.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name of the terminal session (required for all operations except create where it's the name to assign)"
                    },
                    "operation": {
                        "type": "string",
                        "enum": ["create", "send", "read", "resize", "wait", "close", "list"],
                        "description": "Operation to perform: create (new session), send (input to terminal), read (terminal output), resize (change dimensions), wait (for conditions), close (terminate session), list (show all terminals)"
                    },
                    // Create operation parameters
                    "cols": {
                        "type": "integer",
                        "description": "For operation='create' or 'resize': Terminal width in columns (default: 140 for create)"
                    },
                    "rows": {
                        "type": "integer",
                        "description": "For operation='create' or 'resize': Terminal height in rows (default: 50 for create)"
                    },
                    "cwd": {
                        "type": "string",
                        "description": "For operation='create': Working directory for the shell (default: current directory)"
                    },
                    "env": {
                        "type": "object",
                        "description": "For operation='create': Additional environment variables as key-value pairs",
                        "additionalProperties": { "type": "string" }
                    },
                    // Send operation parameters
                    "input": {
                        "type": "string",
                        "description": "For operation='send': Text to send to the terminal. Normal text is sent as-is. Special sequences in <> notation: <C-a> (Ctrl+A), <C-A> (Ctrl+Shift+A), <A-a> (Alt+A), <C-A-a> (Ctrl+Alt+A), <RET> (Enter), <TAB> (Tab), <BSPC> (Backspace), <DEL> (Delete), <<text>> (literal <text>)"
                    },
                    // Read operation parameters
                    "lines": {
                        "type": "integer",
                        "description": "For operation='read': Number of lines to read (default: terminal height)"
                    },
                    "offset_from_bottom": {
                        "type": "integer",
                        "description": "For operation='read': Lines from bottom to skip (default: 0)"
                    },
                    // Wait operation parameters
                    "wait_type": {
                        "type": "string",
                        "enum": ["command_complete", "output_contains", "idle"],
                        "description": "For operation='wait': Wait condition type"
                    },
                    "text": {
                        "type": "string",
                        "description": "For operation='wait' with wait_type='output_contains': Text to wait for in terminal output"
                    },
                    "seconds": {
                        "type": "integer",
                        "description": "For operation='wait' with wait_type='idle': Seconds of no output to consider idle"
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "For operation='wait': Maximum seconds to wait (default: 30)",
                        "default": 30
                    },
                    // Close operation parameters
                    "archive": {
                        "type": "boolean",
                        "description": "For operation='close': Whether to save final screen content before closing",
                        "default": false
                    }
                },
                "required": ["operation"]
            }),
        },
    }
}

/// Execute the unified terminal tool
pub async fn execute(
    args: Value,
    runtime: &ToolRuntime,
    manager: &TerminalManager,
) -> ToolResult {
    // Extract required operation
    let operation = match require_str(&args, "operation") {
        Ok(op) => op,
        Err(e) => return e,
    };

    match operation.as_str() {
        "create" => execute_create(args, manager).await,
        "send" => execute_send(args, runtime, manager).await,
        "read" => execute_read(args, runtime, manager).await,
        "resize" => execute_resize(args, runtime, manager).await,
        "wait" => execute_wait(args, runtime, manager).await,
        "close" => execute_close(args, runtime, manager).await,
        "list" => execute_list(manager).await,
        _ => ToolResult {
            content: format!("Error: Unknown operation '{}'", operation),
            is_error: true,
        },
    }
}

/// Execute create operation
async fn execute_create(args: Value, manager: &TerminalManager) -> ToolResult {
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
    match manager.create(name.clone(), cols, rows, cwd, env).await {
        Ok(_) => {
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

/// Execute send operation
async fn execute_send(
    args: Value,
    runtime: &ToolRuntime,
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

    // Validate that input is provided
    if input.is_none() {
        return ToolResult {
            content: "Error: 'input' must be provided for send operation".to_string(),
            is_error: true,
        };
    }

    // Parse the unified input notation
    let data = match parse_input(input.unwrap()) {
        Ok(bytes) => bytes,
        Err(e) => return e,
    };

    // Checkout the terminal for this operation
    let owner = runtime.thread_id.clone();
    match manager.checkout_terminal(&name, owner.clone()).await {
        Ok(true) => {}
        Ok(false) => {
            return ToolResult {
                content: format!("Error: Terminal session '{}' no longer exists", name),
                is_error: true,
            };
        }
        Err(e) => {
            return ToolResult {
                content: format!("Error: Failed to checkout terminal: {}", e),
                is_error: true,
            };
        }
    }

    // Send the data to the terminal
    let result = match manager.write_to_terminal(&name, &owner, data).await {
        Ok(true) => {
            let response = json!({
                "name": name,
                "operation": "send",
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
    };

    // Checkin the terminal
    let _ = manager.checkin_terminal(&name, &owner).await;

    result
}

/// Parse unified input notation into bytes
/// 
/// Syntax:
/// - Normal text: sent as-is
/// - `<C-a>` → Ctrl+A (0x01)
/// - `<C-A>` → Ctrl+Shift+A (0x01, uppercase indicates shift)
/// - `<A-a>` → Alt+A (ESC + a)
/// - `<C-A-a>` → Ctrl+Alt+A (ESC + 0x01)
/// - `<RET>` → Enter/Return (\r)
/// - `<TAB>` → Tab (\t)
/// - `<BSPC>` → Backspace (0x7f)
/// - `<DEL>` → Delete (ESC[3~)
/// - `<<text>>` → literal `<text>` (escape sequence)
fn parse_input(input: &str) -> Result<Vec<u8>, ToolResult> {
    let mut result = Vec::new();
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '<' {
            // Check for escape sequence: <<...>>
            if chars.peek() == Some(&'<') {
                chars.next(); // consume second '<'
                // Collect until >>
                let mut literal = String::new();
                let mut found_end = false;
                
                while let Some(inner_ch) = chars.next() {
                    if inner_ch == '>' {
                        if chars.peek() == Some(&'>') {
                            chars.next(); // consume second '>'
                            found_end = true;
                            break;
                        } else {
                            literal.push(inner_ch);
                        }
                    } else {
                        literal.push(inner_ch);
                    }
                }
                
                if !found_end {
                    return Err(ToolResult {
                        content: "Error: Unclosed escape sequence <<...>>".to_string(),
                        is_error: true,
                    });
                }
                
                // Output the literal including the angle brackets
                result.push(b'<');
                result.extend_from_slice(literal.as_bytes());
                result.push(b'>');
            } else {
                // Parse <...> notation
                let mut content = String::new();
                let mut found_close = false;
                
                while let Some(inner_ch) = chars.next() {
                    if inner_ch == '>' {
                        found_close = true;
                        break;
                    }
                    content.push(inner_ch);
                }
                
                if !found_close {
                    return Err(ToolResult {
                        content: "Error: Unclosed notation <...>".to_string(),
                        is_error: true,
                    });
                }
                
                // Parse the content and append the sequence
                match parse_bracket_notation(&content) {
                    Ok(seq) => result.extend_from_slice(seq.as_bytes()),
                    Err(e) => return Err(e),
                }
            }
        } else {
            // Normal character
            result.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
        }
    }

    Ok(result)
}

/// Parse content inside angle brackets
/// 
/// Examples:
/// - `C-a` → Ctrl+A
/// - `C-A` → Ctrl+Shift+A  
/// - `A-a` → Alt+A
/// - `C-A-a` → Ctrl+Alt+A
/// - `RET` → Enter
/// - `TAB` → Tab
/// - `BSPC` → Backspace
/// - `DEL` → Delete
fn parse_bracket_notation(content: &str) -> Result<String, ToolResult> {
    if content.is_empty() {
        return Err(ToolResult {
            content: "Error: Empty notation <>".to_string(),
            is_error: true,
        });
    }

    // Check for special key names (no modifiers)
    let upper = content.to_uppercase();
    match upper.as_str() {
        "RET" | "RETURN" | "CR" => return Ok("\r".to_string()),
        "TAB" => return Ok("\t".to_string()),
        "BSPC" | "BACKSPACE" | "BS" => return Ok("\x7f".to_string()),
        "DEL" | "DELETE" => return Ok("\x1b[3~".to_string()),
        "ESC" | "ESCAPE" => return Ok("\x1b".to_string()),
        "SPACE" => return Ok(" ".to_string()),
        "UP" => return Ok("\x1b[A".to_string()),
        "DOWN" => return Ok("\x1b[B".to_string()),
        "RIGHT" => return Ok("\x1b[C".to_string()),
        "LEFT" => return Ok("\x1b[D".to_string()),
        "HOME" => return Ok("\x1b[H".to_string()),
        "END" => return Ok("\x1b[F".to_string()),
        "PGUP" | "PAGEUP" | "PAGE_UP" => return Ok("\x1b[5~".to_string()),
        "PGDN" | "PAGEDOWN" | "PAGE_DOWN" => return Ok("\x1b[6~".to_string()),
        "INS" | "INSERT" => return Ok("\x1b[2~".to_string()),
        "F1" => return Ok("\x1bOP".to_string()),
        "F2" => return Ok("\x1bOQ".to_string()),
        "F3" => return Ok("\x1bOR".to_string()),
        "F4" => return Ok("\x1bOS".to_string()),
        "F5" => return Ok("\x1b[15~".to_string()),
        "F6" => return Ok("\x1b[17~".to_string()),
        "F7" => return Ok("\x1b[18~".to_string()),
        "F8" => return Ok("\x1b[19~".to_string()),
        "F9" => return Ok("\x1b[20~".to_string()),
        "F10" => return Ok("\x1b[21~".to_string()),
        "F11" => return Ok("\x1b[23~".to_string()),
        "F12" => return Ok("\x1b[24~".to_string()),
        _ => {}
    }

    // Parse modifier notation: C-, A-, S- prefixes
    let mut has_ctrl = false;
    let mut has_alt = false;
    let mut has_shift = false;
    
    // Scan for modifier prefixes by iterating through the string
    let mut remaining = content;
    loop {
        if remaining.len() >= 2 {
            let first = remaining.chars().next().unwrap();
            let second = remaining.chars().nth(1).unwrap();
            
            if second == '-' {
                match first {
                    'C' => { has_ctrl = true; remaining = &remaining[2..]; continue; }
                    'A' => { has_alt = true; remaining = &remaining[2..]; continue; }
                    'S' => { has_shift = true; remaining = &remaining[2..]; continue; }
                    _ => break,
                }
            }
        }
        break;
    }

    // If no modifiers found, check if it's a single character (for C-a style)
    if !has_ctrl && !has_alt && !has_shift {
        // Try parsing as single char with implicit modifiers from case
        if content.len() == 1 {
            let ch = content.chars().next().unwrap();
            return Ok(build_modified_char(ch, false, false, false));
        }
        
        // Unknown notation
        return Err(ToolResult {
            content: format!("Error: Unknown notation '<{}>'", content),
            is_error: true,
        });
    }

    // We have modifiers, now parse the key
    if remaining.is_empty() {
        return Err(ToolResult {
            content: format!("Error: Missing key after modifiers in '<{}>'", content),
            is_error: true,
        });
    }

    // Check if remaining is a special key name
    let key_upper = remaining.to_uppercase();
    match key_upper.as_str() {
        "RET" | "RETURN" => {
            if has_ctrl {
                return Ok("\n".to_string());
            } else {
                return Ok("\r".to_string());
            }
        }
        "TAB" => {
            if has_shift {
                return Ok("\x1b[Z".to_string());
            } else {
                return Ok("\t".to_string());
            }
        }
        "BSPC" | "BACKSPACE" | "BS" => return Ok("\x7f".to_string()),
        "DEL" | "DELETE" => {
            let seq = match (has_shift, has_ctrl, has_alt) {
                (true, _, _) => "\x1b[3;2~",
                (_, true, _) => "\x1b[3;5~",
                (_, _, true) => "\x1b[3;3~",
                _ => "\x1b[3~",
            };
            return Ok(seq.to_string());
        }
        "UP" => return Ok(build_modified_vt_sequence("A", has_shift, has_ctrl, has_alt)),
        "DOWN" => return Ok(build_modified_vt_sequence("B", has_shift, has_ctrl, has_alt)),
        "RIGHT" => return Ok(build_modified_vt_sequence("C", has_shift, has_ctrl, has_alt)),
        "LEFT" => return Ok(build_modified_vt_sequence("D", has_shift, has_ctrl, has_alt)),
        "HOME" => return Ok(build_modified_vt_sequence("H", has_shift, has_ctrl, has_alt)),
        "END" => return Ok(build_modified_vt_sequence("F", has_shift, has_ctrl, has_alt)),
        "INS" | "INSERT" => {
            let seq = match (has_shift, has_ctrl, has_alt) {
                (true, _, _) => "\x1b[2;2~",
                (_, true, _) => "\x1b[2;5~",
                (_, _, true) => "\x1b[2;3~",
                _ => "\x1b[2~",
            };
            return Ok(seq.to_string());
        }
        "PGUP" | "PAGEUP" => {
            let seq = match (has_shift, has_ctrl, has_alt) {
                (true, _, _) => "\x1b[5;2~",
                (_, true, _) => "\x1b[5;5~",
                (_, _, true) => "\x1b[5;3~",
                _ => "\x1b[5~",
            };
            return Ok(seq.to_string());
        }
        "PGDN" | "PAGEDOWN" => {
            let seq = match (has_shift, has_ctrl, has_alt) {
                (true, _, _) => "\x1b[6;2~",
                (_, true, _) => "\x1b[6;5~",
                (_, _, true) => "\x1b[6;3~",
                _ => "\x1b[6~",
            };
            return Ok(seq.to_string());
        }
        _ => {}
    }

    // Single character key with modifiers
    if remaining.len() == 1 {
        let ch = remaining.chars().next().unwrap();
        return Ok(build_modified_char(ch, has_ctrl, has_alt, has_shift));
    }

    // Unknown key with modifiers
    Err(ToolResult {
        content: format!("Error: Unknown key '{}' in notation '<{}>'", remaining, content),
        is_error: true,
    })
}

/// Build a modified character sequence
fn build_modified_char(ch: char, has_ctrl: bool, has_alt: bool, _has_shift: bool) -> String {
    let mut result = String::new();
    
    if has_alt {
        result.push('\x1b');
    }
    
    if has_ctrl {
        let ctrl_char = match ch.to_ascii_lowercase() {
            'a' => '\x01',
            'b' => '\x02',
            'c' => '\x03',
            'd' => '\x04',
            'e' => '\x05',
            'f' => '\x06',
            'g' => '\x07',
            'h' => '\x08',
            'i' => '\x09',
            'j' => '\x0a',
            'k' => '\x0b',
            'l' => '\x0c',
            'm' => '\x0d',
            'n' => '\x0e',
            'o' => '\x0f',
            'p' => '\x10',
            'q' => '\x11',
            'r' => '\x12',
            's' => '\x13',
            't' => '\x14',
            'u' => '\x15',
            'v' => '\x16',
            'w' => '\x17',
            'x' => '\x18',
            'y' => '\x19',
            'z' => '\x1a',
            '[' => '\x1b',
            '\\' => '\x1c',
            ']' => '\x1d',
            '^' => '\x1e',
            '_' => '\x1f',
            '?' => '\x7f',
            c => c,
        };
        result.push(ctrl_char);
    } else {
        result.push(ch);
    }
    
    result
}

/// Build modified VT sequence for arrow keys and similar
fn build_modified_vt_sequence(base: &str, has_shift: bool, has_ctrl: bool, has_alt: bool) -> String {
    // Determine modifier parameter
    let modifier = match (has_shift, has_ctrl, has_alt) {
        (true, false, false) => ";2",
        (false, true, false) => ";5",
        (false, false, true) => ";3",
        (true, true, false) => ";6",
        (true, false, true) => ";4",
        (false, true, true) => ";7",
        (true, true, true) => ";8",
        _ => "",
    };

    if modifier.is_empty() {
        format!("\x1b[{}]", base)
    } else {
        format!("\x1b[1{}[{}]", modifier, base)
    }
}

/// Execute read operation
async fn execute_read(
    args: Value,
    runtime: &ToolRuntime,
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

    // Checkout the terminal for this operation
    let owner = runtime.thread_id.clone();
    match manager.checkout_terminal(&name, owner.clone()).await {
        Ok(true) => {}
        Ok(false) => {
            return ToolResult {
                content: format!("Error: Terminal session '{}' no longer exists", name),
                is_error: true,
            };
        }
        Err(e) => {
            return ToolResult {
                content: format!("Error: Failed to checkout terminal: {}", e),
                is_error: true,
            };
        }
    }

    // Extract optional parameters with defaults
    let offset_from_bottom = args
        .get("offset_from_bottom")
        .and_then(|v| v.as_u64())
        .map(|v| v as u16)
        .unwrap_or(0);

    // Check if terminal is in alternate screen mode
    let is_alt_screen = match manager.is_alternate_screen(&name, &owner).await {
        Ok(Some(is_alt)) => is_alt,
        Ok(None) => {
            let _ = manager.checkin_terminal(&name, &owner).await;
            return ToolResult {
                content: format!("Error: Terminal session '{}' not found", name),
                is_error: true,
            };
        }
        Err(e) => {
            let _ = manager.checkin_terminal(&name, &owner).await;
            return ToolResult {
                content: format!("Error: Failed to check terminal screen mode: {}", e),
                is_error: true,
            };
        }
    };

    // If in alternate screen mode and trying to access scrollback, return error
    if is_alt_screen && offset_from_bottom > 0 {
        let _ = manager.checkin_terminal(&name, &owner).await;
        return ToolResult {
            content: "Error: Terminal is in alternate screen mode (e.g., vim, less). Scrollback is not available. Use offset_from_bottom=0 to read only the visible screen.".to_string(),
            is_error: true,
        };
    }

    // Get terminal size for defaults
    let size = match manager.get_terminal_size(&name, &owner).await {
        Ok(Some((cols, rows))) => (cols, rows),
        Ok(None) => {
            let _ = manager.checkin_terminal(&name, &owner).await;
            return ToolResult {
                content: format!("Error: Terminal session '{}' not found", name),
                is_error: true,
            };
        }
        Err(e) => {
            let _ = manager.checkin_terminal(&name, &owner).await;
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
    let content = match manager.read_terminal(&name, &owner).await {
        Ok(Some(content)) => content,
        Ok(None) => {
            let _ = manager.checkin_terminal(&name, &owner).await;
            return ToolResult {
                content: format!("Error: Terminal session '{}' not found", name),
                is_error: true,
            };
        }
        Err(e) => {
            let _ = manager.checkin_terminal(&name, &owner).await;
            return ToolResult {
                content: format!("Error: Failed to read terminal: {}", e),
                is_error: true,
            };
        }
    };

    // Checkin the terminal
    let _ = manager.checkin_terminal(&name, &owner).await;

    // Parse content into lines
    let all_lines: Vec<&str> = content.lines().collect();
    let total_lines = all_lines.len() as u16;

    // Calculate which lines to return based on offset_from_bottom and lines
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

/// Execute resize operation
async fn execute_resize(
    args: Value,
    runtime: &ToolRuntime,
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
                content: "Error: 'cols' argument required for resize operation".to_string(),
                is_error: true,
            }
        }
    };

    // Extract required rows
    let rows = match args.get("rows").and_then(|v| v.as_u64()) {
        Some(v) => v as u16,
        None => {
            return ToolResult {
                content: "Error: 'rows' argument required for resize operation".to_string(),
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

    // Checkout the terminal for this operation
    let owner = runtime.thread_id.clone();
    match manager.checkout_terminal(&name, owner.clone()).await {
        Ok(true) => {}
        Ok(false) => {
            return ToolResult {
                content: format!("Error: Terminal session '{}' no longer exists", name),
                is_error: true,
            };
        }
        Err(e) => {
            return ToolResult {
                content: format!("Error: Failed to checkout terminal: {}", e),
                is_error: true,
            };
        }
    }

    // Resize the terminal using the manager
    let result = match manager.resize_terminal(&name, &owner, cols, rows).await {
        Ok(true) => {
            let response = json!({
                "name": name,
                "operation": "resize",
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
    };

    // Checkin the terminal
    let _ = manager.checkin_terminal(&name, &owner).await;

    result
}

/// Execute wait operation
async fn execute_wait(
    args: Value,
    runtime: &ToolRuntime,
    manager: &TerminalManager,
) -> ToolResult {
    // Extract required name
    let name = match require_str(&args, "name") {
        Ok(n) => n,
        Err(e) => return e,
    };

    // Extract required wait_type
    let wait_type = match require_str(&args, "wait_type") {
        Ok(t) => t,
        Err(e) => return e,
    };

    // Extract timeout (default: 30 seconds)
    let timeout_secs = args
        .get("timeout")
        .and_then(|v| v.as_u64())
        .unwrap_or(30) as u64;

    // Check if terminal exists
    if !manager.contains(&name).await {
        return ToolResult {
            content: format!("Error: Terminal session '{}' does not exist", name),
            is_error: true,
        };
    }

    // Checkout the terminal for this operation
    let owner = runtime.thread_id.clone();
    match manager.checkout_terminal(&name, owner.clone()).await {
        Ok(true) => {}
        Ok(false) => {
            return ToolResult {
                content: format!("Error: Terminal session '{}' no longer exists", name),
                is_error: true,
            };
        }
        Err(e) => {
            return ToolResult {
                content: format!("Error: Failed to checkout terminal: {}", e),
                is_error: true,
            };
        }
    }

    // Execute the appropriate wait operation
    let result = match wait_type.as_str() {
        "command_complete" => wait_command_complete(&name, &owner, manager, timeout_secs).await,
        "output_contains" => {
            // Extract required text parameter
            let text = match require_str(&args, "text") {
                Ok(t) => t,
                Err(e) => {
                    let _ = manager.checkin_terminal(&name, &owner).await;
                    return e;
                }
            };
            wait_output_contains(&name, &owner, manager, &text, timeout_secs).await
        }
        "idle" => {
            // Extract required seconds parameter
            let idle_secs = match args.get("seconds").and_then(|v| v.as_u64()) {
                Some(s) => s as u64,
                None => {
                    let _ = manager.checkin_terminal(&name, &owner).await;
                    return ToolResult {
                        content: "Error: 'seconds' argument required for idle wait_type".to_string(),
                        is_error: true,
                    };
                }
            };
            wait_idle(&name, &owner, manager, idle_secs, timeout_secs).await
        }
        _ => {
            let _ = manager.checkin_terminal(&name, &owner).await;
            return ToolResult {
                content: format!("Error: Unknown wait_type '{}'", wait_type),
                is_error: true,
            };
        }
    };

    // Checkin the terminal
    let _ = manager.checkin_terminal(&name, &owner).await;

    result
}

/// Wait for the current command to complete (command state != Running)
async fn wait_command_complete(
    name: &str,
    owner: &str,
    manager: &TerminalManager,
    timeout_secs: u64,
) -> ToolResult {
    let start = Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
    let poll_interval = Duration::from_millis(100);

    loop {
        // Check timeout
        if start.elapsed() >= timeout {
            return ToolResult {
                content: json!({
                    "name": name,
                    "wait_type": "command_complete",
                    "status": "timeout",
                    "waited_seconds": start.elapsed().as_secs(),
                    "timeout_seconds": timeout_secs
                }).to_string(),
                is_error: true,
            };
        }

        // Check if command is still running
        match manager.is_command_running(name, owner).await {
            Ok(Some(false)) => {
                // Command completed - get exit code if available
                let exit_code = match manager.get_last_exit_code(name, owner).await {
                    Ok(Some(code)) => code,
                    _ => None,
                };

                return ToolResult {
                    content: json!({
                        "name": name,
                        "wait_type": "command_complete",
                        "status": "success",
                        "waited_seconds": start.elapsed().as_secs(),
                        "exit_code": exit_code
                    }).to_string(),
                    is_error: false,
                };
            }
            Ok(Some(true)) => {
                // Command still running, continue waiting
            }
            Ok(None) => {
                return ToolResult {
                    content: format!("Error: Terminal session '{}' not found during wait", name),
                    is_error: true,
                };
            }
            Err(e) => {
                return ToolResult {
                    content: format!("Error: Failed to check command state: {}", e),
                    is_error: true,
                };
            }
        }

        // Sleep before next poll
        sleep(poll_interval).await;
    }
}

/// Wait for specific text to appear in terminal output
async fn wait_output_contains(
    name: &str,
    owner: &str,
    manager: &TerminalManager,
    text: &str,
    timeout_secs: u64,
) -> ToolResult {
    let start = Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
    let poll_interval = Duration::from_millis(100);

    loop {
        // Check timeout
        if start.elapsed() >= timeout {
            return ToolResult {
                content: json!({
                    "name": name,
                    "wait_type": "output_contains",
                    "status": "timeout",
                    "waited_seconds": start.elapsed().as_secs(),
                    "timeout_seconds": timeout_secs,
                    "search_text": text
                }).to_string(),
                is_error: true,
            };
        }

        // Read terminal content
        match manager.read_terminal(name, owner).await {
            Ok(Some(content)) => {
                // Check if text appears in content
                if content.contains(text) {
                    return ToolResult {
                        content: json!({
                            "name": name,
                            "wait_type": "output_contains",
                            "status": "success",
                            "waited_seconds": start.elapsed().as_secs(),
                            "found_text": text
                        }).to_string(),
                        is_error: false,
                    };
                }
            }
            Ok(None) => {
                return ToolResult {
                    content: format!("Error: Terminal session '{}' not found during wait", name),
                    is_error: true,
                };
            }
            Err(e) => {
                return ToolResult {
                    content: format!("Error: Failed to read terminal: {}", e),
                    is_error: true,
                };
            }
        }

        // Sleep before next poll
        sleep(poll_interval).await;
    }
}

/// Wait for terminal to be idle (no output changes for specified seconds)
async fn wait_idle(
    name: &str,
    owner: &str,
    manager: &TerminalManager,
    idle_secs: u64,
    timeout_secs: u64,
) -> ToolResult {
    let start = Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
    let poll_interval = Duration::from_millis(100);

    let mut last_content_hash: Option<u64> = None;
    let mut last_change_time = Instant::now();

    loop {
        // Check overall timeout
        if start.elapsed() >= timeout {
            return ToolResult {
                content: json!({
                    "name": name,
                    "wait_type": "idle",
                    "status": "timeout",
                    "waited_seconds": start.elapsed().as_secs(),
                    "timeout_seconds": timeout_secs,
                    "required_idle_seconds": idle_secs
                }).to_string(),
                is_error: true,
            };
        }

        // Read terminal content
        match manager.read_terminal(name, owner).await {
            Ok(Some(content)) => {
                // Compute a simple hash of the content
                let current_hash = compute_hash(&content);

                match last_content_hash {
                    Some(prev_hash) => {
                        if current_hash != prev_hash {
                            // Content changed, reset idle timer
                            last_content_hash = Some(current_hash);
                            last_change_time = Instant::now();
                        } else {
                            // Content unchanged, check if we've been idle long enough
                            let idle_duration = last_change_time.elapsed();
                            if idle_duration >= Duration::from_secs(idle_secs) {
                                return ToolResult {
                                    content: json!({
                                        "name": name,
                                        "wait_type": "idle",
                                        "status": "success",
                                        "waited_seconds": start.elapsed().as_secs(),
                                        "idle_seconds": idle_duration.as_secs(),
                                        "required_idle_seconds": idle_secs
                                    }).to_string(),
                                    is_error: false,
                                };
                            }
                        }
                    }
                    None => {
                        // First read, initialize hash
                        last_content_hash = Some(current_hash);
                        last_change_time = Instant::now();
                    }
                }
            }
            Ok(None) => {
                return ToolResult {
                    content: format!("Error: Terminal session '{}' not found during wait", name),
                    is_error: true,
                };
            }
            Err(e) => {
                return ToolResult {
                    content: format!("Error: Failed to read terminal: {}", e),
                    is_error: true,
                };
            }
        }

        // Sleep before next poll
        sleep(poll_interval).await;
    }
}

/// Execute close operation
async fn execute_close(
    args: Value,
    runtime: &ToolRuntime,
    manager: &TerminalManager,
) -> ToolResult {
    // Extract required name
    let name = match require_str(&args, "name") {
        Ok(n) => n,
        Err(e) => return e,
    };

    // Extract optional archive parameter (default: false)
    let archive = args.get("archive").and_then(|v| v.as_bool()).unwrap_or(false);

    // Checkout the terminal for this operation (we keep it checked out until close)
    let owner = runtime.thread_id.clone();
    match manager.checkout_terminal(&name, owner.clone()).await {
        Ok(true) => {}
        Ok(false) => {
            return ToolResult {
                content: format!("Error: Terminal session '{}' does not exist", name),
                is_error: true,
            };
        }
        Err(e) => {
            return ToolResult {
                content: format!("Error: Failed to checkout terminal: {}", e),
                is_error: true,
            };
        }
    }

    // If archiving is requested, read final content before closing
    let archived_content = if archive {
        match manager.read_terminal(&name, &owner).await {
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

    // Shutdown the terminal session (this removes it from the manager)
    match manager.shutdown_terminal(&name).await {
        Ok(true) => {
            let response = if let Some(content) = archived_content {
                json!({
                    "name": name,
                    "operation": "close",
                    "status": "closed",
                    "archived": true,
                    "final_content": content
                })
            } else {
                json!({
                    "name": name,
                    "operation": "close",
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

/// Execute list operation - returns all terminals with their info
async fn execute_list(manager: &TerminalManager) -> ToolResult {
    match manager.list_terminals().await {
        Ok(terminals) => {
            let response = json!({
                "terminals": terminals,
                "count": terminals.len()
            });

            ToolResult {
                content: response.to_string(),
                is_error: false,
            }
        }
        Err(e) => ToolResult {
            content: format!("Error: Failed to list terminals: {}", e),
            is_error: true,
        },
    }
}

/// Strip ANSI escape sequences from text
fn strip_ansi_codes(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&next_ch) = chars.peek() {
                    chars.next();
                    if next_ch.is_ascii_alphabetic() || matches!(next_ch, '@' | '`' | '{' | '|' | '}' | '~') {
                        break;
                    }
                }
            }
        } else {
            result.push(ch);
        }
    }
    
    result
}

/// Compute a simple hash of a string for change detection
fn compute_hash(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}
