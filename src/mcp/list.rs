use anyhow::Result;

use crate::config::{cache_dir, plugin_dir, PluginKind, PluginManifest};
use crate::ui;

/// List all installed plugins with name, kind, version, cache status, and source.
pub async fn list() -> Result<()> {
  let manifests = PluginManifest::list_installed()?;

  if manifests.is_empty() {
    ui::info("No plugins installed.");
    ui::hint("Install one with: craft mcp install <path-to-plugin>");
    return Ok(());
  }

  ui::table_header(&[("NAME", 20), ("KIND", 6), ("VERSION", 12), ("CACHED", 6), ("SOURCE", 36)]);

  for m in &manifests {
    let kind_str = match m.kind {
      PluginKind::Wasm => "wasm",
      PluginKind::Js => "js",
    };

    let version = m.version.as_deref().unwrap_or("—");

    // A .cwasm file in the plugin dir means AOT compilation is done.
    let cached = match m.kind {
      PluginKind::Wasm => {
        let cwasm = plugin_dir(&m.name).join("plugin.cwasm");
        if cwasm.exists() { "yes" } else { "no" }
      }
      // JS uses the cache dir for pre-parsed bytecode (optional)
      PluginKind::Js => {
        let js_cache = cache_dir().join(format!("{}.jsc", m.name));
        if js_cache.exists() { "yes" } else { "no" }
      }
    };

    let source_short = if m.source.chars().count() > 34 {
      let tail: String =
        m.source.chars().rev().take(33).collect::<String>().chars().rev().collect();
      format!("…{tail}")
    } else {
      m.source.clone()
    };

    println!(
      "{:<20} {:<6} {:<12} {:<6} {:<36}",
      m.name, kind_str, version, cached, source_short
    );
  }

  println!("\n{} plugin(s) installed.", manifests.len());
  Ok(())
}
