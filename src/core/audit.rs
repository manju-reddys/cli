//! Structured audit logging — one JSON line per event, appended to
//! `~/.craft/audit.log`.
//!
//! # Usage
//! ```rust
//! crate::audit::log(audit::Event::PluginInstalled {
//!     name: "jira-connector",
//!     version: Some("1.0.0"),
//!     source: "/tmp/jira.wasm",
//!     signer_pubkey: Some("d75a3c…"),
//!     hash: "a3f9c2…",
//! });
//! ```
//!
//! # Format
//! ```json
//! {"ts":"2026-03-16T14:23:01.042Z","event":"plugin.installed","actor":"user",
//!  "result":"success","session":"a3f9c2b1","plugin":"jira-connector",…}
//! ```
//!
//! Credential _values_ are never written.  Key names only.

use chrono::Utc;
use serde_json::{Value, json};
use std::sync::OnceLock;

// ─── Session ID ───────────────────────────────────────────────────────────────

static SESSION_ID: OnceLock<String> = OnceLock::new();

/// A short hex identifier that ties all events within one process lifetime
/// together.  Generated once from PID + subsecond timestamp.
pub fn session_id() -> &'static str {
  SESSION_ID.get_or_init(|| {
    let pid = std::process::id();
    let ns = std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .map(|d| d.subsec_nanos())
      .unwrap_or(0);
    format!("{:04x}{:04x}", pid & 0xFFFF, ns & 0xFFFF)
  })
}

// ─── Events ───────────────────────────────────────────────────────────────────

pub enum Event<'a> {
  // ── Plugin lifecycle ───────────────────────────────────────────────────
  PluginInstalled {
    name: &'a str,
    version: Option<&'a str>,
    source: &'a str,
    signer_pubkey: Option<&'a str>,
    hash: &'a str,
  },
  PluginRemoved {
    name: &'a str,
  },
  PluginUpdated {
    name: &'a str,
    old_hash: &'a str,
    new_hash: &'a str,
  },
  PluginSignatureRejected {
    name: &'a str,
    reason: &'a str,
  },
  PluginUnsignedAccepted {
    name: &'a str,
  },

  // ── Plugin execution ───────────────────────────────────────────────────
  PluginRunStarted {
    name: &'a str,
    kind: &'a str,
  },
  PluginRunCompleted {
    name: &'a str,
    duration_ms: u64,
  },
  PluginRunFailed {
    name: &'a str,
    error: &'a str,
    duration_ms: u64,
  },
  PluginRunTimeout {
    name: &'a str,
    timeout_secs: u64,
  },

  // ── Credentials ────────────────────────────────────────────────────────
  CredentialAccessed {
    plugin: &'a str,
    key: &'a str,
  },
  CredentialSet {
    plugin: &'a str,
    key: &'a str,
  },
  CredentialDeleted {
    plugin: &'a str,
    key: &'a str,
  },

  // ── Network sandbox ────────────────────────────────────────────────────
  NetworkAllowed {
    plugin: &'a str,
    domain: &'a str,
  },
  NetworkBlocked {
    plugin: &'a str,
    url: &'a str,
    domain: &'a str,
  },

  // ── Sandbox violations ─────────────────────────────────────────────────
  SandboxMemoryExceeded {
    plugin: &'a str,
    limit_mb: u64,
  },

  // ── Daemon lifecycle ───────────────────────────────────────────────────
  DaemonStarted {
    pid: u32,
  },
  DaemonStopped {
    pid: u32,
    uptime_secs: u64,
  },

  // ── Signing / trust ────────────────────────────────────────────────────
  KeyTrusted {
    fingerprint: &'a str,
    plugin: &'a str,
  },
}

