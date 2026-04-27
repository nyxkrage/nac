use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::terminal::runner::{spawn_runner, CommandState, TerminalCommand, TerminalResponse};

/// Default timeout for auto-unown when terminal is idle
const DEFAULT_AUTO_UNOWN_TIMEOUT: Duration = Duration::from_secs(300); // 5 minutes

/// Information about a terminal session for listing
#[derive(Debug, Clone, serde::Serialize)]
pub struct TerminalInfo {
    /// Terminal session name
    pub name: String,
    /// Current owner thread ID (if checked out)
    pub owner: Option<String>,
    /// Current command execution state
    #[serde(with = "command_state_serde")]
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

/// Custom serialization for CommandState
mod command_state_serde {
    use serde::Serializer;
    use crate::terminal::runner::CommandState;

    pub fn serialize<S: Serializer>(state: &CommandState, serializer: S) -> Result<S::Ok, S::Error> {
        let s = match state {
            CommandState::Idle => "idle",
            CommandState::Running => "running",
            CommandState::Completed => "completed",
        };
        serializer.serialize_str(s)
    }
}

/// Handle to a terminal session that can be stored in the manager.
/// Contains the session name and the channel sender to communicate with the runner thread.
/// The actual TerminalSession lives in a dedicated thread and is NOT held here.
#[derive(Clone)]
pub struct TerminalHandle {
    pub name: String,
    pub sender: mpsc::Sender<TerminalCommand>,
    /// Owner thread identifier. None means the terminal is not checked out.
    /// When Some(owner), only that owner can perform operations on the terminal.
    pub owner: Arc<Mutex<Option<String>>>,
    /// Last activity timestamp for auto-unown
    last_activity: Arc<Mutex<Instant>>,
    /// Auto-unown timeout duration
    auto_unown_timeout: Arc<Mutex<Duration>>,
}

impl TerminalHandle {
    /// Create a new terminal handle
    pub fn new(name: String, sender: mpsc::Sender<TerminalCommand>) -> Self {
        Self {
            name,
            sender,
            owner: Arc::new(Mutex::new(None)),
            last_activity: Arc::new(Mutex::new(Instant::now())),
            auto_unown_timeout: Arc::new(Mutex::new(DEFAULT_AUTO_UNOWN_TIMEOUT)),
        }
    }

    /// Send a command to the terminal runner
    pub async fn send(&self, cmd: TerminalCommand) -> anyhow::Result<()> {
        self.sender.send(cmd).await?;
        Ok(())
    }

    /// Try to checkout this terminal for the given owner.
    /// Returns true if checkout succeeded (terminal was available or already owned by this owner).
    /// Returns false if terminal is already owned by a different owner.
    pub async fn try_checkout(&self, owner: &str) -> bool {
        let mut current_owner = self.owner.lock().await;
        match &*current_owner {
            None => {
                *current_owner = Some(owner.to_string());
                // Update activity timestamp on checkout
                *self.last_activity.lock().await = Instant::now();
                true
            }
            Some(existing) if existing == owner => {
                // Update activity timestamp on activity from owner
                *self.last_activity.lock().await = Instant::now();
                true
            }
            Some(_) => false, // Owned by different owner
        }
    }

    /// Checkin this terminal (release ownership).
    /// Returns true if checkin succeeded (terminal was owned by this owner).
    /// Returns false if terminal was not owned or owned by different owner.
    pub async fn checkin(&self, owner: &str) -> bool {
        let mut current_owner = self.owner.lock().await;
        match &*current_owner {
            Some(existing) if existing == owner => {
                *current_owner = None;
                true
            }
            _ => false,
        }
    }

    /// Get the current owner if any
    pub async fn get_owner(&self) -> Option<String> {
        self.owner.lock().await.clone()
    }

    /// Update the last activity timestamp
    pub async fn update_activity(&self) {
        *self.last_activity.lock().await = Instant::now();
    }

    /// Get the last activity timestamp
    pub async fn get_last_activity(&self) -> Instant {
        *self.last_activity.lock().await
    }

    /// Set the auto-unown timeout duration
    pub async fn set_auto_unown_timeout(&self, timeout: Duration) {
        *self.auto_unown_timeout.lock().await = timeout;
    }

    /// Get the auto-unown timeout duration
    pub async fn get_auto_unown_timeout(&self) -> Duration {
        *self.auto_unown_timeout.lock().await
    }

