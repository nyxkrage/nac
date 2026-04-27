use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Default configuration file name
pub const CONFIG_FILE_NAME: &str = "config.toml";

/// Default config directory relative to home
pub const DEFAULT_CONFIG_DIR: &str = ".config/nac";

/// Environment variable to override config path
pub const NAC_CONFIG_ENV: &str = "NAC_CONFIG";

/// Global configuration state
static GLOBAL_CONFIG: RwLock<Option<Arc<Config>>> = RwLock::new(None);

/// Configuration for API backends
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ApiConfig {
    /// OpenAI API configuration
    pub openai: Option<BackendApiConfig>,
    
    /// Anthropic API configuration  
    pub anthropic: Option<BackendApiConfig>,
    
    /// Google/Gemini API configuration
    pub google: Option<BackendApiConfig>,
    
    /// Ollama/local API configuration
    pub ollama: Option<BackendApiConfig>,
}

/// Per-backend API configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BackendApiConfig {
    /// API key for this backend
    pub api_key: Option<String>,
    
    /// Base URL for API requests (optional, for custom endpoints)
    pub base_url: Option<String>,
    
    /// Default model to use
    pub model: Option<String>,
    
    /// Request timeout in seconds
    pub timeout_secs: Option<u64>,
}

/// Terminal configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TerminalConfig {
    /// Default terminal width (columns)
    pub default_cols: u16,
    
    /// Default terminal height (rows)
    pub default_rows: u16,
    
    /// Minimum terminal width
    pub min_width: u16,
    
    /// Minimum terminal height
    pub min_height: u16,
    
    /// Scrollback buffer size (lines)
    pub scrollback_lines: usize,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            default_cols: 140,
            default_rows: 50,
            min_width: 72,
            min_height: 22,
            scrollback_lines: 10000,
        }
    }
}

/// UI/TUI configuration
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct UiConfig {
    /// Enable animations
    pub animations: Option<bool>,
    
    /// Color theme
    pub theme: Option<String>,
    
    /// Show timestamps in UI
    pub show_timestamps: Option<bool>,
    
    /// Default editor for external editing
    pub editor: Option<String>,
}

/// Sandbox configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SandboxConfig {
    /// Default sandbox image
    pub default_image: String,
    
    /// Default working directory in sandbox
    pub workdir: String,
    
    /// Auto-mount current directory
    pub auto_mount_cwd: bool,
    
    /// Additional default mounts (host:guest format)
    pub mounts: Vec<String>,
    
    /// Default GPUs to expose
    pub gpus: Vec<String>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            default_image: "docker.io/library/alpine:latest".to_string(),
            workdir: "/workspace".to_string(),
            auto_mount_cwd: true,
            mounts: vec![],
            gpus: vec![],
        }
    }
}

/// Storage configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StoreConfig {
    /// Default store path (relative to home or absolute)
    pub path: Option<PathBuf>,
    
    /// Auto-save interval in seconds
    pub auto_save_secs: Option<u64>,
    
    /// Maximum retained episodes per session
    pub max_retained_episodes: Option<usize>,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            path: None,
            auto_save_secs: Some(30),
            max_retained_episodes: Some(100),
        }
    }
}

/// MCP (Model Context Protocol) configuration
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct McpConfig {
    /// MCP servers to connect to
    pub servers: HashMap<String, McpServerConfig>,
}

/// Single MCP server configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpServerConfig {
    /// Command to run the server
    pub command: String,
    
    /// Arguments for the command
    #[serde(default)]
    pub args: Vec<String>,
    
    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,
    
    /// Working directory
    pub cwd: Option<PathBuf>,
    
    /// Auto-connect on startup
    #[serde(default = "default_true")]
    pub auto_connect: bool,
}

fn default_true() -> bool {
    true
}

/// Agent behavior configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentConfig {
    /// Default backend to use
    pub default_backend: Option<String>,
    
    /// Default reasoning effort
    pub default_reasoning_effort: Option<String>,
    
    /// Max tool iterations per request
    pub max_tool_iterations: Option<u32>,
    
    /// Enable tool confirmation prompts
    pub confirm_tools: Option<bool>,
    
    /// Default system prompt additions
    pub system_prompt_extra: Option<String>,
    
    /// Default thread timeout in seconds
    pub thread_timeout_secs: Option<u64>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            default_backend: Some("auto".to_string()),
            default_reasoning_effort: None,
            max_tool_iterations: Some(50),
            confirm_tools: Some(true),
            system_prompt_extra: None,
            thread_timeout_secs: Some(3600),
        }
    }
}

