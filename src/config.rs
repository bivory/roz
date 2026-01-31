//! Configuration loading and management.
//!
//! Configuration is loaded with the following precedence:
//! 1. Environment variables (`ROZ_*`)
//! 2. Config file (`~/.roz/config.toml`)
//! 3. Defaults

use crate::error::{Error, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;

/// Main configuration struct.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    /// Storage configuration.
    pub storage: StorageConfig,

    /// Review configuration.
    pub review: ReviewConfig,

    /// Circuit breaker configuration.
    pub circuit_breaker: CircuitBreakerConfig,

    /// Cleanup configuration.
    pub cleanup: CleanupConfig,

    /// External model configuration.
    pub external_models: ExternalModelsConfig,

    /// Template configuration.
    pub templates: TemplateConfig,

    /// Trace configuration.
    pub trace: TraceConfig,
}

/// Storage configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// Path to the roz home directory.
    pub path: PathBuf,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            path: default_roz_home(),
        }
    }
}

/// Review configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ReviewConfig {
    /// Review mode: "always", "prompt", or "never".
    pub mode: ReviewMode,

    /// Gate configuration.
    pub gates: GatesConfig,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            mode: ReviewMode::Prompt,
            gates: GatesConfig::default(),
        }
    }
}

/// Review mode.
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReviewMode {
    /// Review every prompt.
    Always,

    /// Review when #roz prefix used (default).
    #[default]
    Prompt,

    /// Disable review entirely.
    Never,
}

/// Gate configuration for automatic review triggers.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct GatesConfig {
    /// Tool patterns to gate (glob syntax).
    pub tools: Vec<String>,

    /// How long does gate approval last.
    pub approval_scope: ApprovalScope,

    /// Optional TTL for approvals in seconds.
    pub approval_ttl_seconds: Option<u64>,
}

impl GatesConfig {
    /// Check if gates are enabled (tools array is non-empty).
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        !self.tools.is_empty()
    }
}

/// Approval scope for gates.
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalScope {
    /// Once approved, all gated tools allowed until session ends.
    Session,

    /// Approval resets when user sends a new prompt (recommended).
    #[default]
    Prompt,

    /// Every gated tool call requires fresh review.
    Tool,
}

/// Circuit breaker configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CircuitBreakerConfig {
    /// Maximum blocks before tripping.
    pub max_blocks: u32,

    /// Cooldown time in seconds before breaker resets.
    pub cooldown_seconds: u64,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            max_blocks: 3,
            cooldown_seconds: 300,
        }
    }
}

/// Cleanup configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CleanupConfig {
    /// Auto-cleanup sessions older than this many days.
    pub retention_days: u32,
}

impl Default for CleanupConfig {
    fn default() -> Self {
        Self { retention_days: 7 }
    }
}

/// External model configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ExternalModelsConfig {
    /// Path to codex CLI (empty to disable).
    pub codex: String,

    /// Path to gemini CLI (empty to disable).
    pub gemini: String,
}

impl Default for ExternalModelsConfig {
    fn default() -> Self {
        Self {
            codex: "codex".to_string(),
            gemini: "gemini".to_string(),
        }
    }
}

/// Template configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TemplateConfig {
    /// Which template to use: "v1", "v2", "v3", or "random".
    pub active: String,

    /// Weights for random selection.
    pub weights: HashMap<String, u32>,
}

impl Default for TemplateConfig {
    fn default() -> Self {
        let mut weights = HashMap::new();
        weights.insert("default".to_string(), 100);
        Self {
            active: "default".to_string(),
            weights,
        }
    }
}

/// Trace configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TraceConfig {
    /// Maximum trace events per session.
    pub max_events: usize,
}

impl Default for TraceConfig {
    fn default() -> Self {
        Self { max_events: 500 }
    }
}

/// Get the default roz home directory.
fn default_roz_home() -> PathBuf {
    dirs::home_dir().map_or_else(|| PathBuf::from(".roz"), |h| h.join(".roz"))
}