    /// Check if this terminal should be auto-unowned (idle timeout exceeded)
    pub async fn should_auto_unown(&self) -> bool {
        let owner = self.owner.lock().await;
        if owner.is_none() {
            return false; // Not owned, nothing to unown
        }
        
        let last_activity = *self.last_activity.lock().await;
        let timeout = *self.auto_unown_timeout.lock().await;
        
        last_activity.elapsed() > timeout
    }

    /// Force unown (release ownership regardless of who owns it)
    /// Returns the previous owner if there was one
    pub async fn force_unown(&self) -> Option<String> {
        let mut current_owner = self.owner.lock().await;
        let prev = current_owner.take();
        prev
    }
}

/// Manages multiple terminal sessions in a thread-safe manner.
/// Provides access to named terminal sessions via a HashMap.
///
/// Each terminal session runs in its own dedicated thread and is controlled
/// via channels. The TerminalManager holds TerminalHandle structs which
/// contain the sender ends of these channels.
#[derive(Clone)]
pub struct TerminalManager {
    /// Map of terminal name -> TerminalHandle
    /// Each handle contains a sender connected to a dedicated runner thread
    /// that owns the TerminalSession.
    sessions: Arc<Mutex<HashMap<String, TerminalHandle>>>,
}

impl TerminalManager {
    /// Create a new empty TerminalManager
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Create and insert a new terminal session.
    /// Returns true if a session with this name already existed (and was replaced).
    pub async fn create(
        &self,
        name: String,
        cols: Option<usize>,
        rows: Option<usize>,
        cwd: Option<std::path::PathBuf>,
        env: Option<HashMap<String, String>>,
    ) -> anyhow::Result<bool> {
        // Convert PathBuf to &Path for spawn_runner
        let cwd_ref = cwd.as_deref();

        // Spawn the terminal runner (this creates the dedicated thread)
        // The TerminalSession lives in the runner thread, NOT here
        let sender = spawn_runner(&name, cols, rows, cwd_ref, env)?;

        // Store the handle (name + sender) in the manager
        let mut sessions = self.sessions.lock().await;
        let handle = TerminalHandle::new(name.clone(), sender);
        let existed = sessions.insert(name, handle).is_some();
        Ok(existed)
    }

    /// Insert an existing terminal handle into the manager.
    /// Returns true if a session with this name already existed (and was replaced).
    pub async fn insert(&self, handle: TerminalHandle) -> bool {
        let name = handle.name.clone();
        let mut sessions = self.sessions.lock().await;
        sessions.insert(name, handle).is_some()
    }

    /// Get a terminal handle by name, removing it from the manager.
    /// Returns None if no session with that name exists.
    /// Use this when you need ownership of the handle (e.g., for shutdown).
    pub async fn take(&self, name: &str) -> Option<TerminalHandle> {
        let mut sessions = self.sessions.lock().await;
        sessions.remove(name)
    }

    /// Remove a terminal session by name.
    /// Returns the removed handle if it existed.
    pub async fn remove(&self, name: &str) -> Option<TerminalHandle> {
        let mut sessions = self.sessions.lock().await;
        sessions.remove(name)
    }

    /// Check if a terminal session with the given name exists.
    pub async fn contains(&self, name: &str) -> bool {
        let sessions = self.sessions.lock().await;
        sessions.contains_key(name)
    }

    /// List all terminal session names.
    pub async fn list_names(&self) -> Vec<String> {
        let sessions = self.sessions.lock().await;
        sessions.keys().cloned().collect()
    }

