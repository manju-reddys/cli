//! Typed IPC message protocol.
//!
//! After the 32-byte nonce handshake, client sends a length-prefixed JSON
//! IpcRequest and the daemon responds with IpcResponse or streams stdio bytes.
//!
//! For MCP stdio mode the client sends RunMcp and then raw stdin bytes follow.
//! For control commands (hot-reload, proxy start/stop, status) the full
//! request/response cycle is JSON only.

use serde::{Deserialize, Serialize};

// ── Requests (client → daemon) ───────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcRequest {
  /// Run an MCP plugin in stdio mode.
  /// Raw stdin bytes follow this JSON frame until client sends EOF.
  RunMcp { plugin: String },

  /// Start an API proxy plugin on the given port.
  StartProxy { plugin: String, port: Option<u16> },

  /// Stop a running proxy.
  StopProxy { plugin: String },

  /// Hot-reload a newly installed / updated plugin module into the cache.
  HotReload { plugin: String },

  /// Evict a removed plugin from the module cache.
  Evict { plugin: String },

  /// Return daemon health / stats.
  Status,
}

// ── Responses (daemon → client) ──────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcResponse {
  /// MCP plugin accepted — raw stdout bytes follow until daemon sends EOF.
  McpReady,

  /// Proxy started successfully.
  ProxyStarted { port: u16 },

  /// Proxy stopped.
  ProxyStopped,

  /// Module reloaded into cache.
  HotReloaded,

  /// Module evicted from cache.
  Evicted,

  /// Daemon status payload.
  Status(DaemonStatus),

  /// Something went wrong — maps to a CraftError reason code.
  Error { reason: String, detail: String, retry: bool },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonStatus {
  pub pid: u32,
  pub uptime_secs: u64,
  pub active_connections: usize,
  pub loaded_modules: usize,
  pub running_proxies: Vec<ProxyInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProxyInfo {
  pub plugin: String,
  pub port: u16,
}

// ── Framing helpers ───────────────────────────────────────────────────────────

/// Encode a request as a 4-byte LE length prefix + JSON bytes.
pub fn encode(msg: &IpcRequest) -> anyhow::Result<Vec<u8>> {
  let json = serde_json::to_vec(msg)?;
  let mut buf = Vec::with_capacity(4 + json.len());
  buf.extend_from_slice(&(json.len() as u32).to_le_bytes());
  buf.extend_from_slice(&json);
  Ok(buf)
}

/// Encode a response as a 4-byte LE length prefix + JSON bytes.
pub fn encode_response(msg: &IpcResponse) -> anyhow::Result<Vec<u8>> {
  let json = serde_json::to_vec(msg)?;
  let mut buf = Vec::with_capacity(4 + json.len());
  buf.extend_from_slice(&(json.len() as u32).to_le_bytes());
  buf.extend_from_slice(&json);
  Ok(buf)
}
