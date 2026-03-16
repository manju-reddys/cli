use anyhow::{Context, Result};

use crate::config::{self, PluginKind, PluginManifest};
use crate::ui;

/// Re-install a plugin from its registered source.
///
/// Reads the existing manifest to find the original source path,
/// re-reads the binary, recomputes the BLAKE3 hash, and replaces
/// the binary + manifest. Sends HotReload to daemon if running.
pub async fn update(name: &str) -> Result<()> {
  let manifest =
    PluginManifest::load(name).with_context(|| format!("plugin '{name}' is not installed"))?;

  let source_path = std::path::Path::new(&manifest.source);
  anyhow::ensure!(source_path.exists(), "source no longer found: {}", manifest.source);

  let bytes = std::fs::read(source_path).with_context(|| format!("reading {}", manifest.source))?;

  let new_hash = blake3::hash(&bytes).to_hex().to_string();

  if new_hash == manifest.source_hash {
    ui::success(format!("{name} is already up to date"));
    return Ok(());
  }

  // Overwrite plugin binary
  let ext = match manifest.kind {
    PluginKind::Wasm => "wasm",
    PluginKind::Js => "js",
  };
  let dest = config::plugin_dir(name).join(format!("plugin.{ext}"));
  std::fs::copy(source_path, &dest).with_context(|| format!("copying to {}", dest.display()))?;

  // Update manifest with new hash
  let updated = PluginManifest { source_hash: new_hash, ..manifest };
  updated.save()?;

  ui::success(format!("updated {name}"));

  // Notify daemon
  if let Ok(mut stream) = crate::ipc::connect().await {
    use tokio::io::AsyncWriteExt;
    let req = crate::ipc_proto::IpcRequest::HotReload { plugin: name.to_string() };
    let frame = crate::ipc_proto::encode(&req)?;
    stream.write_all(&frame).await.ok();
    ui::detail(format!("notified daemon to hot-reload {name}"));
  }

  Ok(())
}