/// Logging configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LogConfig {
    /// Log level (error, warn, info, debug, trace)
    pub level: Option<String>,
    
    /// Log file path
    pub file: Option<PathBuf>,
    
    /// Enable console logging
    #[serde(default = "default_true")]
    pub console: bool,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: Some("info".to_string()),
            file: None,
            console: true,
        }
    }
}

/// Main configuration structure
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Config {
    /// API backend configuration
    #[serde(default)]
    pub api: ApiConfig,
    
    /// Terminal settings
    #[serde(default)]
    pub terminal: TerminalConfig,
    
    /// UI settings
    #[serde(default)]
    pub ui: UiConfig,
    
    /// Sandbox settings
    #[serde(default)]
    pub sandbox: SandboxConfig,
    
    /// Storage settings
    #[serde(default)]
    pub store: StoreConfig,
    
    /// MCP server settings
    #[serde(default)]
    pub mcp: McpConfig,
    
    /// Agent behavior settings
    #[serde(default)]
    pub agent: AgentConfig,
    
    /// Logging settings
    #[serde(default)]
    pub log: LogConfig,
    
    /// Extra key-value pairs for future extensibility
    #[serde(flatten)]
    pub extra: HashMap<String, toml::Value>,
}

/// Configuration loader with file watching
pub struct ConfigManager {
    config_path: PathBuf,
    config: Arc<RwLock<Arc<Config>>>,
    last_modified: Arc<RwLock<Option<SystemTime>>>,
    _watcher: Option<tokio::task::JoinHandle<()>>,
    reload_tx: mpsc::Sender<()>,
}

impl ConfigManager {
    /// Create a new config manager, loading from the default location
    pub fn new() -> Result<Self> {
        let config_path = Self::find_config_path()?;
        Self::with_path(config_path)
    }
    
    /// Create a new config manager with a specific config path
    pub fn with_path(config_path: PathBuf) -> Result<Self> {
        let (reload_tx, _reload_rx) = mpsc::channel(1);
        
        let mut manager = Self {
            config_path: config_path.clone(),
            config: Arc::new(RwLock::new(Arc::new(Config::default()))),
            last_modified: Arc::new(RwLock::new(None)),
            _watcher: None,
            reload_tx,
        };
        
        // Load initial config
        manager.load()?;
        
        // Set global config reference
        manager.update_global();
        
        Ok(manager)
    }
    
    /// Find the configuration file path
    pub fn find_config_path() -> Result<PathBuf> {
        // Check NAC_CONFIG environment variable first
        if let Ok(path) = std::env::var(NAC_CONFIG_ENV) {
            return Ok(PathBuf::from(path));
        }
        
        // Check XDG_CONFIG_HOME
        if let Ok(xdg_config) = std::env::var("XDG_CONFIG_HOME") {
            let path = PathBuf::from(xdg_config).join("nac").join(CONFIG_FILE_NAME);
            if path.exists() {
                return Ok(path);
            }
        }
        
        // Check home directory
        if let Some(home) = dirs::home_dir() {
            let path = home.join(DEFAULT_CONFIG_DIR).join(CONFIG_FILE_NAME);
            if path.exists() {
                return Ok(path);
            }
        }
        
        // Return default path (may not exist yet)
        if let Some(home) = dirs::home_dir() {
            return Ok(home.join(DEFAULT_CONFIG_DIR).join(CONFIG_FILE_NAME));
        }
        
        anyhow::bail!("Could not determine config file path")
    }
    
    /// Get the current configuration
    pub fn get(&self) -> Arc<Config> {
        self.config.read().unwrap().clone()
    }
    
    /// Load configuration from disk
    pub fn load(&mut self) -> Result<()> {
        if !self.config_path.exists() {
            // No config file exists yet, use defaults
            let config = Arc::new(Config::default());
            *self.config.write().unwrap() = config;
            return Ok(());
        }
        
        let content = std::fs::read_to_string(&self.config_path)
            .with_context(|| format!("Failed to read config file: {}", self.config_path.display()))?;
        
        let config: Config = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", self.config_path.display()))?;
        
        // Update last modified time
        let metadata = std::fs::metadata(&self.config_path)?;
        let modified = metadata.modified().ok();
        *self.last_modified.write().unwrap() = modified;
        
        // Update config
        *self.config.write().unwrap() = Arc::new(config);
        self.update_global();
        
        Ok(())
    }
    
    /// Reload configuration from disk
    pub fn reload(&mut self) -> Result<()> {
        self.load()?;
        log::info!("Configuration reloaded from {}", self.config_path.display());
        Ok(())
    }
    