impl<'a> Event<'a> {
  fn name(&self) -> &'static str {
    match self {
      Self::PluginInstalled { .. } => "plugin.installed",
      Self::PluginRemoved { .. } => "plugin.removed",
      Self::PluginUpdated { .. } => "plugin.updated",
      Self::PluginSignatureRejected { .. } => "plugin.signature_rejected",
      Self::PluginUnsignedAccepted { .. } => "plugin.unsigned_accepted",
      Self::PluginRunStarted { .. } => "plugin.run.started",
      Self::PluginRunCompleted { .. } => "plugin.run.completed",
      Self::PluginRunFailed { .. } => "plugin.run.failed",
      Self::PluginRunTimeout { .. } => "plugin.run.timeout",
      Self::CredentialAccessed { .. } => "credential.accessed",
      Self::CredentialSet { .. } => "credential.set",
      Self::CredentialDeleted { .. } => "credential.deleted",
      Self::NetworkAllowed { .. } => "network.allowed",
      Self::NetworkBlocked { .. } => "network.blocked",
      Self::SandboxMemoryExceeded { .. } => "sandbox.memory_exceeded",
      Self::DaemonStarted { .. } => "daemon.started",
      Self::DaemonStopped { .. } => "daemon.stopped",
      Self::KeyTrusted { .. } => "auth.key_trusted",
    }
  }

  fn actor(&self) -> &'static str {
    match self {
      Self::PluginRunStarted { .. }
      | Self::PluginRunCompleted { .. }
      | Self::PluginRunFailed { .. }
      | Self::PluginRunTimeout { .. }
      | Self::CredentialAccessed { .. }
      | Self::NetworkAllowed { .. }
      | Self::NetworkBlocked { .. }
      | Self::SandboxMemoryExceeded { .. }
      | Self::DaemonStarted { .. }
      | Self::DaemonStopped { .. } => "daemon",
      _ => "user",
    }
  }

  fn result(&self) -> &'static str {
    match self {
      Self::PluginSignatureRejected { .. }
      | Self::PluginRunFailed { .. }
      | Self::PluginRunTimeout { .. }
      | Self::NetworkBlocked { .. }
      | Self::SandboxMemoryExceeded { .. } => "blocked",
      _ => "success",
    }
  }

  fn details(&self) -> Value {
    match self {
      Self::PluginInstalled { name, version, source, signer_pubkey, hash } => json!({
        "plugin": name,
        "version": version,
        "source": source,
        "signer_pubkey": signer_pubkey,
        "hash": short(hash),
      }),
      Self::PluginRemoved { name } => json!({ "plugin": name }),
      Self::PluginUpdated { name, old_hash, new_hash } => json!({
        "plugin": name,
        "old_hash": short(old_hash),
        "new_hash": short(new_hash),
      }),
      Self::PluginSignatureRejected { name, reason } => {
        json!({ "plugin": name, "reason": reason })
      }
      Self::PluginUnsignedAccepted { name } => json!({ "plugin": name }),
      Self::PluginRunStarted { name, kind } => json!({ "plugin": name, "kind": kind }),
      Self::PluginRunCompleted { name, duration_ms } => {
        json!({ "plugin": name, "duration_ms": duration_ms })
      }
      Self::PluginRunFailed { name, error, duration_ms } => {
        json!({ "plugin": name, "error": error, "duration_ms": duration_ms })
      }
      Self::PluginRunTimeout { name, timeout_secs } => {
        json!({ "plugin": name, "timeout_secs": timeout_secs })
      }
      Self::CredentialAccessed { plugin, key } => json!({ "plugin": plugin, "key": key }),
      Self::CredentialSet { plugin, key } => json!({ "plugin": plugin, "key": key }),
      Self::CredentialDeleted { plugin, key } => json!({ "plugin": plugin, "key": key }),
      Self::NetworkAllowed { plugin, domain } => json!({ "plugin": plugin, "domain": domain }),
      Self::NetworkBlocked { plugin, url, domain } => {
        json!({ "plugin": plugin, "url": url, "domain": domain })
      }
      Self::SandboxMemoryExceeded { plugin, limit_mb } => {
        json!({ "plugin": plugin, "limit_mb": limit_mb })
      }
      Self::DaemonStarted { pid } => {
        json!({ "pid": pid, "version": env!("CARGO_PKG_VERSION") })
      }
      Self::DaemonStopped { pid, uptime_secs } => {
        json!({ "pid": pid, "uptime_secs": uptime_secs })
      }
      Self::KeyTrusted { fingerprint, plugin } => {
        json!({ "fingerprint": fingerprint, "plugin": plugin })
      }
    }
  }
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Write one audit event to `~/.craft/audit.log`.
///
/// Never panics and never blocks — errors are silently dropped so audit
/// failures never affect the critical path.
pub fn log(event: Event<'_>) {
  let mut details = event.details();
  let fields = details.as_object_mut().expect("details always returns an object");

  // Build ordered JSON: standard envelope first, then event-specific fields.
  let mut map = serde_json::Map::new();
  map.insert("ts".into(), json!(Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)));
  map.insert("event".into(), json!(event.name()));
  map.insert("actor".into(), json!(event.actor()));
  map.insert("result".into(), json!(event.result()));
  map.insert("session".into(), json!(session_id()));
  for (k, v) in fields.iter() {
    map.insert(k.clone(), v.clone());
  }

  let line = serde_json::to_string(&Value::Object(map))
    .unwrap_or_else(|_| r#"{"event":"audit.serialize_error"}"#.to_string());

  write_line(&line);
}

// ─── Writer ───────────────────────────────────────────────────────────────────

fn write_line(line: &str) {
  use std::io::Write as _;

  let path = crate::config::craft_dir().join("audit.log");

  if let Some(parent) = path.parent() {
    let _ = std::fs::create_dir_all(parent);
  }

  // O_APPEND writes are atomic for writes < PIPE_BUF on POSIX (typically
  // 4 KiB) — safe for concurrent CLI and daemon processes on the same file.
  if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
    let _ = writeln!(f, "{line}");
  }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// First 12 chars of a hash string — enough for correlation, not full exposure.
fn short(hash: &str) -> &str {
  &hash[..hash.len().min(12)]
}
