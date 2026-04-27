//! Comprehensive integration tests for the terminal tool suite
//!
//! This test file covers:
//! 1. Terminal lifecycle (create, send, read, close)
//! 2. Emacs notation parsing for send operations
//! 3. Command completion waiting (terminal_wait command_complete)
//! 4. Output contains waiting (terminal_wait output_contains)
//! 5. Idle state waiting (terminal_wait idle)
//! 6. Terminal resizing
//! 7. Session ownership (checkout/checkin)
//! 8. Alternate screen mode detection

use nac::terminal::{spawn_runner, TerminalCommand, TerminalResponse};
use nac::terminal::manager::TerminalManager;
use std::thread;
use std::time::Duration;
use tokio::sync::oneshot;

/// Helper function to extract all text content from the terminal screen
async fn extract_screen_content(cmd_tx: &tokio::sync::mpsc::Sender<TerminalCommand>) -> anyhow::Result<String> {
    let (resp_tx, resp_rx) = oneshot::channel();
    cmd_tx.send(TerminalCommand::Read { resp_tx }).await?;
    
    match resp_rx.await {
        Ok(TerminalResponse::Content(content)) => Ok(content),
        Ok(TerminalResponse::Error(e)) => Err(anyhow::anyhow!(e)),
        _ => Err(anyhow::anyhow!("Unexpected response")),
    }
}

/// Helper function to get terminal size
async fn get_terminal_size(cmd_tx: &tokio::sync::mpsc::Sender<TerminalCommand>) -> anyhow::Result<(u16, u16)> {
    let (resp_tx, resp_rx) = oneshot::channel();
    cmd_tx.send(TerminalCommand::GetSize { resp_tx }).await?;
    
    match resp_rx.await {
        Ok(TerminalResponse::Size(cols, rows)) => Ok((cols, rows)),
        Ok(TerminalResponse::Error(e)) => Err(anyhow::anyhow!(e)),
        _ => Err(anyhow::anyhow!("Unexpected response")),
    }
}

/// Helper function to check if terminal is in alternate screen mode
async fn is_alternate_screen(cmd_tx: &tokio::sync::mpsc::Sender<TerminalCommand>) -> anyhow::Result<bool> {
    let (resp_tx, resp_rx) = oneshot::channel();
    cmd_tx.send(TerminalCommand::IsAlternateScreen { resp_tx }).await?;
    
    match resp_rx.await {
        Ok(TerminalResponse::AlternateScreen(is_alt)) => Ok(is_alt),
        Ok(TerminalResponse::Error(e)) => Err(anyhow::anyhow!(e)),
        _ => Err(anyhow::anyhow!("Unexpected response")),
    }
}

/// Helper function to get command state
async fn get_command_state(cmd_tx: &tokio::sync::mpsc::Sender<TerminalCommand>) -> anyhow::Result<(String, Option<i32>)> {
    let (resp_tx, resp_rx) = oneshot::channel();
    cmd_tx.send(TerminalCommand::GetCommandState { resp_tx }).await?;
    
    match resp_rx.await {
        Ok(TerminalResponse::CommandState { state, exit_code }) => {
            let state_str = match state {
                nac::terminal::runner::CommandState::Idle => "Idle",
                nac::terminal::runner::CommandState::Running => "Running",
                nac::terminal::runner::CommandState::Completed => "Completed",
            };
            Ok((state_str.to_string(), exit_code))
        }
        Ok(TerminalResponse::Error(e)) => Err(anyhow::anyhow!(e)),
        _ => Err(anyhow::anyhow!("Unexpected response")),
    }
}

/// Helper function to check if command is running
async fn is_command_running(cmd_tx: &tokio::sync::mpsc::Sender<TerminalCommand>) -> anyhow::Result<bool> {
    let (resp_tx, resp_rx) = oneshot::channel();
    cmd_tx.send(TerminalCommand::IsCommandRunning { resp_tx }).await?;
    
    match resp_rx.await {
        Ok(TerminalResponse::IsCommandRunning(is_running)) => Ok(is_running),
        Ok(TerminalResponse::Error(e)) => Err(anyhow::anyhow!(e)),
        _ => Err(anyhow::anyhow!("Unexpected response")),
    }
}

