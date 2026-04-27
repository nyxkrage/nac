use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::terminal::runner::{spawn_runner, TerminalCommand, TerminalResponse};

/// Handle to a terminal session that can be stored in the manager.
/// Contains the session name and the channel sender to communicate with the runner thread.
/// The actual TerminalSession lives in a dedicated thread and is NOT held here.
#[derive(Clone)]
pub struct TerminalHandle {
    pub name: String,
    pub sender: mpsc::Sender<TerminalCommand>,
}

impl TerminalHandle {
    /// Create a new terminal handle
    pub fn new(name: String, sender: mpsc::Sender<TerminalCommand>) -> Self {
        Self { name, sender }
    }

    /// Send a command to the terminal runner
    pub async fn send(&self, cmd: TerminalCommand) -> anyhow::Result<()> {
        self.sender.send(cmd).await?;
        Ok(())
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

    /// Send a command to a specific terminal session.
    /// Returns true if the session existed and the command was sent.
    pub async fn send_command(&self, name: &str, cmd: TerminalCommand) -> anyhow::Result<bool> {
        let sessions = self.sessions.lock().await;
        if let Some(handle) = sessions.get(name) {
            handle.sender.send(cmd).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Write data to a terminal session's PTY.
    /// Returns true if the session existed.
    pub async fn write_to_terminal(&self, name: &str, data: Vec<u8>) -> anyhow::Result<bool> {
        self.send_command(name, TerminalCommand::Write(data)).await
    }

    /// Resize a terminal session.
    /// Returns true if the session existed.
    pub async fn resize_terminal(&self, name: &str, cols: u16, rows: u16) -> anyhow::Result<bool> {
        self.send_command(name, TerminalCommand::Resize { cols, rows })
            .await
    }

    /// Read the current content of a terminal session.
    /// Returns the content string if the session exists.
    pub async fn read_terminal(&self, name: &str) -> anyhow::Result<Option<String>> {
        let sessions = self.sessions.lock().await;
        if let Some(handle) = sessions.get(name) {
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
    pub async fn get_terminal_size(&self, name: &str) -> anyhow::Result<Option<(u16, u16)>> {
        let sessions = self.sessions.lock().await;
        if let Some(handle) = sessions.get(name) {
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
    pub async fn is_alternate_screen(&self, name: &str) -> anyhow::Result<Option<bool>> {
        let sessions = self.sessions.lock().await;
        if let Some(handle) = sessions.get(name) {
            let (resp_tx, resp_rx) = oneshot::channel();
            handle.sender.send(TerminalCommand::IsAlternateScreen { resp_tx }).await?;
            match resp_rx.await {
                Ok(TerminalResponse::AlternateScreen(is_alt)) => Ok(Some(is_alt)),
                Ok(TerminalResponse::Error(e)) => Err(anyhow::anyhow!(e)),
                _ => Err(anyhow::anyhow!("Unexpected response")),
            }
        } else {
            Ok(None)
        }
    }

    /// Shutdown a terminal session by name.
    /// Returns true if the session existed and was shut down.
    pub async fn shutdown_terminal(&self, name: &str) -> anyhow::Result<bool> {
        if let Some(handle) = self.take(name).await {
            handle.sender.send(TerminalCommand::Close).await?;
            Ok(true)
        } else {
            Ok(false)
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

        // Get size
        let size = manager.get_terminal_size("test").await.expect("Failed to get size");
        assert_eq!(size, Some((80, 24)));

        // Shutdown the session
        let shut_down = manager
            .shutdown_terminal("test")
            .await
            .expect("Failed to shutdown");
        assert!(shut_down);

        // Back to empty
        assert!(manager.is_empty().await);
    }
}
