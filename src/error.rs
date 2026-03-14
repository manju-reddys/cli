use serde_json::json;
use thiserror::Error;

/// Top-level error type for `craft`.
#[derive(Debug, Error)]
pub enum CraftError {
  #[error("daemon unavailable: {0}")]
  DaemonUnavailable(String),

  #[error("plugin not installed: {0}")]
  PluginNotInstalled(String),

  #[error("WASM trap: {0}")]
  WasmTrap(String),

  #[error("credential missing: {0}")]
  CredentialMissing(String),

  #[error("network denied: request to {0} blocked by domain allowlist")]
  NetworkDenied(String),

  #[error("timeout: plugin execution exceeded {0}s limit")]
  Timeout(u64),

  #[error("auth failed: nonce mismatch — unauthorized connection attempt")]
  AuthFailed,

  #[error(transparent)]
  Other(#[from] anyhow::Error),
}

impl CraftError {
  /// Returns the JSON-RPC `reason` string and whether the LLM should retry.
  pub fn rpc_fields(&self) -> (&'static str, bool) {
    match self {
      Self::DaemonUnavailable(_) => ("daemon_unavailable", true),
      Self::PluginNotInstalled(_) => ("plugin_not_installed", false),
      Self::WasmTrap(_) => ("wasm_trap", true),
      Self::CredentialMissing(_) => ("credential_missing", false),
      Self::NetworkDenied(_) => ("network_denied", false),
      Self::Timeout(_) => ("timeout", true),
      Self::AuthFailed => ("auth_failed", false),
      Self::Other(_) => ("internal_error", false),
    }
  }

  /// Write a well-formed JSON-RPC 2.0 error to stdout and return exit code 1.
  ///
  /// Per PRD §10: the client must NEVER silently close the pipe — always write
  /// a structured error so the LLM agent does not hang waiting for a response.
  pub fn write_jsonrpc_error(&self, plugin: &str) {
    let (reason, retry) = self.rpc_fields();
    let payload = json!({
        "jsonrpc": "2.0",
        "id": null,
        "error": {
            "code": -32000,
            "message": format!("craft: plugin execution failed — {plugin}"),
            "data": {
                "reason": reason,
                "detail": self.to_string(),
                "retry": retry,
            }
        }
    });
    println!("{}", serde_json::to_string(&payload).unwrap());
  }
}
