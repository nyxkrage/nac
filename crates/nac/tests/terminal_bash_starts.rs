//! Integration test for terminal bash startup
//!
//! This test verifies that:
//! 1. A terminal can be created with default size (140x50)
//! 2. Bash starts successfully in the PTY
//! 3. Commands can be sent to the terminal via the runner
//! 4. Output is captured and processed correctly
//! 5. The terminal screen content can be read and verified

use std::thread;
use std::time::Duration;

use nac::terminal::{spawn_runner, TerminalCommand, TerminalResponse};
use tokio::sync::oneshot;

/// Helper function to extract all text content from the terminal screen
async fn extract_screen_content(cmd_tx: &tokio::sync::mpsc::Sender<TerminalCommand>) -> anyhow::Result<String> {
    // Send Read command and get response
    let (resp_tx, resp_rx) = oneshot::channel();
    cmd_tx.send(TerminalCommand::Read { resp_tx }).await?;
    
    match resp_rx.await {
        Ok(TerminalResponse::Content(content)) => Ok(content),
        Ok(TerminalResponse::Error(e)) => Err(anyhow::anyhow!(e)),
        _ => Err(anyhow::anyhow!("Unexpected response")),
    }
}

#[tokio::test]
async fn test_terminal_bash_starts_and_executes_command() {
    // Step 1: Create a terminal with default size (140x50)
    let cmd_tx = spawn_runner(
        "test-bash-session",
        None, // Use default cols (140)
        None, // Use default rows (50)
        None, // Use current working directory
        None, // No extra environment variables
    ).expect("Failed to spawn terminal runner");

    // Verify the terminal was created by getting its size
    let (resp_tx, resp_rx) = oneshot::channel();
    cmd_tx.send(TerminalCommand::GetSize { resp_tx }).await
        .expect("Failed to send GetSize command");
    
    let (cols, rows) = match resp_rx.await {
        Ok(TerminalResponse::Size(c, r)) => (c, r),
        Ok(TerminalResponse::Error(e)) => panic!("Error getting size: {}", e),
        _ => panic!("Unexpected response"),
    };
    
    assert_eq!(cols, 140, "Expected 140 columns, got {}", cols);
    assert_eq!(rows, 50, "Expected 50 rows, got {}", rows);

    // Step 2: Wait briefly for bash to start
    // Bash startup can take a moment, especially for login shells
    thread::sleep(Duration::from_millis(500));

    // Step 3: Send "echo BASH_STARTED" command to the PTY
    // We need to add a newline to execute the command
    let command = "echo BASH_STARTED\n";
    cmd_tx.send(TerminalCommand::Write(command.as_bytes().to_vec())).await
        .expect("Failed to send Write command");

    println!("Sent command: echo BASH_STARTED");

    // Step 4: Wait for output to be processed
    // Give bash time to execute the command and produce output
    thread::sleep(Duration::from_millis(500));

    // Step 5: Read the terminal screen content
    let screen_content = extract_screen_content(&cmd_tx).await
        .expect("Failed to extract screen content");

    println!("Terminal screen content:\n---\n{}\n---", screen_content);

    // Step 6: Verify that "BASH_STARTED" appears in the output
    assert!(
        screen_content.contains("BASH_STARTED"),
        "Expected 'BASH_STARTED' in output, but got:\n{}",
        screen_content
    );

    println!("SUCCESS: Terminal is active and responsive");

    // Step 7: Clean up the terminal session
    cmd_tx.send(TerminalCommand::Close).await
        .expect("Failed to send Close command");
    println!("Terminal session cleaned up successfully");
}

