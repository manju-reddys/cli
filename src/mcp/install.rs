use anyhow::{Context, Result};

use crate::config::{self, PluginKind, PluginManifest};
use crate::ui;

// https://webassembly.github.io/spec/core/binary/modules.html#binary-module
/// WASM magic bytes: `\0asm`
const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6D];

/// Install a plugin from a local path or URL.
///
/// Steps (PRD §4):
/// 1. Read source binary from local path (URL fetch TODO)
/// 2. Detect kind: WASM (magic bytes) or JS (fallback)
/// 3. BLAKE3 hash the source
/// 4. Copy binary to `~/.craft/plugins/<name>/`
/// 5. Prompt for each env_var with masked input (rpassword)
/// 6. Save credentials to OS keychain
/// 7. Write manifest.toml
/// 8. Hot-reload signal to daemon if running
pub async fn install(source: &str) -> Result<()> {
  // ── 1. Read source ───────────────────────────────────────────────────
  let source_path = std::path::Path::new(source);
  anyhow::ensure!(source_path.exists(), "source not found: {source}");

  let bytes = std::fs::read(source_path).with_context(|| format!("reading {source}"))?;

  // ── 2. Detect plugin kind ────────────────────────────────────────────
  let kind =
    if bytes.len() >= 4 && bytes[..4] == WASM_MAGIC { PluginKind::Wasm } else { PluginKind::Js };

  // ── 3. Derive name from filename ─────────────────────────────────────
  let name = source_path
    .file_stem()
    .and_then(|s| s.to_str())
    .with_context(|| format!("cannot derive plugin name from path: {source}"))?
    .to_string();

  // ── 4. BLAKE3 hash ───────────────────────────────────────────────────
  let hash = blake3::hash(&bytes).to_hex().to_string();

  // ── 5. Copy binary to plugin dir ─────────────────────────────────────
  let plugin_dir = config::plugin_dir(&name);
  std::fs::create_dir_all(&plugin_dir)?;

  let ext = match kind {
    PluginKind::Wasm => "wasm",
    PluginKind::Js => "js",
  };
  let dest = plugin_dir.join(format!("plugin.{ext}"));
  std::fs::copy(source_path, &dest).with_context(|| format!("copying to {}", dest.display()))?;

  // ── 6. Prompt for credentials ────────────────────────────────────────
  // TODO: read env_vars from a plugin header / package.json / wit metadata
  // For now, skip credential prompting — users can use `craft auth credentials set`
  let env_vars: Vec<String> = vec![];

  // ── 7. Write manifest ────────────────────────────────────────────────
  let manifest = PluginManifest {
    name: name.clone(),
    kind: kind.clone(),
    source: source.to_string(),
    source_hash: hash,
    env_vars,
    allowed_domains: vec![],
  };
  manifest.save()?;

  ui::success(format!("installed {name} ({kind:?}) → {}", plugin_dir.display()));

  // ── 8. Hot-reload to daemon ──────────────────────────────────────────
  if let Ok(mut stream) = crate::ipc::connect().await {
    use tokio::io::AsyncWriteExt;
    let req = crate::ipc_proto::IpcRequest::HotReload { plugin: name.clone() };
    let frame = crate::ipc_proto::encode(&req)?;
    stream.write_all(&frame).await.ok();
    ui::detail(format!("notified daemon to hot-reload {name}"));
  }

  Ok(())
}
