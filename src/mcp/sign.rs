use anyhow::Result;

use crate::{signing, ui};

/// Sign a plugin binary and write `craft.sig` alongside it.
///
/// Usage: `craft mcp sign <path-to-plugin>`
///
/// The signing key is loaded from the OS keychain (`craft/signing.key`).
/// If no key exists yet, a fresh Ed25519 keypair is generated and stored.
///
/// Distributable bundle after signing:
///   plugin.wasm  (or plugin.js)
///   craft.config.yaml
///   craft.sig
pub async fn sign(source: &str) -> Result<()> {
  let source_path = std::path::Path::new(source);
  anyhow::ensure!(source_path.exists(), "plugin not found: {source}");

  let binary = std::fs::read(source_path)?;
  let parent = source_path.parent().unwrap_or(std::path::Path::new("."));

  // craft.config.yaml is optional — if present it is included in the signed message.
  let config_path = parent.join("craft.config.yaml");
  let config_yaml: Option<Vec<u8>> = if config_path.exists() {
    Some(std::fs::read(&config_path)?)
  } else {
    None
  };

  // ── Load or generate signing key ─────────────────────────────────────
  let (signing_key, is_new) = signing::load_or_generate_key()?;
  let pubkey_hex = hex::encode(signing_key.verifying_key().as_bytes());

  if is_new {
    ui::success("generated new Ed25519 signing key");
    ui::kv("public key", &pubkey_hex);
    ui::hint("private key stored in OS keychain — it never leaves this machine");
    ui::hint("publish your public key (README / website) so users can verify it");
  } else {
    ui::step(format!("signing with key {}…", &pubkey_hex[..16]));
  }

  // ── Sign ─────────────────────────────────────────────────────────────
  let sig_file = signing::sign(&binary, config_yaml.as_deref(), &signing_key);

  // ── Write craft.sig ──────────────────────────────────────────────────
  let sig_path = parent.join(signing::SIG_FILENAME);
  sig_file.save(&sig_path)?;

  ui::success(format!("signed → {}", sig_path.display()));

  if config_yaml.is_some() {
    ui::detail("signature covers: binary + craft.config.yaml");
  } else {
    ui::detail("signature covers: binary only (no craft.config.yaml found)");
  }

  ui::kv("public key", &pubkey_hex);
  ui::kv("signed at", &sig_file.signed_at);

  Ok(())
}