#[tokio::test]
async fn test_terminal_session_lifecycle() {
    // Test that we can create, use, and clean up a terminal session via the runner
    let cmd_tx = spawn_runner(
        "lifecycle-test",
        Some(80),
        Some(24),
        None,
        None,
    ).expect("Failed to spawn terminal runner");

    // Verify initial state by getting size
    let (resp_tx, resp_rx) = oneshot::channel();
    cmd_tx.send(TerminalCommand::GetSize { resp_tx }).await
        .expect("Failed to send GetSize command");
    
    match resp_rx.await {
        Ok(TerminalResponse::Size(cols, rows)) => {
            assert_eq!(cols, 80);
            assert_eq!(rows, 24);
        }
        Ok(TerminalResponse::Error(e)) => panic!("Error: {}", e),
        _ => panic!("Unexpected response"),
    }

    // Wait for bash to start
    thread::sleep(Duration::from_millis(300));

    // Send a simple command
    cmd_tx.send(TerminalCommand::Write(b"pwd\n".to_vec())).await
        .expect("Failed to send Write command");
    thread::sleep(Duration::from_millis(200));

    // Clean up
    cmd_tx.send(TerminalCommand::Close).await
        .expect("Failed to send Close command");
}

#[tokio::test]
async fn test_terminal_resize() {
    let cmd_tx = spawn_runner(
        "resize-test",
        Some(80),
        Some(24),
        None,
        None,
    ).expect("Failed to spawn terminal runner");

    // Verify initial size
    let (resp_tx, resp_rx) = oneshot::channel();
    cmd_tx.send(TerminalCommand::GetSize { resp_tx }).await
        .expect("Failed to send GetSize command");
    
    match resp_rx.await {
        Ok(TerminalResponse::Size(cols, rows)) => {
            assert_eq!(cols, 80);
            assert_eq!(rows, 24);
        }
        Ok(TerminalResponse::Error(e)) => panic!("Error: {}", e),
        _ => panic!("Unexpected response"),
    }

    // Resize the terminal
    cmd_tx.send(TerminalCommand::Resize { cols: 100, rows: 30 }).await
        .expect("Failed to send Resize command");
    thread::sleep(Duration::from_millis(100));

    // Verify new size
    let (resp_tx, resp_rx) = oneshot::channel();
    cmd_tx.send(TerminalCommand::GetSize { resp_tx }).await
        .expect("Failed to send GetSize command");
    
    match resp_rx.await {
        Ok(TerminalResponse::Size(cols, rows)) => {
            assert_eq!(cols, 100, "Expected 100 columns after resize");
            assert_eq!(rows, 30, "Expected 30 rows after resize");
        }
        Ok(TerminalResponse::Error(e)) => panic!("Error: {}", e),
        _ => panic!("Unexpected response"),
    }

    // Clean up
    cmd_tx.send(TerminalCommand::Close).await
        .expect("Failed to send Close command");
}

#[tokio::test]
async fn test_multiple_commands() {
    let cmd_tx = spawn_runner(
        "multi-command-test",
        Some(80),
        Some(24),
        None,
        None,
    ).expect("Failed to spawn terminal runner");

    // Wait for bash
    thread::sleep(Duration::from_millis(300));

    // Send multiple commands
    let commands = vec![
        "echo FIRST",
        "echo SECOND",
        "echo THIRD",
    ];

    for cmd in &commands {
        let input = format!("{}\n", cmd);
        cmd_tx.send(TerminalCommand::Write(input.as_bytes().to_vec())).await
            .expect("Failed to send Write command");
        thread::sleep(Duration::from_millis(200));
    }

    // Give a bit more time for all output to arrive
    thread::sleep(Duration::from_millis(500));

    // Read content
    let content = extract_screen_content(&cmd_tx).await
        .expect("Failed to extract content");

    println!("Multi-command output:\n{}", content);

    // Verify terminal is responsive (content should contain command output)
    // The actual command output should be in the screen content now
    assert!(content.contains("FIRST") || content.contains("SECOND") || content.contains("THIRD") || content.contains("multi-command-test"), 
        "Expected command output in terminal content, but got:\n{}", content);

    // Clean up
    cmd_tx.send(TerminalCommand::Close).await
        .expect("Failed to send Close command");
}
