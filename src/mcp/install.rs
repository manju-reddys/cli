use anyhow::{Context, Result};
use dialoguer::{Confirm, Input, Password};

use crate::auth::keychain;
use crate::config::{self, PluginKind, PluginManifest};
use crate::mcp::craft_config::{AuthMethod, CraftConfig, EnvDecl, EnvKind};
use crate::signing::{SignatureFile, SIG_FILENAME};
use crate::ui;

// https://webassembly.github.io/spec/core/binary/modules.html#binary-module
const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6D];

/// Install a plugin from a local path or URL.
///
/// Steps (PRD §4):
/// 0. Verify signature (TOFU) / warn if unsigned
/// 1. Fetch source bytes (local file or HTTPS URL)
/// 2. Validate plugin name — reject path traversal characters
/// 3. Detect kind: WASM (magic bytes) or JS (fallback)
/// 4. BLAKE3 hash the source
/// 5. Copy binary to `~/.craft/plugins/<name>/`
/// 6. Parse `craft.config.yaml` shipped alongside the binary
/// 7. Prompt for each env_var per its declared type; store in OS keychain
/// 8. Write manifest.toml
/// 9. Hot-reload signal to daemon (triggers AOT compilation if running)
pub async fn install(source: &str) -> Result<()> {
  // ── 1. Fetch source bytes ─────────────────────────────────────────────
  let (bytes, source_display) = fetch_source(source).await?;

  // ── 0. Verify signature (must happen before name/copy so abort is clean) ─
  let signer_pubkey = verify_signature(source, &bytes).await?;

  // ── 2. Derive + validate plugin name ─────────────────────────────────
  let name = derive_name(source)?;
  crate::mcp::craft_config::validate_plugin_name(&name)?;

  // ── 3. Detect plugin kind ─────────────────────────────────────────────
  let kind =
    if bytes.len() >= 4 && bytes[..4] == WASM_MAGIC { PluginKind::Wasm } else { PluginKind::Js };

  // ── 4. BLAKE3 hash ────────────────────────────────────────────────────
  let hash = blake3::hash(&bytes).to_hex().to_string();

  // ── 5. Copy binary to plugin dir ──────────────────────────────────────
  let plugin_dir = config::plugin_dir(&name);
  std::fs::create_dir_all(&plugin_dir)?;

  let ext = match kind {
    PluginKind::Wasm => "wasm",
    PluginKind::Js => "js",
  };
  let dest = plugin_dir.join(format!("plugin.{ext}"));
  std::fs::write(&dest, &bytes).with_context(|| format!("writing to {}", dest.display()))?;

  // ── 6. Parse craft.config.yaml ────────────────────────────────────────
  let cfg = load_craft_config(source, &plugin_dir);
  if cfg.is_err() && !is_url(source) {
    // Config is optional — warn but continue
    ui::warn("craft.config.yaml not found; skipping credential setup");
  }

  // ── 7. Prompt for credentials + collect manifest fields ───────────────
  let (env_vars, allowed_domains, version) = match cfg {
    Ok(ref c) => {
      let names = prompt_credentials(&name, c)?;
      (names, c.allowed_domains.clone(), c.version.clone())
    }
    Err(_) => (vec![], vec![], None),
  };

  // ── 8. Write manifest ─────────────────────────────────────────────────
  let manifest = PluginManifest {
    name: name.clone(),
    kind: kind.clone(),
    source: source_display,
    source_hash: hash,
    version,
    signer_pubkey,
    env_vars,
    allowed_domains,
  };
  manifest.save()?;

  crate::audit::log(crate::audit::Event::PluginInstalled {
    name: &name,
    version: manifest.version.as_deref(),
    source: &manifest.source,
    signer_pubkey: manifest.signer_pubkey.as_deref(),
    hash: &manifest.source_hash,
  });

  ui::success(format!("installed {name} ({kind:?}) → {}", plugin_dir.display()));

  // ── 9. Hot-reload to daemon (triggers AOT compilation) ────────────────
  if let Ok(mut stream) = crate::ipc::connect().await {
    use tokio::io::AsyncWriteExt;
    let req = crate::ipc_proto::IpcRequest::HotReload { plugin: name.clone() };
    let frame = crate::ipc_proto::encode(&req)?;
    stream.write_all(&frame).await.ok();
    ui::detail("notified daemon — AOT compilation in progress…");
  } else {
    ui::hint("daemon not running — plugin will compile on first use (~12 s)");
  }

  Ok(())
}

// ─── Signature verification ───────────────────────────────────────────────────

