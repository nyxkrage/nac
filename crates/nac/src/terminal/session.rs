use anyhow::{Context, Result};
use libghostty_vt::{Terminal, TerminalOptions};
use portable_pty::{CommandBuilder, NativePtySystem, PtyPair, PtySize, PtySystem};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver};
use std::thread::{self, JoinHandle};
use std::time::Instant;
use tokio::sync::mpsc;

/// Commands that can be sent to the terminal runner thread
#[derive(Debug)]
pub enum TerminalCommand {
    /// Write data to the PTY
    Write(Vec<u8>),
    /// Resize the terminal
    Resize { cols: u16, rows: u16 },
    /// Process pending output and return the count of chunks processed
    ProcessOutput,
    /// Get terminal dimensions (response sent back via oneshot)
    GetSize,
    /// Shutdown the terminal session
    Shutdown,
}

/// Responses from the terminal runner thread
#[derive(Debug)]
pub enum TerminalResponse {
    /// Output processed count
    OutputProcessed(usize),
    /// Terminal dimensions
    Size(u16, u16),
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
/// It must be created and used on the same thread, or wrapped in SendTerminalSession.
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
}

/// Handle to a terminal runner thread that can be used to send commands.
/// This is Send + Sync and can be safely shared between threads.
pub struct TerminalHandle {
    /// Channel sender for commands
    pub cmd_tx: mpsc::Sender<TerminalCommand>,
    /// Response receiver (this is a tokio mpsc, so we need a separate channel for responses)
    pub resp_rx: mpsc::Receiver<TerminalResponse>,
    /// Session name
    pub name: String,
    /// Session creation time
    pub created_at: Instant,
}

impl TerminalHandle {
    /// Send a command to the terminal runner and wait for a response
    pub async fn send_command(&mut self, cmd: TerminalCommand) -> Result<TerminalResponse> {
        self.cmd_tx.send(cmd).await.context("Failed to send command")?;
        self.resp_rx.recv().await.context("Failed to receive response")
    }

    /// Write data to the terminal PTY
    pub async fn write(&mut self, data: Vec<u8>) -> Result<()> {
        self.cmd_tx.send(TerminalCommand::Write(data)).await.context("Failed to send write command")?;
        Ok(())
    }

    /// Resize the terminal
    pub async fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        self.cmd_tx.send(TerminalCommand::Resize { cols, rows }).await.context("Failed to send resize command")?;
        Ok(())
    }

    /// Get terminal dimensions
    pub async fn get_size(&mut self) -> Result<(u16, u16)> {
        self.cmd_tx.send(TerminalCommand::GetSize).await.context("Failed to send get_size command")?;
        match self.resp_rx.recv().await {
            Some(TerminalResponse::Size(cols, rows)) => Ok((cols, rows)),
            Some(TerminalResponse::Error(e)) => Err(anyhow::anyhow!(e)),
            _ => Err(anyhow::anyhow!("Unexpected response")),
        }
    }

    /// Shutdown the terminal session
    pub async fn shutdown(self) -> Result<()> {
        self.cmd_tx.send(TerminalCommand::Shutdown).await.context("Failed to send shutdown command")?;
        Ok(())
    }
}

/// Spawn a terminal runner in a dedicated thread.
/// Returns a TerminalHandle that can be used to send commands to the runner.
/// 
/// # Arguments
/// * `name` - Human-readable name for the session
/// * `cols` - Terminal width in columns (default: 140)
/// * `rows` - Terminal height in rows (default: 50)
/// * `cwd` - Working directory for the shell (default: current dir)
/// * `env` - Additional environment variables to set
///
/// # Returns
/// A TerminalHandle that can be used to control the terminal session.
pub fn spawn_runner(
    name: impl Into<String>,
    cols: Option<usize>,
    rows: Option<usize>,
    cwd: Option<&Path>,
    env: Option<HashMap<String, String>>,
) -> Result<TerminalHandle> {
    let name = name.into();
    let name_clone = name.clone();
    let cols = cols.unwrap_or(140);
    let rows = rows.unwrap_or(50);

    // Convert cwd to owned PathBuf to avoid lifetime issues
    let cwd_owned: Option<std::path::PathBuf> = cwd.map(|p| p.to_path_buf());

    // Create channels for command/response
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<TerminalCommand>(32);
    let (resp_tx, resp_rx) = mpsc::channel::<TerminalResponse>(32);

    // Spawn the terminal session in a dedicated thread
    let thread_name = name.clone();
    let _runner_thread = thread::spawn(move || {
        // Create the terminal session in this thread
        // Convert back to &Path for the spawn method
        let cwd_ref = cwd_owned.as_deref();
        let mut session = match TerminalSession::spawn(&name_clone, Some(cols), Some(rows), cwd_ref, env) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to spawn terminal session '{}': {}", thread_name, e);
                return;
            }
        };

        let _created_at = session.created_at;

        // Run the event loop
        let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
        rt.block_on(async {
            loop {
                tokio::select! {
                    // Process commands from the channel
                    Some(cmd) = cmd_rx.recv() => {
                        match cmd {
                            TerminalCommand::Write(data) => {
                                if let Err(e) = session.write_to_pty(&data) {
                                    let _ = resp_tx.send(TerminalResponse::Error(e.to_string())).await;
                                }
                            }
                            TerminalCommand::Resize { cols, rows } => {
                                if let Err(e) = session.resize(cols, rows) {
                                    let _ = resp_tx.send(TerminalResponse::Error(e.to_string())).await;
                                } else {
                                    let _ = resp_tx.send(TerminalResponse::Ack).await;
                                }
                            }
                            TerminalCommand::ProcessOutput => {
                                match session.process_output() {
                                    Ok(count) => {
                                        let _ = resp_tx.send(TerminalResponse::OutputProcessed(count)).await;
                                    }
                                    Err(e) => {
                                        let _ = resp_tx.send(TerminalResponse::Error(e.to_string())).await;
                                    }
                                }
                            }
                            TerminalCommand::GetSize => {
                                match session.size() {
                                    Ok((cols, rows)) => {
                                        let _ = resp_tx.send(TerminalResponse::Size(cols, rows)).await;
                                    }
                                    Err(e) => {
                                        let _ = resp_tx.send(TerminalResponse::Error(e.to_string())).await;
                                    }
                                }
                            }
                            TerminalCommand::Shutdown => {
                                // Drop the session to clean up
                                drop(session);
                                break;
                            }
                        }
                    }
                    // Periodically process output even without commands
                    _ = tokio::time::sleep(tokio::time::Duration::from_millis(10)) => {
                        let _ = session.process_output();
                    }
                }
            }
        });
    });

    Ok(TerminalHandle {
        cmd_tx,
        resp_rx,
        name,
        created_at: Instant::now(),
    })
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
    pub fn write_to_pty(&self, data: &[u8]) -> Result<()> {
        use std::io::Write;
        let mut writer = self
            .pty_pair
            .master
            .take_writer()
            .context("Failed to get PTY writer")?;
        writer.write_all(data).context("Failed to write to PTY")?;
        Ok(())
    }

    /// Get the terminal dimensions
    pub fn size(&self) -> Result<(u16, u16)> {
        Ok((self.terminal.cols()?, self.terminal.rows()?))
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
        let mut handle = spawn_runner("test-runner", Some(80), Some(24), None, None)
            .expect("Failed to spawn terminal runner");

        // Get size
        let (cols, rows) = handle.get_size().await.expect("Failed to get size");
        assert_eq!(cols, 80);
        assert_eq!(rows, 24);

        // Shutdown
        handle.shutdown().await.expect("Failed to shutdown");
    }
}