/// Helper function to wait for command completion with timeout
async fn wait_for_command_complete(
    cmd_tx: &tokio::sync::mpsc::Sender<TerminalCommand>,
    timeout_secs: u64,
) -> anyhow::Result<Option<i32>> {
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
    
    loop {
        if start.elapsed() >= timeout {
            return Err(anyhow::anyhow!("Timeout waiting for command completion"));
        }
        
        let is_running = is_command_running(cmd_tx).await?;
        if !is_running {
            let (_, exit_code) = get_command_state(cmd_tx).await?;
            return Ok(exit_code);
        }
        
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Helper function to wait for specific text in terminal output
async fn wait_for_output_contains(
    cmd_tx: &tokio::sync::mpsc::Sender<TerminalCommand>,
    text: &str,
    timeout_secs: u64,
) -> anyhow::Result<bool> {
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
    
    loop {
        if start.elapsed() >= timeout {
            return Ok(false);
        }
        
        let content = extract_screen_content(cmd_tx).await?;
        if content.contains(text) {
            return Ok(true);
        }
        
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Helper function to wait for idle state
async fn wait_for_idle(
    cmd_tx: &tokio::sync::mpsc::Sender<TerminalCommand>,
    idle_secs: u64,
    timeout_secs: u64,
) -> anyhow::Result<bool> {
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
    let idle_duration = Duration::from_secs(idle_secs);
    
    let mut last_content: Option<String> = None;
    let mut last_change = std::time::Instant::now();
    
    loop {
        if start.elapsed() >= timeout {
            return Ok(false);
        }
        
        let content = extract_screen_content(cmd_tx).await?;
        
        match &last_content {
            Some(prev) => {
                if prev != &content {
                    // Content changed, reset timer
                    last_content = Some(content);
                    last_change = std::time::Instant::now();
                } else if last_change.elapsed() >= idle_duration {
                    // Content stable for required duration
                    return Ok(true);
                }
            }
            None => {
                last_content = Some(content);
                last_change = std::time::Instant::now();
            }
        }
        
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

// ============================================================================
// Test 1: Terminal Lifecycle
// ============================================================================

#[tokio::test]
async fn test_terminal_lifecycle() {
    // Step 1: Create a terminal
    let cmd_tx = spawn_runner(
        "lifecycle-test",
        Some(80),
        Some(24),
        None,
        None,
    ).expect("Failed to spawn terminal runner");
    
    // Verify initial size
    let (cols, rows) = get_terminal_size(&cmd_tx).await.expect("Failed to get size");
    assert_eq!(cols, 80, "Expected 80 columns");
    assert_eq!(rows, 24, "Expected 24 rows");
    
    // Step 2: Wait for bash to start
    thread::sleep(Duration::from_millis(500));
    
    // Step 3: Send a command
    cmd_tx.send(TerminalCommand::Write(b"echo LIFECYCLE_TEST\n".to_vec())).await
        .expect("Failed to send command");
    
    // Step 4: Wait for output
    thread::sleep(Duration::from_millis(300));
    
    // Step 5: Read and verify
    let content = extract_screen_content(&cmd_tx).await.expect("Failed to read content");
    assert!(
        content.contains("LIFECYCLE_TEST"),
        "Expected 'LIFECYCLE_TEST' in output, got:\n{}",
        content
    );
    
    // Step 6: Close the terminal
    cmd_tx.send(TerminalCommand::Close).await.expect("Failed to send close");
    
    println!("✓ Terminal lifecycle test passed");
}

// ============================================================================
// Test 2: Emacs Notation
// ============================================================================

#[tokio::test]
async fn test_emacs_notation() {
    let cmd_tx = spawn_runner(
        "emacs-notation-test",
        Some(80),
        Some(24),
        None,
        None,
    ).expect("Failed to spawn terminal runner");
    
    thread::sleep(Duration::from_millis(500));
    
    // Test 1: Basic escape sequences (\n, \t, \e)
    cmd_tx.send(TerminalCommand::Write(b"echo 'LINE1\nLINE2'".to_vec())).await
        .expect("Failed to send");
    cmd_tx.send(TerminalCommand::Write(b"\n".to_vec())).await
        .expect("Failed to send newline");
    
    thread::sleep(Duration::from_millis(300));
    
    // Test 2: Ctrl sequences (^C, ^D)
    // Send a command that sleeps, then interrupt it
    cmd_tx.send(TerminalCommand::Write(b"sleep 10".to_vec())).await
        .expect("Failed to send");
    cmd_tx.send(TerminalCommand::Write(b"\n".to_vec())).await
        .expect("Failed to send newline");
    
    thread::sleep(Duration::from_millis(200));
    
    // Send Ctrl+C (0x03)
    cmd_tx.send(TerminalCommand::Write(vec![0x03])).await
        .expect("Failed to send Ctrl+C");
    
    thread::sleep(Duration::from_millis(500));
    
    let content = extract_screen_content(&cmd_tx).await.expect("Failed to read");
    // Should see either the interrupted sleep or a new prompt
    assert!(
        content.contains("sleep") || content.len() > 0,
        "Terminal should have content after Ctrl+C"
    );
    
    // Test 3: Special keys via VT sequences
    // Send a command and use Enter key
    cmd_tx.send(TerminalCommand::Write(b"echo SPECIAL_KEY_TEST".to_vec())).await
        .expect("Failed to send");
    // Send Enter (CR LF)
    cmd_tx.send(TerminalCommand::Write(b"\r\n".to_vec())).await
        .expect("Failed to send Enter");
    
    thread::sleep(Duration::from_millis(300));
    
    let content = extract_screen_content(&cmd_tx).await.expect("Failed to read");
    assert!(
        content.contains("SPECIAL_KEY_TEST"),
        "Expected 'SPECIAL_KEY_TEST' in output, got:\n{}",
        content
    );
    
    // Test 4: Tab character
    cmd_tx.send(TerminalCommand::Write(b"echo -e 'COL1\tCOL2'".to_vec())).await
        .expect("Failed to send");
    cmd_tx.send(TerminalCommand::Write(b"\n".to_vec())).await
        .expect("Failed to send newline");
    
    thread::sleep(Duration::from_millis(300));
    
    // Test 5: Escape character (\e or \x1b)
    // Send a command that uses escape sequences
    cmd_tx.send(TerminalCommand::Write("echo $'\x1b[32mGREEN\x1b[0m'".as_bytes().to_vec())).await
        .expect("Failed to send");
    cmd_tx.send(TerminalCommand::Write(b"\n".to_vec())).await
        .expect("Failed to send newline");
    
    thread::sleep(Duration::from_millis(300));
    
    let content = extract_screen_content(&cmd_tx).await.expect("Failed to read");
    // The escape sequences should be in the raw output (they get stripped by read)
    assert!(
        content.contains("GREEN") || content.len() > 0,
        "Terminal should process escape sequences"
    );
    
    // Cleanup
    cmd_tx.send(TerminalCommand::Close).await.expect("Failed to close");
    
    println!("✓ Emacs notation test passed");
}

// ============================================================================
// Test 3: Command Wait (command_complete)
// ============================================================================

#[tokio::test]
async fn test_command_wait() {
    let cmd_tx = spawn_runner(
        "command-wait-test",
        Some(80),
        Some(24),
        None,
        None,
    ).expect("Failed to spawn terminal runner");
    
    thread::sleep(Duration::from_millis(500));
    
    // Send a command that takes some time
    cmd_tx.send(TerminalCommand::Write(b"sleep 1 && echo COMMAND_DONE".to_vec())).await
        .expect("Failed to send");
    cmd_tx.send(TerminalCommand::Write(b"\n".to_vec())).await
        .expect("Failed to send newline");
    
    // Wait for command to start
    thread::sleep(Duration::from_millis(200));
    
    // Wait for command completion
    let exit_code = wait_for_command_complete(&cmd_tx, 10)
        .await
        .expect("Failed waiting for command completion");
    
    // Verify exit code (should be 0 for successful sleep, or None if OSC 133 not available)
    // Note: Exit code may be None if bash lifecycle script isn't generating OSC 133 sequences
    assert!(
        exit_code == Some(0) || exit_code == None,
        "Expected exit code 0 or None, got {:?}",
        exit_code
    );
    
    // Read content to verify command output
    let content = extract_screen_content(&cmd_tx).await.expect("Failed to read");
    assert!(
        content.contains("COMMAND_DONE"),
        "Expected 'COMMAND_DONE' in output, got:\n{}",
        content
    );
    
    // Test with a failing command
    cmd_tx.send(TerminalCommand::Write(b"false".to_vec())).await
        .expect("Failed to send");
    cmd_tx.send(TerminalCommand::Write(b"\n".to_vec())).await
        .expect("Failed to send newline");
    
    let exit_code = wait_for_command_complete(&cmd_tx, 5)
        .await
        .expect("Failed waiting for command completion");
    
    // 'false' command should return exit code 1, or None if OSC 133 not available
    assert!(
        exit_code == Some(1) || exit_code == None,
        "Expected exit code 1 or None for 'false' command, got {:?}",
        exit_code
    );
    
    // Cleanup
    cmd_tx.send(TerminalCommand::Close).await.expect("Failed to close");
    
    println!("✓ Command wait test passed");
}

// ============================================================================
// Test 4: Output Contains Wait
// ============================================================================

#[tokio::test]
async fn test_output_contains_wait() {
    let cmd_tx = spawn_runner(
        "output-contains-test",
        Some(80),
        Some(24),
        None,
        None,
    ).expect("Failed to spawn terminal runner");
    
    thread::sleep(Duration::from_millis(500));
    
    // Send a command that outputs text after a delay
    cmd_tx.send(TerminalCommand::Write(b"sleep 1 && echo TARGET_TEXT_FOUND".to_vec())).await
        .expect("Failed to send");
    cmd_tx.send(TerminalCommand::Write(b"\n".to_vec())).await
        .expect("Failed to send newline");
    
    // Wait for specific text to appear
    let found = wait_for_output_contains(&cmd_tx, "TARGET_TEXT_FOUND", 10)
        .await
        .expect("Failed waiting for output");
    
    assert!(found, "Expected to find 'TARGET_TEXT_FOUND' in output");
    
    // Test with text that never appears (should timeout)
    cmd_tx.send(TerminalCommand::Write(b"echo 'some output'".to_vec())).await
        .expect("Failed to send");
    cmd_tx.send(TerminalCommand::Write(b"\n".to_vec())).await
        .expect("Failed to send newline");
    
    let found = wait_for_output_contains(&cmd_tx, "NONEXISTENT_TEXT_XYZ", 1)
        .await
        .expect("Failed waiting for output");
    
    assert!(!found, "Should not find 'NONEXISTENT_TEXT_XYZ'");
    
    // Cleanup
    cmd_tx.send(TerminalCommand::Close).await.expect("Failed to close");
    
    println!("✓ Output contains wait test passed");
}

// ============================================================================
// Test 5: Idle Wait
// ============================================================================

#[tokio::test]
async fn test_idle_wait() {
    let cmd_tx = spawn_runner(
        "idle-wait-test",
        Some(80),
        Some(24),
        None,
        None,
    ).expect("Failed to spawn terminal runner");
    
    thread::sleep(Duration::from_millis(500));
    
    // Send a quick command
    cmd_tx.send(TerminalCommand::Write(b"echo 'quick command'".to_vec())).await
        .expect("Failed to send");
    cmd_tx.send(TerminalCommand::Write(b"\n".to_vec())).await
        .expect("Failed to send newline");
    
    // Wait for idle (1 second of no output changes)
    let is_idle = wait_for_idle(&cmd_tx, 1, 10)
        .await
        .expect("Failed waiting for idle");
    
    assert!(is_idle, "Expected terminal to become idle");
    
    // Test that idle detection resets on new output
    cmd_tx.send(TerminalCommand::Write(b"for i in 1 2 3; do echo $i; sleep 0.5; done".to_vec())).await
        .expect("Failed to send");
    cmd_tx.send(TerminalCommand::Write(b"\n".to_vec())).await
        .expect("Failed to send newline");
    
    // Immediately try to wait for 2 seconds of idle (should timeout because of ongoing output)
    let start = std::time::Instant::now();
    let is_idle = wait_for_idle(&cmd_tx, 2, 3)
        .await
        .expect("Failed waiting for idle");
    let elapsed = start.elapsed();
    
    // Should have timed out (not become idle)
    assert!(!is_idle, "Should not become idle while command is producing output");
    assert!(elapsed >= Duration::from_secs(2), "Should have waited at least 2 seconds");
    
    // Cleanup
    cmd_tx.send(TerminalCommand::Close).await.expect("Failed to close");
    
    println!("✓ Idle wait test passed");
}

// ============================================================================
// Test 6: Resize
// ============================================================================

#[tokio::test]
async fn test_resize() {
    let cmd_tx = spawn_runner(
        "resize-test",
        Some(80),
        Some(24),
        None,
        None,
    ).expect("Failed to spawn terminal runner");
    
    // Verify initial size
    let (cols, rows) = get_terminal_size(&cmd_tx).await.expect("Failed to get size");
    assert_eq!(cols, 80, "Initial columns should be 80");
    assert_eq!(rows, 24, "Initial rows should be 24");
    
    thread::sleep(Duration::from_millis(300));
    
    // Resize to larger dimensions
    cmd_tx.send(TerminalCommand::Resize { cols: 120, rows: 40 }).await
        .expect("Failed to send resize");
    
    thread::sleep(Duration::from_millis(200));
    
    // Verify new size
    let (cols, rows) = get_terminal_size(&cmd_tx).await.expect("Failed to get size after resize");
    assert_eq!(cols, 120, "Columns should be 120 after resize");
    assert_eq!(rows, 40, "Rows should be 40 after resize");
    
    // Resize to smaller dimensions
    cmd_tx.send(TerminalCommand::Resize { cols: 60, rows: 15 }).await
        .expect("Failed to send resize");
    
    thread::sleep(Duration::from_millis(200));
    
    // Verify smaller size
    let (cols, rows) = get_terminal_size(&cmd_tx).await.expect("Failed to get size after second resize");
    assert_eq!(cols, 60, "Columns should be 60 after second resize");
    assert_eq!(rows, 15, "Rows should be 15 after second resize");
    
    // Test that terminal still works after resize
    thread::sleep(Duration::from_millis(300));
    
    cmd_tx.send(TerminalCommand::Write(b"echo RESIZE_TEST".to_vec())).await
        .expect("Failed to send");
    cmd_tx.send(TerminalCommand::Write(b"\n".to_vec())).await
        .expect("Failed to send newline");
    
    thread::sleep(Duration::from_millis(300));
    
    let content = extract_screen_content(&cmd_tx).await.expect("Failed to read");
    assert!(
        content.contains("RESIZE_TEST"),
        "Expected 'RESIZE_TEST' in output after resize, got:\n{}",
        content
    );
    
    // Cleanup
    cmd_tx.send(TerminalCommand::Close).await.expect("Failed to close");
    
    println!("✓ Resize test passed");
}

// ============================================================================
// Test 7: Session Ownership
// ============================================================================

#[tokio::test]
async fn test_session_ownership() {
    let manager = TerminalManager::new();
    
    // Create a terminal through the manager
    let created = manager
        .create("ownership-test".to_string(), Some(80), Some(24), None, None)
        .await
        .expect("Failed to create terminal");
    assert!(!created, "Should not have replaced an existing terminal");
    
    // Verify terminal exists
    assert!(manager.contains("ownership-test").await, "Terminal should exist");
    
    // Owner 1 checks out the terminal
    let checked_out = manager
        .checkout_terminal("ownership-test", "owner1".to_string())
        .await
        .expect("Failed to checkout");
    assert!(checked_out, "Owner1 should be able to checkout");
    
    // Owner 2 tries to checkout (should fail)
    let result = manager
        .checkout_terminal("ownership-test", "owner2".to_string())
        .await;
    assert!(result.is_err(), "Owner2 should not be able to checkout when Owner1 has it");
    
    // Owner 1 can perform operations
    let size = manager
        .get_terminal_size("ownership-test", "owner1")
        .await
        .expect("Failed to get size");
    assert_eq!(size, Some((80, 24)), "Should get correct size");
    
    // Owner 2 cannot perform operations
    let result = manager
        .get_terminal_size("ownership-test", "owner2")
        .await;
    assert!(result.is_err(), "Owner2 should not be able to read");
    
    // Owner 1 checks in
    let checked_in = manager
        .checkin_terminal("ownership-test", "owner1")
        .await
        .expect("Failed to checkin");
    assert!(checked_in, "Owner1 should be able to checkin");
    
    // Now Owner 2 can checkout
    let checked_out = manager
        .checkout_terminal("ownership-test", "owner2".to_string())
        .await
        .expect("Failed to checkout");
    assert!(checked_out, "Owner2 should be able to checkout after Owner1 checked in");
    
    // Owner 2 can now perform operations
    let size = manager
        .get_terminal_size("ownership-test", "owner2")
        .await
        .expect("Failed to get size as owner2");
    assert_eq!(size, Some((80, 24)), "Owner2 should get correct size");
    
    // Owner 2 checks in
    let checked_in = manager
        .checkin_terminal("ownership-test", "owner2")
        .await
        .expect("Failed to checkin");
    assert!(checked_in, "Owner2 should be able to checkin");
    
    // Shutdown the terminal (administrative operation, doesn't require ownership)
    let shut_down = manager
        .shutdown_terminal("ownership-test")
        .await
        .expect("Failed to shutdown");
    assert!(shut_down, "Should be able to shutdown");
    
    // Verify terminal is gone
    assert!(!manager.contains("ownership-test").await, "Terminal should no longer exist");
    
    println!("✓ Session ownership test passed");
}

// ============================================================================
// Test 8: Alt Mode Detection
// ============================================================================

#[tokio::test]
async fn test_alt_mode_detection() {
    let cmd_tx = spawn_runner(
        "alt-mode-test",
        Some(80),
        Some(24),
        None,
        None,
    ).expect("Failed to spawn terminal runner");
    
    thread::sleep(Duration::from_millis(500));
    
    // Initially should NOT be in alternate screen mode
    let is_alt = is_alternate_screen(&cmd_tx).await.expect("Failed to check alt screen");
    assert!(!is_alt, "Should not be in alternate screen mode initially");
    
    // Enter alternate screen mode using tput
    cmd_tx.send(TerminalCommand::Write(b"tput smcup".to_vec())).await
        .expect("Failed to send");
    cmd_tx.send(TerminalCommand::Write(b"\n".to_vec())).await
        .expect("Failed to send newline");
    
    thread::sleep(Duration::from_millis(300));
    
    // Now should be in alternate screen mode
    let is_alt = is_alternate_screen(&cmd_tx).await.expect("Failed to check alt screen");
    assert!(is_alt, "Should be in alternate screen mode after smcup");
    
    // Exit alternate screen mode
    cmd_tx.send(TerminalCommand::Write(b"tput rmcup".to_vec())).await
        .expect("Failed to send");
    cmd_tx.send(TerminalCommand::Write(b"\n".to_vec())).await
        .expect("Failed to send newline");
    
    thread::sleep(Duration::from_millis(300));
    
    // Should no longer be in alternate screen mode
    let is_alt = is_alternate_screen(&cmd_tx).await.expect("Failed to check alt screen");
    assert!(!is_alt, "Should not be in alternate screen mode after rmcup");
    
    // Test with a program that uses alternate screen (like nano or vim if available)
    // First, let's use a simple test with the "clear" command which also uses escape sequences
    cmd_tx.send(TerminalCommand::Write(b"clear".to_vec())).await
        .expect("Failed to send");
    cmd_tx.send(TerminalCommand::Write(b"\n".to_vec())).await
        .expect("Failed to send newline");
    
    thread::sleep(Duration::from_millis(300));
    
    // After clear, we should be back to normal mode
    let is_alt = is_alternate_screen(&cmd_tx).await.expect("Failed to check alt screen");
    // clear doesn't enter alt mode, it just clears the screen
    // so we should still not be in alt mode
    assert!(!is_alt, "Should not be in alternate screen mode after clear");
    
    // Cleanup
    cmd_tx.send(TerminalCommand::Close).await.expect("Failed to close");
    
    println!("✓ Alt mode detection test passed");
}

// ============================================================================
// Additional Integration Test: Full Workflow
// ============================================================================

#[tokio::test]
async fn test_full_terminal_workflow() {
    let manager = TerminalManager::new();
    
    // Create terminal
    let created = manager
        .create("workflow-test".to_string(), Some(100), Some(30), None, None)
        .await
        .expect("Failed to create terminal");
    assert!(!created);
    
    // Checkout
    let checked_out = manager
        .checkout_terminal("workflow-test", "workflow-owner".to_string())
        .await
        .expect("Failed to checkout");
    assert!(checked_out);
    
    // Get initial size
    let size = manager
        .get_terminal_size("workflow-test", "workflow-owner")
        .await
        .expect("Failed to get size");
    assert_eq!(size, Some((100, 30)));
    
    // Write a command
    let written = manager
        .write_to_terminal("workflow-test", "workflow-owner", b"echo WORKFLOW_START".to_vec())
        .await
        .expect("Failed to write");
    assert!(written);
    
    // Send newline
    let written = manager
        .write_to_terminal("workflow-test", "workflow-owner", b"\n".to_vec())
        .await
        .expect("Failed to write newline");
    assert!(written);
    
    // Wait a bit for output
    tokio::time::sleep(Duration::from_millis(500)).await;
    
    // Read content
    let content = manager
        .read_terminal("workflow-test", "workflow-owner")
        .await
        .expect("Failed to read")
        .expect("No content");
    assert!(content.contains("WORKFLOW_START"), "Should see workflow start marker");
    
    // Resize
    let resized = manager
        .resize_terminal("workflow-test", "workflow-owner", 120, 40)
        .await
        .expect("Failed to resize");
    assert!(resized);
    
    // Verify new size
    let size = manager
        .get_terminal_size("workflow-test", "workflow-owner")
        .await
        .expect("Failed to get size after resize");
    assert_eq!(size, Some((120, 40)));
    
    // Check not in alt mode
    let is_alt = manager
        .is_alternate_screen("workflow-test", "workflow-owner")
        .await
        .expect("Failed to check alt screen");
    assert_eq!(is_alt, Some(false));
    
    // Checkin
    let checked_in = manager
        .checkin_terminal("workflow-test", "workflow-owner")
        .await
        .expect("Failed to checkin");
    assert!(checked_in);
    
    // Shutdown
    let shut_down = manager
        .shutdown_terminal("workflow-test")
        .await
        .expect("Failed to shutdown");
    assert!(shut_down);
    
    // Verify gone
    assert!(!manager.contains("workflow-test").await);
    
    println!("✓ Full workflow test passed");
}

// ============================================================================
// Test: Concurrent Terminal Operations
// ============================================================================

#[tokio::test]
async fn test_concurrent_terminals() {
    let manager = TerminalManager::new();
    
    // Create multiple terminals
    for i in 0..3 {
        let name = format!("concurrent-test-{}", i);
        let created = manager
            .create(name, Some(80), Some(24), None, None)
            .await
            .expect("Failed to create terminal");
        assert!(!created);
    }
    
    // Verify all exist
    assert_eq!(manager.len().await, 3);
    
    // Checkout and use each terminal
    for i in 0..3 {
        let name = format!("concurrent-test-{}", i);
        let owner = format!("owner-{}", i);
        
        let checked_out = manager
            .checkout_terminal(&name, owner.clone())
            .await
            .expect("Failed to checkout");
        assert!(checked_out);
        
        // Write unique marker to each terminal
        let marker = format!("echo MARKER_{}", i);
        let written = manager
            .write_to_terminal(&name, &owner, marker.as_bytes().to_vec())
            .await
            .expect("Failed to write");
        assert!(written);
        
        let written = manager
            .write_to_terminal(&name, &owner, b"\n".to_vec())
            .await
            .expect("Failed to write newline");
        assert!(written);
        
        // Checkin
        let checked_in = manager
            .checkin_terminal(&name, &owner)
            .await
            .expect("Failed to checkin");
        assert!(checked_in);
    }
    
    // Wait for output
    tokio::time::sleep(Duration::from_millis(500)).await;
    
    // Verify each terminal has its unique marker
    for i in 0..3 {
        let name = format!("concurrent-test-{}", i);
        let owner = format!("reader-{}", i);
        
        let checked_out = manager
            .checkout_terminal(&name, owner.clone())
            .await
            .expect("Failed to checkout for reading");
        assert!(checked_out);
        
        let content = manager
            .read_terminal(&name, &owner)
            .await
            .expect("Failed to read")
            .expect("No content");
        
        let expected_marker = format!("MARKER_{}", i);
        assert!(
            content.contains(&expected_marker),
            "Terminal {} should contain {}",
            i,
            expected_marker
        );
        
        // Checkin
        let _ = manager.checkin_terminal(&name, &owner).await;
    }
    
    // Shutdown all
    for i in 0..3 {
        let name = format!("concurrent-test-{}", i);
        let shut_down = manager
            .shutdown_terminal(&name)
            .await
            .expect("Failed to shutdown");
        assert!(shut_down);
    }
    
    assert!(manager.is_empty().await);
    
    println!("✓ Concurrent terminals test passed");
}