    /// List all terminals with detailed information.
    /// Does NOT require ownership - this is an administrative operation.
    pub async fn list_terminals(&self) -> anyhow::Result<Vec<TerminalInfo>> {
        let sessions = self.sessions.lock().await;
        let mut result = Vec::with_capacity(sessions.len());

        for (name, handle) in sessions.iter() {
            // Get owner
            let owner = handle.get_owner().await;

            // Send GetInfo command to get terminal details
            let (resp_tx, resp_rx) = oneshot::channel();
            match handle.sender.send(TerminalCommand::GetInfo { resp_tx }).await {
                Ok(_) => {
                    match resp_rx.await {
                        Ok(TerminalResponse::TerminalInfo { 
                            command_state, 
                            current_command, 
                            last_exit_code, 
                            size, 
                            is_alternate_screen,
                            age_seconds 
                        }) => {
                            result.push(TerminalInfo {
                                name: name.clone(),
                                owner,
                                command_state,
                                current_command,
                                last_exit_code,
                                size,
                                is_alternate_screen,
                                age_seconds,
                            });
                        }
                        Ok(TerminalResponse::Error(e)) => {
                            // Include terminal with error state
                            result.push(TerminalInfo {
                                name: name.clone(),
                                owner,
                                command_state: CommandState::Idle,
                                current_command: Some(format!("[Error: {}]", e)),
                                last_exit_code: None,
                                size: (0, 0),
                                is_alternate_screen: false,
                                age_seconds: 0,
                            });
                        }
                        _ => {
                            // Include terminal with unknown state
                            result.push(TerminalInfo {
                                name: name.clone(),
                                owner,
                                command_state: CommandState::Idle,
                                current_command: Some("[Unknown state]".to_string()),
                                last_exit_code: None,
                                size: (0, 0),
                                is_alternate_screen: false,
                                age_seconds: 0,
                            });
                        }
                    }
                }
                Err(_) => {
                    // Terminal may be shutting down, include with error state
                    result.push(TerminalInfo {
                        name: name.clone(),
                        owner,
                        command_state: CommandState::Idle,
                        current_command: Some("[Disconnected]".to_string()),
                        last_exit_code: None,
                        size: (0, 0),
                        is_alternate_screen: false,
                        age_seconds: 0,
                    });
                }
            }
        }

        Ok(result)
    }

    /// Get the number of active terminal sessions.
    pub async fn len(&self) -> usize {
        let sessions = self.sessions.lock().await;
        sessions.len()
    }

    /// Check if there are no terminal sessions.
    pub async fn is_empty(&self) -> bool {
        let sessions = self.sessions.lock().await;
        sessions.is_empty()
    }

    /// Checkout a terminal for exclusive access by the given owner.
    /// Returns Ok(true) if checkout succeeded.
    /// Returns Ok(false) if terminal doesn't exist.
    /// Returns Err if terminal is already owned by a different owner.
    pub async fn checkout_terminal(&self, name: &str, owner: String) -> anyhow::Result<bool> {
        let sessions = self.sessions.lock().await;
        if let Some(handle) = sessions.get(name) {
            if handle.try_checkout(&owner).await {
                Ok(true)
            } else {
                let current_owner = handle.get_owner().await;
                Err(anyhow::anyhow!(
                    "Terminal '{}' is already owned by {:?}",
                    name,
                    current_owner
                ))
            }
        } else {
            Ok(false)
        }
    }

    /// Checkin a terminal to release ownership.
    /// Returns Ok(true) if checkin succeeded.
    /// Returns Ok(false) if terminal doesn't exist.
    /// Returns Err if terminal was not owned by the given owner.
    pub async fn checkin_terminal(&self, name: &str, owner: &str) -> anyhow::Result<bool> {
        let sessions = self.sessions.lock().await;
        if let Some(handle) = sessions.get(name) {
            if handle.checkin(owner).await {
                Ok(true)
            } else {
                let current_owner = handle.get_owner().await;
                Err(anyhow::anyhow!(
                    "Terminal '{}' is not owned by '{}' (current owner: {:?})",
                    name,
                    owner,
                    current_owner
                ))
            }
        } else {
            Ok(false)
        }
    }

