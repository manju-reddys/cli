use anyhow::Result;

use crate::config::{PluginKind, PluginManifest};
use crate::ui;

/// List all installed plugins with name, kind, source, and hash.
pub async fn list() -> Result<()> {
  let manifests = PluginManifest::list_installed()?;

  if manifests.is_empty() {
    ui::info("No plugins installed.");
    ui::hint("Install one with: craft mcp install <path-to-plugin>");
    return Ok(());
  }

  ui::table_header(&[("NAME", 20), ("KIND", 6), ("SOURCE", 40), ("HASH", 12)]);

  for m in &manifests {
    let kind_str = match m.kind {
      PluginKind::Wasm => "wasm",
      PluginKind::Js => "js",
    };
    let hash_short = if m.source_hash.len() >= 12 { &m.source_hash[..12] } else { &m.source_hash };
    // Truncate on char boundaries to avoid panicking on non-ASCII paths.
    let source_short = if m.source.chars().count() > 38 {
      let tail: String = m.source.chars().rev().take(37).collect::<String>().chars().rev().collect();
      format!("…{tail}")
    } else {
      m.source.clone()
    };
    println!("{:<20} {:<6} {:<40} {:<12}", m.name, kind_str, source_short, hash_short);
  }

  println!("\n{} plugin(s) installed.", manifests.len());
  Ok(())
}
