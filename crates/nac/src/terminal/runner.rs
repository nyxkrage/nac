use anyhow::{Context, Result};
use libghostty_vt::{Terminal, TerminalOptions};
use portable_pty::{CommandBuilder, NativePtySystem, PtyPair, PtySize, PtySystem};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc::{channel, Receiver};
use std::thread::{self, JoinHandle};
use std::time::Instant;
use tokio::sync::{mpsc, oneshot};

/// Commands that can be sent to the terminal runner thread
#[derive(Debug)]
pub enum TerminalCommand {
    /// Write data to the PTY
    Write(Vec<u8>),
    /// Resize the terminal
    Resize { cols: u16, rows: u16 },
    /// Read terminal content (response sent via oneshot)
    Read {
        resp_tx: oneshot::Sender<TerminalResponse>,
    },
    /// Get terminal dimensions (response sent via oneshot)
    GetSize {
        resp_tx: oneshot::Sender<TerminalResponse>,
    },
    /// Check if terminal is in alternate screen mode (response sent via oneshot)
    IsAlternateScreen {
        resp_tx: oneshot::Sender<TerminalResponse>,
    },
    /// Shutdown the terminal session
    Close,
}

/// Responses from the terminal runner thread
#[derive(Debug, Clone)]
pub enum TerminalResponse {
    /// Terminal content as string
    Content(String),
    /// Terminal dimensions
    Size(u16, u16),
    /// Alternate screen status (true = alternate screen, false = primary screen)
    AlternateScreen(bool),
    /// Acknowledgment
    Ack,
    /// Error message
    Error(String),
}

/// Data sent from the PTY reader thread to be processed by the terminal
#[derive(Debug)]
pub struct PtyOutput {
    pub data: Vec<u8>,
}

/// A terminal session that wraps a libghostty_vt::Terminal and a PTY.
/// Manages the lifecycle of a shell process with VT100 emulation.
///
/// # Safety
/// This struct is not Send because the underlying Terminal type contains raw pointers.
/// It must be created and used on the same thread.
pub struct TerminalSession {
    /// Human-readable name for this session
    pub name: String,

    /// The terminal emulator state
    pub terminal: Terminal<'static, 'static>,

    /// The PTY pair (master/slave)
    pty_pair: PtyPair,

    /// Channel receiver for PTY output data
    pty_receiver: Receiver<PtyOutput>,

    /// Handle to the reader thread (not async - uses std::thread)
    reader_thread: Option<JoinHandle<()>>,

    /// When the session was created
    pub created_at: Instant,

    /// The PTY writer for sending input to the shell
    writer: Box<dyn Write + Send>,
}

/// TerminalRunner owns a TerminalSession in a dedicated thread.
/// It listens for commands via an mpsc channel and handles them.
pub struct TerminalRunner {
    /// The terminal session (only valid on this thread)
    session: TerminalSession,
    /// Command receiver
    cmd_rx: mpsc::Receiver<TerminalCommand>,
}

impl TerminalRunner {
    /// Create a new TerminalRunner with the given session and command receiver.
    pub fn new(session: TerminalSession, cmd_rx: mpsc::Receiver<TerminalCommand>) -> Self {
        Self { session, cmd_rx }
    }