    /// Send a command to a specific terminal session.
    /// Returns true if the session existed and the command was sent.
    /// Requires ownership - will fail if terminal is not checked out by the given owner.
    pub async fn send_command(
        &self,
        name: &str,
        owner: &str,
        cmd: TerminalCommand,
    ) -> anyhow::Result<bool> {
        let sessions = self.sessions.lock().await;
        if let Some(handle) = sessions.get(name) {
            // Verify ownership
            let current_owner = handle.get_owner().await;
            if current_owner.as_deref() != Some(owner) {
                return Err(anyhow::anyhow!(
                    "Terminal '{}' is not owned by '{}' (current owner: {:?})",
                    name,
                    owner,
                    current_owner
                ));
            }
            handle.sender.send(cmd).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Write data to a terminal session's PTY.
    /// Returns true if the session existed.
    /// Requires ownership - will fail if terminal is not checked out by the given owner.
    pub async fn write_to_terminal(
        &self,
        name: &str,
        owner: &str,
        data: Vec<u8>,
    ) -> anyhow::Result<bool> {
        let sessions = self.sessions.lock().await;
        if let Some(handle) = sessions.get(name) {
            // Verify ownership
            let current_owner = handle.get_owner().await;
            if current_owner.as_deref() != Some(owner) {
                return Err(anyhow::anyhow!(
                    "Terminal '{}' is not owned by '{}' (current owner: {:?})",
                    name,
                    owner,
                    current_owner
                ));
            }
            handle.sender.send(TerminalCommand::Write(data)).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Resize a terminal session.
    /// Returns true if the session existed.
    /// Requires ownership - will fail if terminal is not checked out by the given owner.
    pub async fn resize_terminal(
        &self,
        name: &str,
        owner: &str,
        cols: u16,
        rows: u16,
    ) -> anyhow::Result<bool> {
        self.send_command(name, owner, TerminalCommand::Resize { cols, rows })
            .await
    }

    /// Read the current content of a terminal session.
    /// Returns the content string if the session exists.
    /// Requires ownership - will fail if terminal is not checked out by the given owner.
    pub async fn read_terminal(
        &self,
        name: &str,
        owner: &str,
    ) -> anyhow::Result<Option<String>> {
        let sessions = self.sessions.lock().await;
        if let Some(handle) = sessions.get(name) {
            // Verify ownership
            let current_owner = handle.get_owner().await;
            if current_owner.as_deref() != Some(owner) {
                return Err(anyhow::anyhow!(
                    "Terminal '{}' is not owned by '{}' (current owner: {:?})",
                    name,
                    owner,
                    current_owner
                ));
            }
            let (resp_tx, resp_rx) = oneshot::channel();
            handle.sender.send(TerminalCommand::Read { resp_tx }).await?;
            match resp_rx.await {
                Ok(TerminalResponse::Content(content)) => Ok(Some(content)),
                Ok(TerminalResponse::Error(e)) => Err(anyhow::anyhow!(e)),
                _ => Err(anyhow::anyhow!("Unexpected response")),
            }
        } else {
            Ok(None)
        }
    }

    /// Get the dimensions of a terminal session.
    /// Returns (cols, rows) if the session exists.
    /// Requires ownership - will fail if terminal is not checked out by the given owner.
    pub async fn get_terminal_size(
        &self,
        name: &str,
        owner: &str,
    ) -> anyhow::Result<Option<(u16, u16)>> {
        let sessions = self.sessions.lock().await;
        if let Some(handle) = sessions.get(name) {
            // Verify ownership
            let current_owner = handle.get_owner().await;
            if current_owner.as_deref() != Some(owner) {
                return Err(anyhow::anyhow!(
                    "Terminal '{}' is not owned by '{}' (current owner: {:?})",
                    name,
                    owner,
                    current_owner
                ));
            }
            let (resp_tx, resp_rx) = oneshot::channel();
            handle.sender.send(TerminalCommand::GetSize { resp_tx }).await?;
            match resp_rx.await {
                Ok(TerminalResponse::Size(cols, rows)) => Ok(Some((cols, rows))),
                Ok(TerminalResponse::Error(e)) => Err(anyhow::anyhow!(e)),
                _ => Err(anyhow::anyhow!("Unexpected response")),
            }
        } else {
            Ok(None)
        }
    }

    /// Check if a terminal session is in alternate screen mode.
    /// Returns true if in alternate screen, false if in primary screen or on error.
    /// Requires ownership - will fail if terminal is not checked out by the given owner.
    pub async fn is_alternate_screen(
        &self,
        name: &str,
        owner: &str,
    ) -> anyhow::Result<Option<bool>> {
        let sessions = self.sessions.lock().await;
        if let Some(handle) = sessions.get(name) {
            // Verify ownership
            let current_owner = handle.get_owner().await;
            if current_owner.as_deref() != Some(owner) {
                return Err(anyhow::anyhow!(
                    "Terminal '{}' is not owned by '{}' (current owner: {:?})",
                    name,
                    owner,
                    current_owner
                ));
            }
            let (resp_tx, resp_rx) = oneshot::channel();
            handle
                .sender
                .send(TerminalCommand::IsAlternateScreen { resp_tx })
                .await?;
            match resp_rx.await {
                Ok(TerminalResponse::AlternateScreen(is_alt)) => Ok(Some(is_alt)),
                Ok(TerminalResponse::Error(e)) => Err(anyhow::anyhow!(e)),
                _ => Err(anyhow::anyhow!("Unexpected response")),
            }
        } else {
            Ok(None)
        }
    }

    /// Get the current command execution state of a terminal session.
    /// Returns the CommandState if the session exists.
    /// Requires ownership - will fail if terminal is not checked out by the given owner.
    pub async fn get_command_state(
        &self,
        name: &str,
        owner: &str,
    ) -> anyhow::Result<Option<CommandState>> {
        let sessions = self.sessions.lock().await;
        if let Some(handle) = sessions.get(name) {
            // Verify ownership
            let current_owner = handle.get_owner().await;
            if current_owner.as_deref() != Some(owner) {
                return Err(anyhow::anyhow!(
                    "Terminal '{}' is not owned by '{}' (current owner: {:?})",
                    name,
                    owner,
                    current_owner
                ));
            }
            let (resp_tx, resp_rx) = oneshot::channel();
            handle
                .sender
                .send(TerminalCommand::GetCommandState { resp_tx })
                .await?;
            match resp_rx.await {
                Ok(TerminalResponse::CommandState { state, .. }) => Ok(Some(state)),
                Ok(TerminalResponse::Error(e)) => Err(anyhow::anyhow!(e)),
                _ => Err(anyhow::anyhow!("Unexpected response")),
            }
        } else {
            Ok(None)
        }
    }

    /// Check if a command is currently running in a terminal session.
    /// Returns true if a command is running, false if idle/completed or on error.
    /// Requires ownership - will fail if terminal is not checked out by the given owner.
    pub async fn is_command_running(
        &self,
        name: &str,
        owner: &str,
    ) -> anyhow::Result<Option<bool>> {
        let sessions = self.sessions.lock().await;
        if let Some(handle) = sessions.get(name) {
            // Verify ownership
            let current_owner = handle.get_owner().await;
            if current_owner.as_deref() != Some(owner) {
                return Err(anyhow::anyhow!(
                    "Terminal '{}' is not owned by '{}' (current owner: {:?})",
                    name,
                    owner,
                    current_owner
                ));
            }
            let (resp_tx, resp_rx) = oneshot::channel();
            handle
                .sender
                .send(TerminalCommand::IsCommandRunning { resp_tx })
                .await?;
            match resp_rx.await {
                Ok(TerminalResponse::IsCommandRunning(is_running)) => Ok(Some(is_running)),
                Ok(TerminalResponse::Error(e)) => Err(anyhow::anyhow!(e)),
                _ => Err(anyhow::anyhow!("Unexpected response")),
            }
        } else {
            Ok(None)
        }
    }

    /// Get the last command exit code from a terminal session.
    /// Returns Some(exit_code) if a command completed, None if no command completed yet.
    /// Requires ownership - will fail if terminal is not checked out by the given owner.
    pub async fn get_last_exit_code(
        &self,
        name: &str,
        owner: &str,
    ) -> anyhow::Result<Option<Option<i32>>> {
        let sessions = self.sessions.lock().await;
        if let Some(handle) = sessions.get(name) {
            // Verify ownership
            let current_owner = handle.get_owner().await;
            if current_owner.as_deref() != Some(owner) {
                return Err(anyhow::anyhow!(
                    "Terminal '{}' is not owned by '{}' (current owner: {:?})",
                    name,
                    owner,
                    current_owner
                ));
            }
            let (resp_tx, resp_rx) = oneshot::channel();
            handle
                .sender
                .send(TerminalCommand::GetLastExitCode { resp_tx })
                .await?;
            match resp_rx.await {
                Ok(TerminalResponse::LastExitCode(exit_code)) => Ok(Some(exit_code)),
                Ok(TerminalResponse::Error(e)) => Err(anyhow::anyhow!(e)),
                _ => Err(anyhow::anyhow!("Unexpected response")),
            }
        } else {
            Ok(None)
        }
    }

    /// Shutdown a terminal session by name.
    /// Returns true if the session existed and was shut down.
    /// Does NOT require ownership - this is an administrative operation.
    pub async fn shutdown_terminal(&self, name: &str) -> anyhow::Result<bool> {
        if let Some(handle) = self.take(name).await {
            handle.sender.send(TerminalCommand::Close).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Check all terminals for auto-unown condition and release ownership
    /// for those that have exceeded the idle timeout.
    /// Returns the list of terminal names that were auto-unowned.
    pub async fn check_auto_unown(&self) -> Vec<String> {
        let sessions = self.sessions.lock().await;
        let mut unowned = Vec::new();

        for (name, handle) in sessions.iter() {
            if handle.should_auto_unown().await {
                if let Some(prev_owner) = handle.force_unown().await {
                    unowned.push(name.clone());
                    eprintln!(
                        "[nac] Auto-unowned terminal '{}' from owner '{}' due to idle timeout",
                        name,
                        prev_owner
                    );
                }
            }
        }

        unowned
    }

    /// Set the auto-unown timeout for a specific terminal.
    /// Returns true if the terminal exists.
    pub async fn set_terminal_timeout(
        &self,
        name: &str,
        timeout: Duration,
    ) -> anyhow::Result<bool> {
        let sessions = self.sessions.lock().await;
        if let Some(handle) = sessions.get(name) {
            handle.set_auto_unown_timeout(timeout).await;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Checkin all terminals owned by the given owner.
    /// Returns the list of terminal names that were checked in.
    pub async fn checkin_all_by_owner(&self, owner: &str) -> Vec<String> {
        let sessions = self.sessions.lock().await;
        let mut checked_in = Vec::new();

        for (name, handle) in sessions.iter() {
            if handle.checkin(owner).await {
                checked_in.push(name.clone());
            }
        }

        checked_in
    }

    /// Find all terminals owned by the given owner.
    /// Returns the list of terminal names owned by this owner.
    pub async fn find_terminals_by_owner(&self, owner: &str) -> Vec<String> {
        let sessions = self.sessions.lock().await;
        let mut owned = Vec::new();

        for (name, handle) in sessions.iter() {
            if let Some(current_owner) = handle.get_owner().await {
                if current_owner == owner {
                    owned.push(name.clone());
                }
            }
        }

        owned
    }

    /// Force unown a terminal (administrative operation).
    /// Returns the previous owner if the terminal was owned.
    /// Does NOT require ownership.
    pub async fn force_unown_terminal(&self, name: &str) -> anyhow::Result<Option<String>> {
        let sessions = self.sessions.lock().await;
        if let Some(handle) = sessions.get(name) {
            let prev_owner = handle.force_unown().await;
            Ok(prev_owner)
        } else {
            Ok(None)
        }
    }
}

impl Default for TerminalManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_manager_basic_operations() {
        let manager = TerminalManager::new();

        // Initially empty
        assert!(manager.is_empty().await);
        assert_eq!(manager.len().await, 0);

        // Create a session
        let replaced = manager
            .create("test".to_string(), Some(80), Some(24), None, None)
            .await
            .expect("Failed to create session");
        assert!(!replaced);

        // Now has one session
        assert!(!manager.is_empty().await);
        assert_eq!(manager.len().await, 1);
        assert!(manager.contains("test").await);

        // List names
        let names = manager.list_names().await;
        assert_eq!(names, vec!["test"]);

        // Checkout the terminal
        let checked_out = manager
            .checkout_terminal("test", "owner1".to_string())
            .await
            .expect("Failed to checkout");
        assert!(checked_out);

        // Get size (requires ownership)
        let size = manager
            .get_terminal_size("test", "owner1")
            .await
            .expect("Failed to get size");
        assert_eq!(size, Some((80, 24)));

        // Another owner cannot checkout
        let result = manager.checkout_terminal("test", "owner2".to_string()).await;
        assert!(result.is_err());

        // Checkin the terminal
        let checked_in = manager
            .checkin_terminal("test", "owner1")
            .await
            .expect("Failed to checkin");
        assert!(checked_in);

        // Now another owner can checkout
        let checked_out = manager
            .checkout_terminal("test", "owner2".to_string())
            .await
            .expect("Failed to checkout");
        assert!(checked_out);

        // Shutdown the session (doesn't require ownership)
        let shut_down = manager
            .shutdown_terminal("test")
            .await
            .expect("Failed to shutdown");
        assert!(shut_down);

        // Back to empty
        assert!(manager.is_empty().await);
    }
}
