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

/// Command execution state tracked via OSC 133 sequences
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandState {
    /// No command is currently running (shell is idle at prompt)
    Idle,
    /// A command is currently executing
    Running,
    /// A command just completed, exit code available
    Completed,
}

impl Default for CommandState {
    fn default() -> Self {
        CommandState::Idle
    }
}

/// Comprehensive terminal information for list operations
#[derive(Debug, Clone)]
pub struct TerminalInfo {
    /// Current command execution state
    pub command_state: CommandState,
    /// Current command being executed (if running)
    pub current_command: Option<String>,
    /// Last exit code (if command completed)
    pub last_exit_code: Option<i32>,
    /// Terminal dimensions (cols, rows)
    pub size: (u16, u16),
    /// Whether terminal is in alternate screen mode
    pub is_alternate_screen: bool,
    /// Session age in seconds
    pub age_seconds: u64,
}

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
    /// Get current command state and last exit code (response sent via oneshot)
    GetCommandState {
        resp_tx: oneshot::Sender<TerminalResponse>,
    },
    /// Check if a command is currently running (response sent via oneshot)
    IsCommandRunning {
        resp_tx: oneshot::Sender<TerminalResponse>,
    },
    /// Get the last command exit code (response sent via oneshot)
    GetLastExitCode {
        resp_tx: oneshot::Sender<TerminalResponse>,
    },
    /// Get current command being executed (response sent via oneshot)
    GetCurrentCommand {
        resp_tx: oneshot::Sender<TerminalResponse>,
    },
    /// Get comprehensive terminal info (response sent via oneshot)
    GetInfo {
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
    /// Command state and last exit code
    CommandState { state: CommandState, exit_code: Option<i32> },
    /// Command is running status
    IsCommandRunning(bool),
    /// Last command exit code
    LastExitCode(Option<i32>),
    /// Current command being executed
    CurrentCommand(Option<String>),
    /// Comprehensive terminal info
    TerminalInfo {
        command_state: CommandState,
        current_command: Option<String>,
        last_exit_code: Option<i32>,
        size: (u16, u16),
        is_alternate_screen: bool,
        age_seconds: u64,
    },
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

/// Path to the temporary rcfile created for bash lifecycle injection
pub type TempRcfilePath = std::path::PathBuf;

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

    /// Temporary rcfile path for cleanup (bash lifecycle injection)
    temp_rcfile: Option<TempRcfilePath>,

    /// Current command execution state (tracked via OSC 133 sequences)
    command_state: CommandState,

    /// Last command exit code (valid when state is Completed)
    last_exit_code: Option<i32>,

    /// Current command being executed (parsed from OSC 133;B or terminal content)
    current_command: Option<String>,

    /// Time when current command started (for timeout tracking)
    command_start_time: Option<Instant>,
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
                            TerminalCommand::GetCommandState { resp_tx } => {
                                let state = self.session.get_command_state();
                                let exit_code = self.session.get_last_exit_code();
                                let _ = resp_tx.send(TerminalResponse::CommandState { state, exit_code });
                            }
                            TerminalCommand::IsCommandRunning { resp_tx } => {
                                let is_running = self.session.is_command_running();
                                let _ = resp_tx.send(TerminalResponse::IsCommandRunning(is_running));
                            }
                            TerminalCommand::GetLastExitCode { resp_tx } => {
                                let exit_code = self.session.get_last_exit_code();
                                let _ = resp_tx.send(TerminalResponse::LastExitCode(exit_code));
                            }
                            TerminalCommand::GetCurrentCommand { resp_tx } => {
                                let cmd = self.session.get_current_command();
                                let _ = resp_tx.send(TerminalResponse::CurrentCommand(cmd));
                            }
                            TerminalCommand::GetInfo { resp_tx } => {
                                let info = self.session.get_terminal_info();
                                let _ = resp_tx.send(TerminalResponse::TerminalInfo {
                                    command_state: info.command_state,
                                    current_command: info.current_command,
                                    last_exit_code: info.last_exit_code,
                                    size: info.size,
                                    is_alternate_screen: info.is_alternate_screen,
                                    age_seconds: info.age_seconds,
                                });
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

        // Inject the lifecycle script via environment variable
        // BASH_ENV is sourced by bash before executing commands in non-interactive mode
        // For interactive shells, we'll use --rcfile with a wrapper
        let lifecycle_script = std::env::var("NAC_LIFECYCLE_SCRIPT")
            .unwrap_or_else(|_| {
                // Default to the script embedded in the crate
                let crate_dir = std::env::var("CARGO_MANIFEST_DIR")
                    .unwrap_or_else(|_| ".".to_string());
                format!("{}/src/terminal/bash_lifecycle.sh", crate_dir)
            });

        // Create a temporary rcfile that sources both system bashrc and our lifecycle script
        let rcfile_content = format!(
            r#"# NAC generated bashrc wrapper
# Source system bashrc if it exists
if [ -f /etc/bash.bashrc ]; then
    source /etc/bash.bashrc
elif [ -f /etc/bashrc ]; then
    source /etc/bashrc
fi
if [ -f ~/.bashrc ]; then
    source ~/.bashrc
fi

# Source NAC lifecycle integration
if [ -f "{lifecycle_script}" ]; then
    source "{lifecycle_script}"
fi
"#
        );

        // Write the rcfile to a temp location
        let temp_rcfile = std::env::temp_dir().join(format!("nac_bashrc_{}.sh", std::process::id()));
        std::fs::write(&temp_rcfile, rcfile_content)
            .context("Failed to write temporary bashrc file")?;

        // Use --rcfile to inject our lifecycle script
        cmd.arg("--rcfile");
        cmd.arg(&temp_rcfile);

        // Set environment variable so the script knows it's running under NAC
        cmd.env("NAC_TERMINAL", "1");
        cmd.env("NAC_LIFECYCLE_SCRIPT", &lifecycle_script);

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
            temp_rcfile: Some(temp_rcfile),
            command_state: CommandState::Idle,
            last_exit_code: None,
            current_command: None,
            command_start_time: None,
        })
    }

    /// Process pending PTY output and feed it to the terminal.
    /// Call this periodically (e.g., in an async task or event loop).
    /// Returns the number of output chunks processed.
    /// 
    /// This method also parses OSC 133 sequences from the bash lifecycle script
    /// to track command execution state:
    /// - OSC 133;A - command started
    /// - OSC 133;D;exitcode - command finished with exit code
    pub fn process_output(&mut self) -> Result<usize> {
        let mut count = 0;

        // Drain all available output from the channel
        while let Ok(output) = self.pty_receiver.try_recv() {
            // Parse OSC 133 sequences before feeding to terminal
            self.parse_osc_133_sequences(&output.data);
            
            // Feed data to the terminal emulator
            self.terminal.vt_write(&output.data);
            count += 1;
        }

        Ok(count)
    }

    /// Parse OSC 133 sequences from PTY output to track command state.
    /// 
    /// OSC sequences have the format: ESC ] <params> BEL or ESC ] <params> ST
    /// where ESC = 0x1B, ] = 0x5D, BEL = 0x07, ST = ESC \ (0x1B 0x5C)
    /// 
    /// OSC 133 sequences from bash lifecycle script:
    /// - ESC ] 133 ; A BEL  -> Command started
    /// - ESC ] 133 ; D ; <exitcode> BEL -> Command finished
    fn parse_osc_133_sequences(&mut self, data: &[u8]) {
        let mut i = 0;
        while i < data.len() {
            // Look for OSC start: ESC ]
            if i + 1 < data.len() && data[i] == 0x1B && data[i + 1] == 0x5D {
                // Found potential OSC sequence, find the end
                if let Some((seq_end, params)) = self.extract_osc_sequence(&data[i..]) {
                    // Check if this is OSC 133
                    if params.starts_with(b"133;") {
                        self.handle_osc_133(&params[4..]); // Skip "133;"
                    }
                    i += seq_end;
                    continue;
                }
            }
            i += 1;
        }
    }

    /// Extract an OSC sequence from data starting at ESC (0x1B).
    /// Returns (bytes_consumed, params) where params is the content between ESC ] and terminator.
    fn extract_osc_sequence<'a>(&self, data: &'a [u8]) -> Option<(usize, &'a [u8])> {
        if data.len() < 3 || data[0] != 0x1B || data[1] != 0x5D {
            return None;
        }

        // Find terminator: BEL (0x07) or ST (ESC \ = 0x1B 0x5C)
        let mut i = 2;
        while i < data.len() {
            if data[i] == 0x07 {
                // BEL terminator found
                let params = &data[2..i];
                return Some((i + 1, params));
            }
            if data[i] == 0x1B && i + 1 < data.len() && data[i + 1] == 0x5C {
                // ST terminator found (ESC \)
                let params = &data[2..i];
                return Some((i + 2, params));
            }
            i += 1;
        }

        // No terminator found - incomplete sequence
        None
    }

    /// Handle an OSC 133 sequence (params after "133;")
    fn handle_osc_133(&mut self, params: &[u8]) {
        if params.is_empty() {
            return;
        }

        match params[0] {
            b'A' => {
                // OSC 133;A - Command started (pre-exec)
                self.command_state = CommandState::Running;
                self.last_exit_code = None;
                self.command_start_time = Some(Instant::now());
                // Try to extract command from params if present: A;<cmd>
                if params.len() > 2 && params[1] == b';' {
                    let cmd = String::from_utf8_lossy(&params[2..]);
                    self.current_command = Some(cmd.to_string());
                }
            }
            b'B' => {
                // OSC 133;B - Command line content (contains the actual command)
                if params.len() > 2 && params[1] == b';' {
                    let cmd = String::from_utf8_lossy(&params[2..]);
                    self.current_command = Some(cmd.trim().to_string());
                }
            }
            b'C' => {
                // OSC 133;C - Command executed (post-exec, before output)
                // Command is now running, state already set by A
            }
            b'D' => {
                // OSC 133;D;exitcode - Command finished
                if params.len() > 2 && params[1] == b';' {
                    let exit_code_str = std::str::from_utf8(&params[2..]).unwrap_or("0");
                    if let Ok(code) = exit_code_str.parse::<i32>() {
                        self.last_exit_code = Some(code);
                    } else {
                        self.last_exit_code = Some(0);
                    }
                } else {
                    self.last_exit_code = Some(0);
                }
                self.command_state = CommandState::Completed;
                self.command_start_time = None;
            }
            _ => {
                // Other OSC 133 sub-commands
            }
        }
    }

    /// Get the current command execution state
    pub fn get_command_state(&self) -> CommandState {
        self.command_state
    }

    /// Get the last command exit code (valid when state is Completed)
    pub fn get_last_exit_code(&self) -> Option<i32> {
        self.last_exit_code
    }

    /// Check if a command is currently running
    pub fn is_command_running(&self) -> bool {
        self.command_state == CommandState::Running
    }

    /// Reset command state from Completed back to Idle.
    /// Call this after processing a completed command.
    pub fn reset_command_state(&mut self) {
        if self.command_state == CommandState::Completed {
            self.command_state = CommandState::Idle;
            self.current_command = None;
        }
    }

    /// Get the current command being executed (if any)
    pub fn get_current_command(&self) -> Option<String> {
        self.current_command.clone()
    }

    /// Get comprehensive terminal information
    pub fn get_terminal_info(&self) -> TerminalInfo {
        let size = self.size().unwrap_or((0, 0));
        let age_seconds = self.age().as_secs();
        
        TerminalInfo {
            command_state: self.command_state,
            current_command: self.current_command.clone(),
            last_exit_code: self.last_exit_code,
            size,
            is_alternate_screen: self.is_alternate_screen(),
            age_seconds,
        }
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

        // Clean up the temporary rcfile if it exists
        if let Some(ref rcfile) = self.temp_rcfile {
            let _ = std::fs::remove_file(rcfile);
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