    /// Run the event loop, processing commands until Close is received.
    pub fn run(mut self) {
        let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
        rt.block_on(async {
            loop {
                tokio::select! {
                    // Process commands from the channel
                    Some(cmd) = self.cmd_rx.recv() => {
                        match cmd {
                            TerminalCommand::Write(data) => {
                                if let Err(e) = self.session.write_to_pty(&data) {
                                    eprintln!("Error writing to PTY: {}", e);
                                }
                            }
                            TerminalCommand::Resize { cols, rows } => {
                                if let Err(e) = self.session.resize(cols, rows) {
                                    eprintln!("Error resizing terminal: {}", e);
                                }
                            }
                            TerminalCommand::Read { resp_tx } => {
                                // Process pending PTY output first to ensure terminal is up to date
                                let _ = self.session.process_output();
                                let content = self.session.read_screen();
                                let _ = resp_tx.send(TerminalResponse::Content(content));
                            }
                            TerminalCommand::GetSize { resp_tx } => {
                                match self.session.size() {
                                    Ok((cols, rows)) => {
                                        let _ = resp_tx.send(TerminalResponse::Size(cols, rows));
                                    }
                                    Err(e) => {
                                        let _ = resp_tx.send(TerminalResponse::Error(e.to_string()));
                                    }
                                }
                            }
                            TerminalCommand::IsAlternateScreen { resp_tx } => {
                                let is_alt = self.session.is_alternate_screen();
                                let _ = resp_tx.send(TerminalResponse::AlternateScreen(is_alt));
                            }
                            TerminalCommand::Close => {
                                // Drop the session to clean up
                                drop(self.session);
                                break;
                            }
                        }
                    }
                    // Periodically process output even without commands
                    _ = tokio::time::sleep(tokio::time::Duration::from_millis(10)) => {
                        let _ = self.session.process_output();
                    }
                }
            }
        });
    }
}

/// Spawn a terminal runner in a dedicated thread.
/// Returns the command sender that can be used to control the terminal.
///
/// # Arguments
/// * `name` - Human-readable name for the session
/// * `cols` - Terminal width in columns (default: 140)
/// * `rows` - Terminal height in rows (default: 50)
/// * `cwd` - Working directory for the shell (default: current dir)
/// * `env` - Additional environment variables to set
///
/// # Returns
/// A `mpsc::Sender<TerminalCommand>` that can be used to control the terminal session.
pub fn spawn_runner(
    name: impl Into<String>,
    cols: Option<usize>,
    rows: Option<usize>,
    cwd: Option<&Path>,
    env: Option<HashMap<String, String>>,
) -> Result<mpsc::Sender<TerminalCommand>> {
    let name = name.into();
    let cols = cols.unwrap_or(140);
    let rows = rows.unwrap_or(50);

    // Convert cwd to owned PathBuf to avoid lifetime issues
    let cwd_owned: Option<std::path::PathBuf> = cwd.map(|p| p.to_path_buf());

    // Create command channel
    let (cmd_tx, cmd_rx) = mpsc::channel::<TerminalCommand>(32);

    // Spawn the terminal session in a dedicated thread
    let name_clone = name.clone();
    thread::spawn(move || {
        // Create the terminal session in this thread
        let cwd_ref = cwd_owned.as_deref();
        let session = match TerminalSession::spawn(&name_clone, Some(cols), Some(rows), cwd_ref, env) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to spawn terminal session '{}': {}", name_clone, e);
                return;
            }
        };

        // Create and run the runner
        let runner = TerminalRunner::new(session, cmd_rx);
        runner.run();
    });

    Ok(cmd_tx)
}