/// Load configuration with precedence: env vars → file → defaults.
///
/// # Errors
///
/// Returns an error if the config file exists but cannot be parsed.
pub fn load_config() -> Result<Config> {
    let mut config = Config::default();

    // Try to load config file
    let config_path = get_config_path();
    if config_path.exists() {
        let contents = fs::read_to_string(&config_path).map_err(Error::Storage)?;
        config = toml::from_str(&contents).map_err(|e| Error::Config(e.to_string()))?;
    }

    // Override with environment variables
    apply_env_overrides(&mut config);

    Ok(config)
}

/// Get the path to the config file.
fn get_config_path() -> PathBuf {
    if let Ok(path) = env::var("ROZ_CONFIG") {
        return PathBuf::from(path);
    }

    if let Ok(home) = env::var("ROZ_HOME") {
        return PathBuf::from(home).join("config.toml");
    }

    default_roz_home().join("config.toml")
}

/// Apply environment variable overrides to config.
fn apply_env_overrides(config: &mut Config) {
    // Storage path
    if let Ok(path) = env::var("ROZ_STORAGE_PATH") {
        config.storage.path = PathBuf::from(path);
    } else if let Ok(home) = env::var("ROZ_HOME") {
        config.storage.path = PathBuf::from(home);
    }

    // Circuit breaker
    if let Ok(val) = env::var("ROZ_MAX_BLOCKS") {
        if let Ok(max) = val.parse() {
            config.circuit_breaker.max_blocks = max;
        }
    }

    if let Ok(val) = env::var("ROZ_COOLDOWN_SECONDS") {
        if let Ok(secs) = val.parse() {
            config.circuit_breaker.cooldown_seconds = secs;
        }
    }

    // Review mode
    if let Ok(mode) = env::var("ROZ_REVIEW_MODE") {
        config.review.mode = match mode.to_lowercase().as_str() {
            "always" => ReviewMode::Always,
            "never" => ReviewMode::Never,
            _ => ReviewMode::Prompt,
        };
    }

    // Trace
    if let Ok(val) = env::var("ROZ_MAX_EVENTS") {
        if let Ok(max) = val.parse() {
            config.trace.max_events = max;
        }
    }

    // Cleanup
    if let Ok(val) = env::var("ROZ_RETENTION_DAYS") {
        if let Ok(days) = val.parse() {
            config.cleanup.retention_days = days;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = Config::default();
        assert_eq!(config.circuit_breaker.max_blocks, 3);
        assert_eq!(config.circuit_breaker.cooldown_seconds, 300);
        assert_eq!(config.trace.max_events, 500);
        assert_eq!(config.cleanup.retention_days, 7);
        assert_eq!(config.review.mode, ReviewMode::Prompt);
    }

    #[test]
    fn gates_config_is_enabled() {
        let empty = GatesConfig::default();
        assert!(!empty.is_enabled());

        let with_tools = GatesConfig {
            tools: vec!["mcp__tissue__*".to_string()],
            ..Default::default()
        };
        assert!(with_tools.is_enabled());
    }

    #[test]
    fn parse_config_toml() {
        let toml = r#"
            [circuit_breaker]
            max_blocks = 5
            cooldown_seconds = 600

            [trace]
            max_events = 1000

            [review]
            mode = "always"

            [review.gates]
            tools = ["mcp__tissue__*"]
            approval_scope = "session"
        "#;

        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.circuit_breaker.max_blocks, 5);
        assert_eq!(config.circuit_breaker.cooldown_seconds, 600);
        assert_eq!(config.trace.max_events, 1000);
        assert_eq!(config.review.mode, ReviewMode::Always);
        assert!(config.review.gates.is_enabled());
        assert_eq!(config.review.gates.approval_scope, ApprovalScope::Session);
    }

    #[test]
    fn partial_config_uses_defaults() {
        let toml = r"
            [circuit_breaker]
            max_blocks = 10
        ";

        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.circuit_breaker.max_blocks, 10);
        assert_eq!(config.circuit_breaker.cooldown_seconds, 300); // Default
        assert_eq!(config.trace.max_events, 500); // Default
    }
}