/// Check the signature of the plugin being installed.
///
/// Returns `Some(pubkey_hex)` if signed and trusted, `None` if the user
/// chose to proceed with an unsigned plugin.  Bails on a bad signature.
async fn verify_signature(source: &str, binary: &[u8]) -> Result<Option<String>> {
  // URL installs: we can't fetch craft.sig here — skip with a notice.
  // (Future: fetch <url>.sig alongside the binary.)
  if is_url(source) {
    ui::warn("signature verification skipped for URL installs");
    return Ok(None);
  }

  let parent = std::path::Path::new(source).parent().unwrap_or(std::path::Path::new("."));
  let sig_path = parent.join(SIG_FILENAME);

  // ── No signature file ─────────────────────────────────────────────────
  if !sig_path.exists() {
    ui::warn("this plugin has no craft.sig — it has not been signed by its author");
    let proceed = Confirm::new()
      .with_prompt("install unsigned plugin?")
      .default(false)
      .interact()
      .unwrap_or(false);
    if !proceed {
      crate::audit::log(crate::audit::Event::PluginSignatureRejected {
        name: &derive_name(source).unwrap_or_default(),
        reason: "user declined unsigned install",
      });
      anyhow::bail!("installation cancelled");
    }
    crate::audit::log(crate::audit::Event::PluginUnsignedAccepted {
      name: &derive_name(source).unwrap_or_default(),
    });
    return Ok(None);
  }

  // ── Load and cryptographically verify the signature ───────────────────
  ui::step("verifying signature…");
  let sig_file = SignatureFile::load(&sig_path)?;

  // Read config for signing if it exists alongside the binary.
  let config_path = parent.join("craft.config.yaml");
  let config_yaml: Option<Vec<u8>> =
    if config_path.exists() { Some(std::fs::read(&config_path)?) } else { None };

  // Hard abort on bad signature — do not prompt, do not continue.
  if let Err(e) = crate::signing::verify(binary, config_yaml.as_deref(), &sig_file) {
    crate::audit::log(crate::audit::Event::PluginSignatureRejected {
      name: &derive_name(source).unwrap_or_default(),
      reason: &e.to_string(),
    });
    return Err(e.context("aborting install"));
  }

  ui::success("signature valid");

  // ── TOFU trust check ──────────────────────────────────────────────────
  let mut trusted = crate::signing::TrustedKeys::load();

  if trusted.is_trusted(&sig_file.public_key) {
    // Known key — check if it was originally trusted for a different plugin
    // (could indicate key reuse or impersonation).
    if let Some(orig) = trusted.original_plugin(&sig_file.public_key) {
      let current_name = derive_name(source).unwrap_or_default();
      if orig != current_name {
        ui::warn(format!(
          "this key was previously trusted for '{orig}', now used for '{current_name}'"
        ));
      }
    }
    ui::detail(format!("trusted key {}…", sig_file.fingerprint()));
  } else {
    // Unknown key — TOFU prompt.
    ui::warn(format!("unknown signer — public key: {}", sig_file.public_key));
    ui::hint("verify this key matches what the plugin author published before trusting");

    let trust = Confirm::new()
      .with_prompt("trust this key for future installs?")
      .default(false)
      .interact()
      .unwrap_or(false);

    anyhow::ensure!(trust, "installation cancelled — key not trusted");

    let plugin_name = derive_name(source).unwrap_or_default();
    trusted.trust(&sig_file.public_key, &plugin_name);
    trusted.save()?;
    crate::audit::log(crate::audit::Event::KeyTrusted {
      fingerprint: sig_file.fingerprint(),
      plugin: &plugin_name,
    });
    ui::success(format!("key trusted — stored in ~/.craft/trusted_keys.toml"));
  }

  Ok(Some(sig_file.public_key.clone()))
}

// ─── Source fetching ──────────────────────────────────────────────────────────

fn is_url(source: &str) -> bool {
  source.starts_with("http://") || source.starts_with("https://")
}

async fn fetch_source(source: &str) -> Result<(Vec<u8>, String)> {
  if is_url(source) {
    ui::step("fetching plugin from URL…");
    let resp = reqwest::get(source).await.with_context(|| format!("fetching {source}"))?;
    anyhow::ensure!(resp.status().is_success(), "HTTP {} fetching {source}", resp.status());
    let bytes = resp.bytes().await?.to_vec();
    Ok((bytes, source.to_string()))
  } else {
    let path = std::path::Path::new(source);
    anyhow::ensure!(path.exists(), "source not found: {source}");
    let bytes = std::fs::read(path).with_context(|| format!("reading {source}"))?;
    Ok((bytes, source.to_string()))
  }
}

/// Derive a plugin name from a local path or URL by taking the file stem.
fn derive_name(source: &str) -> Result<String> {
  let base = if is_url(source) {
    source.split('?').next().unwrap_or(source).rsplit('/').next().unwrap_or(source)
  } else {
    return std::path::Path::new(source)
      .file_stem()
      .and_then(|s| s.to_str())
      .map(|s| s.to_string())
      .with_context(|| format!("cannot derive plugin name from: {source}"));
  };
  // Strip any remaining extension from URL basename
  Ok(base.split('.').next().unwrap_or(base).to_string())
}

// ─── craft.config.yaml loading ────────────────────────────────────────────────