    /// Save current configuration to disk
    pub fn save(&self) -> Result<()> {
        let config = self.get();
        let content = toml::to_string_pretty(&*config)
            .context("Failed to serialize config")?;
        
        // Ensure parent directory exists
        if let Some(parent) = self.config_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create config directory: {}", parent.display()))?;
        }
        
        std::fs::write(&self.config_path, content)
            .with_context(|| format!("Failed to write config file: {}", self.config_path.display()))?;
        
        Ok(())
    }
    
    /// Create a default configuration file if it doesn't exist
    pub fn init_default(&self) -> Result<()> {
        if self.config_path.exists() {
            return Ok(());
        }
        
        self.save()
    }
    
    /// Get the config file path
    pub fn path(&self) -> &Path {
        &self.config_path
    }
    
    /// Update the global static config reference
    fn update_global(&self) {
        let config = self.get();
        if let Ok(mut global) = GLOBAL_CONFIG.write() {
            *global = Some(config);
        }
    }
    
    /// Start watching the config file for changes (async)
    pub async fn start_watching(&mut self) -> Result<()> {
        use tokio::time::{interval, Duration};
        
        let config_path = self.config_path.clone();
        let last_modified = self.last_modified.clone();
        let reload_tx = self.reload_tx.clone();
        let config_lock = self.config.clone();
        
        let handle = tokio::spawn(async move {
            let mut check_interval = interval(Duration::from_secs(2));
            
            loop {
                check_interval.tick().await;
                
                if let Ok(metadata) = tokio::fs::metadata(&config_path).await {
                    if let Ok(modified) = metadata.modified() {
                        let should_reload = {
                            let last = last_modified.read().unwrap();
                            last.map(|t| modified > t).unwrap_or(true)
                        };
                        
                        if should_reload {
                            // Try to reload
                            if let Ok(content) = tokio::fs::read_to_string(&config_path).await {
                                if let Ok(new_config) = toml::from_str::<Config>(&content) {
                                    *config_lock.write().unwrap() = Arc::new(new_config);
                                    *last_modified.write().unwrap() = Some(modified);
                                    let _ = reload_tx.send(()).await;
                                    log::info!("Config auto-reloaded due to file change");
                                }
                            }
                        }
                    }
                }
            }
        });
        
        self._watcher = Some(handle);
        Ok(())
    }
    
    /// Get the reload notification channel
    pub fn reload_notifier(&self) -> mpsc::Receiver<()> {
        let (tx, rx) = mpsc::channel(1);
        drop(tx); // Close immediately so recv returns None
        rx
    }
}

/// Get the global configuration (if initialized)
pub fn global_config() -> Option<Arc<Config>> {
    GLOBAL_CONFIG.read().unwrap().clone()
}

/// Initialize global configuration from default path
pub fn init_global_config() -> Result<Arc<Config>> {
    let manager = ConfigManager::new()?;
    let config = manager.get();
    
    if let Ok(mut global) = GLOBAL_CONFIG.write() {
        *global = Some(config.clone());
    }
    
    Ok(config)
}

/// Initialize global configuration from specific path
pub fn init_global_config_with_path(path: PathBuf) -> Result<Arc<Config>> {
    let manager = ConfigManager::with_path(path)?;
    let config = manager.get();
    
    if let Ok(mut global) = GLOBAL_CONFIG.write() {
        *global = Some(config.clone());
    }
    
    Ok(config)
}

/// Reload global configuration
pub fn reload_global_config() -> Result<Arc<Config>> {
    let path = ConfigManager::find_config_path()?;
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read config: {}", path.display()))?;
    
    let config: Config = toml::from_str(&content)
        .with_context(|| format!("Failed to parse config: {}", path.display()))?;
    
    let config = Arc::new(config);
    
    if let Ok(mut global) = GLOBAL_CONFIG.write() {
        *global = Some(config.clone());
    }
    
    Ok(config)
}

/// Get API key for a specific backend from config only (env vars no longer supported)
pub fn get_api_key(backend: &str) -> Option<String> {
    let config = global_config()?;
    let backend_config = match backend.to_lowercase().as_str() {
        "openai" => config.api.openai.as_ref(),
        "anthropic" => config.api.anthropic.as_ref(),
        "google" | "gemini" => config.api.google.as_ref(),
        "ollama" | "local" => config.api.ollama.as_ref(),
        _ => None,
    };
    
    backend_config.and_then(|bc| bc.api_key.clone())
}

