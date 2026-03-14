use anyhow::{Context, Result};

use crate::config::{self, PluginManifest};

/// Remove a plugin: delete its directory, notify daemon to evict, clean keychain.
pub async fn remove(name: &str) -> Result<()> {
  let plugin_dir = config::plugin_dir(name);
  if !plugin_dir.exists() {
    anyhow::bail!("plugin '{name}' is not installed");
  }

  // Load manifest to clean up keychain entries
  if let Ok(manifest) = PluginManifest::load(name) {
    for key in &manifest.env_vars {
      crate::auth::keychain::delete(name, key).ok();
    }
  }

  // Remove plugin directory
  std::fs::remove_dir_all(&plugin_dir)
    .with_context(|| format!("removing {}", plugin_dir.display()))?;

  println!("✓ removed {name}");

  // Notify daemon to evict from cache
  if let Ok(mut stream) = crate::ipc::connect().await {
    use tokio::io::AsyncWriteExt;
    let req = crate::ipc_proto::IpcRequest::Evict { plugin: name.to_string() };
    let frame = crate::ipc_proto::encode(&req)?;
    stream.write_all(&frame).await.ok();
    println!("  ↻ notified daemon to evict {name}");
  }

  Ok(())
}