/// Copy config file to plugin dir and return parsed CraftConfig.
fn load_craft_config(source: &str, plugin_dir: &std::path::Path) -> Result<CraftConfig> {
  if is_url(source) {
    anyhow::bail!("craft.config.yaml not available for URL installs");
  }
  let yaml_path =
    std::path::Path::new(source).parent().unwrap_or(std::path::Path::new(".")).join(
      "craft.config.yaml",
    );
  anyhow::ensure!(yaml_path.exists(), "craft.config.yaml not found at {}", yaml_path.display());

  // Copy config alongside installed binary so update/list can reference it
  let dst = plugin_dir.join("craft.config.yaml");
  std::fs::copy(&yaml_path, &dst)
    .with_context(|| format!("copying craft.config.yaml to {}", dst.display()))?;

  crate::mcp::craft_config::load(&yaml_path)
}

// ─── Credential prompting ─────────────────────────────────────────────────────

fn prompt_credentials(plugin: &str, cfg: &CraftConfig) -> Result<Vec<String>> {
  if cfg.env.is_empty() {
    return Ok(vec![]);
  }

  ui::section("Plugin configuration");

  let mut env_var_names = Vec::new();
  for decl in &cfg.env {
    prompt_one(plugin, decl, &mut env_var_names)?;
  }
  Ok(env_var_names)
}

fn prompt_one(plugin: &str, decl: &EnvDecl, names: &mut Vec<String>) -> Result<()> {
  match decl.kind {
    // ── fixed: author-set, silent inject ─────────────────────────────────
    EnvKind::Fixed => {
      let val = decl.value.as_deref().unwrap_or("");
      keychain::set(plugin, &decl.name, val)?;
      names.push(decl.name.clone());
    }

    // ── preset: default shown, user may edit ─────────────────────────────
    EnvKind::Preset => {
      let default_val =
        decl.value.as_deref().or(decl.default.as_deref()).unwrap_or("").to_string();
      let prompt_label = prompt_label(decl);
      let val: String = Input::new()
        .with_prompt(&prompt_label)
        .with_initial_text(&default_val)
        .interact_text()
        .with_context(|| format!("prompting for {}", decl.name))?;
      keychain::set(plugin, &decl.name, &val)?;
      names.push(decl.name.clone());
    }

    // ── required: plain text, must provide ───────────────────────────────
    EnvKind::Required => {
      let prompt_label = prompt_label(decl);
      let val: String = Input::new()
        .with_prompt(&prompt_label)
        .interact_text()
        .with_context(|| format!("prompting for {}", decl.name))?;
      keychain::set(plugin, &decl.name, &val)?;
      names.push(decl.name.clone());
    }

    // ── auth: show instructions, then masked input ────────────────────────
    EnvKind::Auth => {
      if let Some(ref instr) = decl.instructions {
        render_auth_instructions(&decl.name, instr);
      }
      match decl.auth_method.as_ref() {
        Some(AuthMethod::Oauth) => {
          ui::warn(format!(
            "{}: OAuth browser flow not yet implemented — skipping",
            decl.name
          ));
          ui::hint(
            "run `craft auth credentials set` after completing OAuth setup manually",
          );
          // OAuth vars are skipped; they'll be wired up in P2.4
        }
        Some(AuthMethod::Basic) => {
          let username: String = Input::new()
            .with_prompt(format!("{} username", decl.name))
            .interact_text()
            .with_context(|| format!("prompting username for {}", decl.name))?;
          let password = Password::new()
            .with_prompt(format!("{} password", decl.name))
            .interact()
            .with_context(|| format!("prompting password for {}", decl.name))?;
          // Convention: "username:password" stored as single keychain entry
          keychain::set(plugin, &decl.name, &format!("{username}:{password}"))?;
          names.push(decl.name.clone());
        }
        _ => {
          // token / pat / apikey (and unspecified auth) — single masked input
          let label = match decl.auth_method.as_ref() {
            Some(AuthMethod::Pat) => "Personal access token",
            Some(AuthMethod::ApiKey) => "API key",
            _ => "Token",
          };
          let val = Password::new()
            .with_prompt(format!("{} ({})", decl.name, label))
            .interact()
            .with_context(|| format!("prompting token for {}", decl.name))?;
          keychain::set(plugin, &decl.name, &val)?;
          names.push(decl.name.clone());
        }
      }
    }
  }
  Ok(())
}

fn prompt_label(decl: &EnvDecl) -> String {
  match &decl.description {
    Some(desc) => format!("{} ({})", decl.name, desc),
    None => decl.name.clone(),
  }
}

// ─── Auth instruction rendering ───────────────────────────────────────────────

fn render_auth_instructions(
  var_name: &str,
  instr: &crate::mcp::craft_config::AuthInstructions,
) {
  ui::section(format!(
    "Setting up {}{}",
    var_name,
    instr.summary.as_deref().map(|s| format!(": {s}")).unwrap_or_default()
  ));

  if let Some(ref url) = instr.url {
    ui::info(format!("Open: {url}"));
  }

  for (i, step) in instr.steps.iter().enumerate() {
    ui::plain(format!("  {}. {step}", i + 1));
  }

  if let Some(ref fmt) = instr.format {
    ui::hint(format!("Expected format: {fmt}"));
  }

  if let Some(ref note) = instr.note {
    ui::warn(note);
  }
}