/// Get base URL for a specific backend from config only (env vars no longer supported)
pub fn get_base_url(backend: &str) -> Option<String> {
    let config = global_config()?;
    let backend_config = match backend.to_lowercase().as_str() {
        "openai" => config.api.openai.as_ref(),
        "anthropic" => config.api.anthropic.as_ref(),
        "google" | "gemini" => config.api.google.as_ref(),
        "ollama" | "local" => config.api.ollama.as_ref(),
        _ => None,
    };
    
    backend_config.and_then(|bc| bc.base_url.clone())
}

/// Get default model for a specific backend from config only (env vars no longer supported)
pub fn get_default_model(backend: &str) -> Option<String> {
    let config = global_config()?;
    let backend_config = match backend.to_lowercase().as_str() {
        "openai" => config.api.openai.as_ref(),
        "anthropic" => config.api.anthropic.as_ref(),
        "google" | "gemini" => config.api.google.as_ref(),
        "ollama" | "local" => config.api.ollama.as_ref(),
        _ => None,
    };
    
    backend_config.and_then(|bc| bc.model.clone())
}

/// Get default thread timeout from config (env vars no longer supported)
pub fn get_thread_timeout() -> u64 {
    global_config()
        .and_then(|c| c.agent.thread_timeout_secs)
        .unwrap_or(3600)
}
pub fn generate_sample_config() -> String {
    r#"# NAC Configuration File
# Place this file at ~/.config/nac/config.toml
# or set NAC_CONFIG environment variable to point to your config file

[api.openai]
api_key = "sk-..."
base_url = "https://api.openai.com/v1"  # Optional: custom endpoint
model = "gpt-4o"  # Optional: default model

[api.anthropic]
api_key = "sk-ant-..."
model = "claude-3-5-sonnet-20241022"

[terminal]
default_cols = 140
default_rows = 50
min_width = 72
min_height = 22
scrollback_lines = 10000

[ui]
animations = true
theme = "default"
show_timestamps = true
editor = "vim"  # Default editor for external editing

[sandbox]
default_image = "docker.io/library/alpine:latest"
workdir = "/workspace"
auto_mount_cwd = true
mounts = []
gpus = []

[store]
# path = ".nac/store.db"  # Relative to home or absolute path
auto_save_secs = 30
max_retained_episodes = 100

[agent]
default_backend = "auto"
# default_reasoning_effort = "medium"
max_tool_iterations = 50
confirm_tools = true
thread_timeout_secs = 3600  # Default timeout for thread tool

[log]
level = "info"
# file = "/var/log/nac.log"
console = true

# MCP Servers - uncomment and configure as needed
# [mcp.servers.filesystem]
# command = "npx"
# args = ["-y", "@modelcontextprotocol/server-filesystem", "/home/user"]
# auto_connect = true

# [mcp.servers.git]
# command = "uvx"
# args = ["mcp-server-git", "--repository", "/path/to/repo"]
# auto_connect = true
"#.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;
    
    #[test]
    fn test_parse_sample_config() {
        let sample = generate_sample_config();
        let config: Config = toml::from_str(&sample).expect("Should parse sample config");
        
        assert!(config.api.openai.is_some());
        assert!(config.terminal.default_cols > 0);
    }
    
    #[test]
    fn test_config_roundtrip() {
        let config = Config {
            api: ApiConfig {
                openai: Some(BackendApiConfig {
                    api_key: Some("test-key".to_string()),
                    base_url: Some("https://api.openai.com".to_string()),
                    model: Some("gpt-4".to_string()),
                    timeout_secs: Some(60),
                }),
                ..Default::default()
            },
            terminal: TerminalConfig::default(),
            ..Default::default()
        };
        
        let toml_str = toml::to_string_pretty(&config).expect("Should serialize");
        let parsed: Config = toml::from_str(&toml_str).expect("Should deserialize");
        
        assert_eq!(parsed.api.openai.unwrap().api_key, Some("test-key".to_string()));
    }
    
    #[test]
    fn test_config_manager_load() {
        let mut temp_file = NamedTempFile::new().expect("Should create temp file");
        let config_toml = r#"
[api.openai]
api_key = "test-key"

[terminal]
default_cols = 100
"#;
        temp_file.write_all(config_toml.as_bytes()).expect("Should write");
        
        let manager = ConfigManager::with_path(temp_file.path().to_path_buf())
            .expect("Should create manager");
        
        let config = manager.get();
        assert_eq!(config.api.openai.as_ref().unwrap().api_key, Some("test-key".to_string()));
        assert_eq!(config.terminal.default_cols, 100);
    }
}