impl TerminalSession {
    /// Spawn a new terminal session with the given configuration.
    ///
    /// # Arguments
    /// * `name` - Human-readable name for the session
    /// * `cols` - Terminal width in columns (default: 140)
    /// * `rows` - Terminal height in rows (default: 50)
    /// * `cwd` - Working directory for the shell (default: current dir)
    /// * `env` - Additional environment variables to set
    ///
    /// # Returns
    /// A new TerminalSession with the PTY and reader thread started.
    /// The caller must call `process_output()` periodically to feed
    /// PTY data into the terminal.
    pub fn spawn(
        name: impl Into<String>,
        cols: Option<usize>,
        rows: Option<usize>,
        cwd: Option<&Path>,
        env: Option<HashMap<String, String>>,
    ) -> Result<Self> {
        let name = name.into();
        let cols = cols.unwrap_or(140);
        let rows = rows.unwrap_or(50);

        // Create the terminal emulator
        let terminal = Terminal::new(TerminalOptions {
            cols: cols as u16,
            rows: rows as u16,
            max_scrollback: 10000,
        })
        .context("Failed to create terminal emulator")?;

        // Create the PTY system
        let pty_system = NativePtySystem::default();

        // Create the PTY pair with the specified size
        let pty_pair = pty_system
            .openpty(PtySize {
                rows: rows as u16,
                cols: cols as u16,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to open PTY")?;

        // Build the command (bash)
        let mut cmd = CommandBuilder::new("bash");
        cmd.arg("--login");

        // Set working directory
        if let Some(cwd) = cwd {
            cmd.cwd(cwd);
        }

        // Set environment variables
        if let Some(env_vars) = env {
            for (key, value) in env_vars {
                cmd.env(key, value);
            }
        }

        // Spawn the shell in the PTY slave
        let _child = pty_pair
            .slave
            .spawn_command(cmd)
            .context("Failed to spawn shell in PTY")?;

        // Create channel for PTY output
        let (tx, rx) = channel::<PtyOutput>();

        // Take the master for reading
        let mut reader = pty_pair
            .master
            .try_clone_reader()
            .context("Failed to clone PTY master reader")?;

        // Take the writer for writing (do this once and store it)
        let writer = pty_pair
            .master
            .take_writer()
            .context("Failed to get PTY writer")?;

        // Start the reader thread (std::thread, not tokio - blocking I/O)
        let reader_thread = thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        // EOF - PTY closed
                        break;
                    }
                    Ok(n) => {
                        let data = buffer[..n].to_vec();
                        if tx.send(PtyOutput { data }).is_err() {
                            // Receiver dropped, exit thread
                            break;
                        }
                    }
                    Err(e) => {
                        eprintln!("Error reading from PTY: {}", e);
                        break;
                    }
                }
            }
        });

        Ok(Self {
            name,
            terminal,
            pty_pair,
            pty_receiver: rx,
            reader_thread: Some(reader_thread),
            created_at: Instant::now(),
            writer,
        })
    }

    /// Process pending PTY output and feed it to the terminal.
    /// Call this periodically (e.g., in an async task or event loop).
    /// Returns the number of output chunks processed.
    pub fn process_output(&mut self) -> Result<usize> {
        let mut count = 0;

        // Drain all available output from the channel
        while let Ok(output) = self.pty_receiver.try_recv() {
            self.terminal.vt_write(&output.data);
            count += 1;
        }

        Ok(count)
    }

    /// Write data to the PTY (sends input to the shell)
    pub fn write_to_pty(&mut self, data: &[u8]) -> Result<()> {
        self.writer.write_all(data).context("Failed to write to PTY")?;
        Ok(())
    }

    /// Get the terminal dimensions
    pub fn size(&self) -> Result<(u16, u16)> {
        Ok((self.terminal.cols()?, self.terminal.rows()?))
    }

    /// Check if the terminal is in alternate screen mode
    pub fn is_alternate_screen(&self) -> bool {
        match self.terminal.active_screen() {
            Ok(screen) => screen == libghostty_vt::ffi::GhosttyTerminalScreen_GHOSTTY_TERMINAL_SCREEN_ALTERNATE,
            Err(_) => false,
        }
    }

    /// Read the current terminal screen content as a string
    /// Uses RenderState, RowIterator, and CellIterator to extract all text from the terminal.
    pub fn read_screen(&self) -> String {
        use libghostty_vt::RenderState;
        use libghostty_vt::render::{RowIterator, CellIterator};
        
        // Create render state and iterators (reusable across calls)
        let mut render_state = match RenderState::new() {
            Ok(rs) => rs,
            Err(_) => return String::from("[Error: Failed to create RenderState]"),
        };
        let mut rows = match RowIterator::new() {
            Ok(r) => r,
            Err(_) => return String::from("[Error: Failed to create RowIterator]"),
        };
        let mut cells = match CellIterator::new() {
            Ok(c) => c,
            Err(_) => return String::from("[Error: Failed to create CellIterator]"),
        };
        
        // Update render state from terminal
        let snapshot = match render_state.update(&self.terminal) {
            Ok(s) => s,
            Err(_) => return String::from("[Error: Failed to update RenderState]"),
        };
        
        // Get terminal dimensions
        let cols = snapshot.cols().unwrap_or(80) as usize;
        let rows_count = snapshot.rows().unwrap_or(24) as usize;
        
        // Iterate over all rows and cells to build the screen content
        let mut lines: Vec<String> = Vec::with_capacity(rows_count);
        let mut current_line = String::with_capacity(cols);
        
        match rows.update(&snapshot) {
            Ok(mut row_iter) => {
                while let Some(row) = row_iter.next() {
                    current_line.clear();
                    
                    // Iterate cells in this row
                    match cells.update(&row) {
                        Ok(mut cell_iter) => {
                            while let Some(cell) = cell_iter.next() {
                                // Get graphemes from cell and append to line
                                match cell.graphemes() {
                                    Ok(graphemes) if !graphemes.is_empty() => {
                                        for g in graphemes {
                                            current_line.push(g);
                                        }
                                    }
                                    _ => {
                                        // Empty cell - add space to preserve layout
                                        current_line.push(' ');
                                    }
                                }
                            }
                        }
                        Err(_) => {
                            // Fill with spaces if cell iteration fails
                            current_line.push_str(&" ".repeat(cols));
                        }
                    }
                    
                    // Trim trailing whitespace but preserve empty lines
                    let trimmed = current_line.trim_end();
                    if trimmed.is_empty() && current_line.len() >= cols {
                        lines.push(String::new());
                    } else {
                        lines.push(trimmed.to_string());
                    }
                }
            }
            Err(_) => {
                return String::from("[Error: Failed to iterate rows]");
            }
        }
        
        // Join lines with newlines
        lines.join("\n")
    }

    /// Resize the terminal and PTY
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        // Resize the terminal emulator
        self.terminal.resize(cols, rows, 0, 0)?;

        // Resize the PTY
        self.pty_pair
            .master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to resize PTY")?;

        Ok(())
    }

    /// Check if the reader thread is still running
    pub fn is_running(&self) -> bool {
        self.reader_thread
            .as_ref()
            .map(|t| !t.is_finished())
            .unwrap_or(false)
    }

    /// Get the age of the session
    pub fn age(&self) -> std::time::Duration {
        self.created_at.elapsed()
    }

    /// Get a reference to the underlying terminal
    pub fn terminal(&self) -> &Terminal<'static, 'static> {
        &self.terminal
    }

    /// Get a mutable reference to the underlying terminal
    pub fn terminal_mut(&mut self) -> &mut Terminal<'static, 'static> {
        &mut self.terminal
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        // Drop the receiver first to signal the thread to exit
        // (the channel is disconnected, so send will fail)

        // Join the reader thread
        if let Some(thread) = self.reader_thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_terminal_session_creation() {
        let mut session = TerminalSession::spawn("test-session", Some(80), Some(24), None, None)
            .expect("Failed to create terminal session");

        assert_eq!(session.name, "test-session");

        // Check dimensions
        let (cols, rows) = session.size().expect("Failed to get size");
        assert_eq!(cols, 80);
        assert_eq!(rows, 24);

        // Process any initial output
        let processed = session.process_output().expect("Failed to process output");
        // May be 0 or more depending on bash startup
        println!("Processed {} output chunks", processed);

        // Clean up
        drop(session);
    }

    #[test]
    fn test_terminal_session_defaults() {
        let session = TerminalSession::spawn(
            "default-session",
            None, // Use defaults
            None,
            None,
            None,
        )
        .expect("Failed to create terminal session");

        let (cols, rows) = session.size().expect("Failed to get size");
        assert_eq!(cols, 140);
        assert_eq!(rows, 50);

        drop(session);
    }

    #[tokio::test]
    async fn test_terminal_runner() {
        let cmd_tx = spawn_runner("test-runner", Some(80), Some(24), None, None)
            .expect("Failed to spawn terminal runner");

        // Get size using oneshot channel
        let (resp_tx, resp_rx) = oneshot::channel();
        cmd_tx
            .send(TerminalCommand::GetSize { resp_tx })
            .await
            .expect("Failed to send command");

        match resp_rx.await {
            Ok(TerminalResponse::Size(cols, rows)) => {
                assert_eq!(cols, 80);
                assert_eq!(rows, 24);
            }
            Ok(TerminalResponse::Error(e)) => panic!("Got error: {}", e),
            _ => panic!("Unexpected response"),
        }

        // Shutdown
        cmd_tx
            .send(TerminalCommand::Close)
            .await
            .expect("Failed to send close command");
    }
}
