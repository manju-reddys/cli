use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Returns `~/.craft/` — the root of all craft state.
pub fn craft_dir() -> PathBuf {
  dirs::home_dir().expect("cannot determine home directory").join(".craft")
}

pub fn plugins_dir() -> PathBuf {
  craft_dir().join("plugins")
}
pub fn cache_dir() -> PathBuf {
  craft_dir().join("cache")
}
pub fn socket_path() -> PathBuf {
  craft_dir().join("daemon.sock")
}
pub fn pid_path() -> PathBuf {
  craft_dir().join("daemon.pid")
}
pub fn lock_path() -> PathBuf {
  craft_dir().join("daemon.lock")
}
pub fn nonce_path() -> PathBuf {
  craft_dir().join("daemon.nonce")
}
pub fn log_path() -> PathBuf {
  craft_dir().join("daemon.log")
}
pub fn config_path() -> PathBuf {
  craft_dir().join("config.toml")
}
pub fn proxies_toml() -> PathBuf {
  craft_dir().join("proxies.toml")
}

/// Per-plugin directory: `~/.craft/plugins/<name>/`
pub fn plugin_dir(name: &str) -> PathBuf {
  plugins_dir().join(name)
}

/// Per-plugin manifest: `~/.craft/plugins/<name>/manifest.toml`
pub fn plugin_manifest_path(name: &str) -> PathBuf {
  plugin_dir(name).join("manifest.toml")
}

// ─── Plugin manifest ─────────────────────────────────────────────────────────

/// Identifies the runtime engine for a plugin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginKind {
  Wasm,
  Js,
}

/// Per-plugin metadata persisted to `~/.craft/plugins/<name>/manifest.toml`.
#[derive(Debug, Serialize, Deserialize)]
pub struct PluginManifest {
  /// Human-readable plugin name (also the directory name).
  pub name: String,
  /// Runtime kind — determines WASM vs JS execution path.
  pub kind: PluginKind,
  /// Original install source (local path or URL) for `mcp update`.
  pub source: String,
  /// BLAKE3 hash of the plugin binary, hex-encoded.
  pub source_hash: String,
  /// Plugin version from `craft.config.yaml`, if present.
  #[serde(default)]
  pub version: Option<String>,
  /// Ed25519 public key (hex) that signed this plugin at install time.
  /// None means the plugin was installed without a signature.
  #[serde(default)]
  pub signer_pubkey: Option<String>,
  /// Env var keys the plugin expects (values stored in OS keychain).
  #[serde(default)]
  pub env_vars: Vec<String>,
  /// Domain allowlist for outbound HTTP from inside the sandbox.
  #[serde(default)]
  pub allowed_domains: Vec<String>,
}

impl PluginManifest {
  /// Load a manifest from `~/.craft/plugins/<name>/manifest.toml`.
  pub fn load(name: &str) -> Result<Self> {
    let path = plugin_manifest_path(name);
    let raw =
      std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
  }

  /// Persist this manifest to disk.
  pub fn save(&self) -> Result<()> {
    let dir = plugin_dir(&self.name);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("manifest.toml");
    let raw = toml::to_string_pretty(self)?;
    std::fs::write(&path, raw).with_context(|| format!("writing {}", path.display()))
  }

  /// List all installed plugin names by scanning `~/.craft/plugins/*/manifest.toml`.
  pub fn list_installed() -> Result<Vec<Self>> {
    let dir = plugins_dir();
    if !dir.exists() {
      return Ok(vec![]);
    }
    let mut manifests = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
      let entry = entry?;
      if entry.path().is_dir() {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Ok(m) = Self::load(&name) {
          manifests.push(m);
        }
      }
    }
    Ok(manifests)
  }
}

// ─── config.toml structs ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
  pub daemon: DaemonConfig,
  pub execution: ExecutionConfig,
  pub proxy: ProxyConfig,
}

impl Default for Config {
  fn default() -> Self {
    Self {
      daemon: DaemonConfig::default(),
      execution: ExecutionConfig::default(),
      proxy: ProxyConfig::default(),
    }
  }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct DaemonConfig {
  pub idle_timeout_secs: u64,
  pub max_connections: usize,
  pub log_level: String,
  pub log_file: String,
}

impl Default for DaemonConfig {
  fn default() -> Self {
    Self {
      idle_timeout_secs: 300,
      max_connections: 64,
      log_level: "info".into(),
      log_file: "~/.craft/daemon.log".into(),
    }
  }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct ExecutionConfig {
  pub max_memory_mb: u64,
  pub timeout_secs: u64,
}

impl Default for ExecutionConfig {
  fn default() -> Self {
    Self { max_memory_mb: 32, timeout_secs: 30 }
  }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct ProxyConfig {
  pub bind_address: String,
  pub default_port_range: [u16; 2],
}

impl Default for ProxyConfig {
  fn default() -> Self {
    Self { bind_address: "127.0.0.1".into(), default_port_range: [7400, 7500] }
  }
}

impl Config {
  /// Load `~/.craft/config.toml`, returning defaults if file doesn't exist.
  pub fn load() -> Result<Self> {
    let path = config_path();
    if !path.exists() {
      return Ok(Self::default());
    }
    let raw =
      std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
  }

  /// Write current config back to `~/.craft/config.toml`.
  pub fn save(&self) -> Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
      std::fs::create_dir_all(parent)?;
    }
    let raw = toml::to_string_pretty(self)?;
    std::fs::write(&path, raw).with_context(|| format!("writing {}", path.display()))
  }
}
