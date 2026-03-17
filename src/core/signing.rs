//! Ed25519 plugin code signing — sign, verify, and TOFU trust management.
//!
//! # Files on disk
//!
//! `craft.sig`  — written by `craft mcp sign`, shipped alongside the plugin:
//! ```toml
//! public_key = "<64-char hex>"    # Ed25519 verifying key (32 bytes)
//! signature  = "<128-char hex>"   # Ed25519 signature (64 bytes)
//! signed_at  = "2026-03-16T…"
//! ```
//!
//! `~/.craft/trusted_keys.toml`  — TOFU store, written by craft on first install:
//! ```toml
//! [keys."<pubkey_hex>"]
//! plugin     = "jira-connector"
//! trusted_at = "2026-03-16"
//! ```
//!
//! # Canonical message
//!
//! The bytes that are signed:
//! ```
//! craft-plugin-v1
//! binary:<blake3_hex>
//! config:<blake3_hex_or_"none">
//! ```
//! Signing the config hash means a tampered `craft.config.yaml`
//! (e.g. injecting extra env vars or widening `allowed_domains`) also fails
//! verification.

use anyhow::{Context, Result};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};

pub const SIG_FILENAME: &str = "craft.sig";

const KEYCHAIN_SERVICE: &str = "craft";
const KEYCHAIN_KEY: &str = "signing.key";
const MSG_PREFIX: &str = "craft-plugin-v1\n";

// ─── craft.sig ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct SignatureFile {
  /// Hex-encoded Ed25519 verifying key (32 bytes → 64 hex chars).
  pub public_key: String,
  /// Hex-encoded Ed25519 signature (64 bytes → 128 hex chars).
  pub signature: String,
  pub signed_at: String,
}

impl SignatureFile {
  pub fn load(path: &std::path::Path) -> Result<Self> {
    let raw =
      std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
  }

  pub fn save(&self, path: &std::path::Path) -> Result<()> {
    let raw = toml::to_string_pretty(self)?;
    std::fs::write(path, raw).with_context(|| format!("writing {}", path.display()))
  }

  /// Short fingerprint for display: first 16 hex chars of the public key.
  pub fn fingerprint(&self) -> &str {
    let end = self.public_key.len().min(16);
    &self.public_key[..end]
  }
}

// ─── Trusted keys (TOFU store) ────────────────────────────────────────────────

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct TrustedKeys {
  #[serde(default)]
  pub keys: std::collections::HashMap<String, TrustedKey>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TrustedKey {
  /// Plugin name this key was first seen on.
  pub plugin: String,
  pub trusted_at: String,
}

impl TrustedKeys {
  pub fn load() -> Self {
    let path = trusted_keys_path();
    if !path.exists() {
      return Self::default();
    }
    std::fs::read_to_string(&path)
      .ok()
      .and_then(|s| toml::from_str(&s).ok())
      .unwrap_or_default()
  }

  pub fn save(&self) -> Result<()> {
    let path = trusted_keys_path();
    if let Some(p) = path.parent() {
      std::fs::create_dir_all(p)?;
    }
    let raw = toml::to_string_pretty(self)?;
    std::fs::write(&path, raw).with_context(|| format!("writing {}", path.display()))
  }

  pub fn is_trusted(&self, pubkey_hex: &str) -> bool {
    self.keys.contains_key(pubkey_hex)
  }

  pub fn trust(&mut self, pubkey_hex: &str, plugin: &str) {
    self.keys.insert(
      pubkey_hex.to_string(),
      TrustedKey {
        plugin: plugin.to_string(),
        trusted_at: chrono::Utc::now().format("%Y-%m-%d").to_string(),
      },
    );
  }

  /// Return the plugin name this key was originally trusted for, if known.
  pub fn original_plugin(&self, pubkey_hex: &str) -> Option<&str> {
    self.keys.get(pubkey_hex).map(|k| k.plugin.as_str())
  }
}

fn trusted_keys_path() -> std::path::PathBuf {
  crate::config::craft_dir().join("trusted_keys.toml")
}

// ─── Canonical message ────────────────────────────────────────────────────────

fn canonical_message(binary: &[u8], config_yaml: Option<&[u8]>) -> Vec<u8> {
  let binary_hash = blake3::hash(binary).to_hex().to_string();
  let config_hash = config_yaml
    .map(|b| blake3::hash(b).to_hex().to_string())
    .unwrap_or_else(|| "none".to_string());
  format!("{MSG_PREFIX}binary:{binary_hash}\nconfig:{config_hash}").into_bytes()
}

// ─── Sign ─────────────────────────────────────────────────────────────────────

/// Sign a plugin binary (+ optional config) and return the populated
/// `SignatureFile` ready to be written to `craft.sig`.
pub fn sign(binary: &[u8], config_yaml: Option<&[u8]>, signing_key: &SigningKey) -> SignatureFile {
  let msg = canonical_message(binary, config_yaml);
  let sig: Signature = signing_key.sign(&msg);
  SignatureFile {
    public_key: hex::encode(signing_key.verifying_key().as_bytes()),
    signature: hex::encode(sig.to_bytes()),
    signed_at: chrono::Utc::now().to_rfc3339(),
  }
}

// ─── Verify ───────────────────────────────────────────────────────────────────

/// Verify that `sig_file` is a valid signature over `binary` (+ optional
/// `config_yaml`).  Returns the `VerifyingKey` so the caller can do TOFU
/// trust checks.
pub fn verify(
  binary: &[u8],
  config_yaml: Option<&[u8]>,
  sig_file: &SignatureFile,
) -> Result<VerifyingKey> {
  let pub_bytes: [u8; 32] = hex::decode(&sig_file.public_key)
    .context("decoding public key hex")?
    .try_into()
    .map_err(|_| anyhow::anyhow!("public key must be exactly 32 bytes"))?;

  let verifying_key =
    VerifyingKey::from_bytes(&pub_bytes).context("invalid Ed25519 public key")?;

  let sig_bytes: [u8; 64] = hex::decode(&sig_file.signature)
    .context("decoding signature hex")?
    .try_into()
    .map_err(|_| anyhow::anyhow!("signature must be exactly 64 bytes"))?;

  let signature = Signature::from_bytes(&sig_bytes);
  let msg = canonical_message(binary, config_yaml);

  verifying_key
    .verify(&msg, &signature)
    .context("signature verification failed — plugin may have been tampered with")?;

  Ok(verifying_key)
}

// ─── Signing key management ───────────────────────────────────────────────────

/// Load the author signing key from the OS keychain, or generate and store a
/// fresh one.  Returns `(key, is_new)`.
pub fn load_or_generate_key() -> Result<(SigningKey, bool)> {
  match keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_KEY)
    .ok()
    .and_then(|e| e.get_password().ok())
  {
    Some(hex_seed) => {
      let seed: [u8; 32] = hex::decode(&hex_seed)
        .context("decoding signing key from keychain")?
        .try_into()
        .map_err(|_| anyhow::anyhow!("signing key seed must be 32 bytes"))?;
      Ok((SigningKey::from_bytes(&seed), false))
    }
    None => {
      let key = SigningKey::generate(&mut OsRng);
      let hex_seed = hex::encode(key.as_bytes());
      keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_KEY)
        .context("creating keychain entry for signing key")?
        .set_password(&hex_seed)
        .context("storing signing key in OS keychain")?;
      Ok((key, true))
    }
  }
}
