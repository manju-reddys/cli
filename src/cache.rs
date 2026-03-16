use anyhow::Result;
use dialoguer::Confirm;

use crate::{config, ui};

/// Clean the AOT (.cwasm) cache for one plugin or all plugins.
///
/// `craft cache clean [plugin]`
pub async fn clean(plugin: Option<&str>) -> Result<()> {
  match plugin {
    Some(name) => clean_one(name),
    None => clean_all(),
  }
}

// ─── Single plugin ────────────────────────────────────────────────────────────

fn clean_one(name: &str) -> Result<()> {
  let manifest = config::PluginManifest::load(name)?;
  let cwasm = config::cache_dir().join(format!("{}.cwasm", manifest.source_hash));

  if !cwasm.exists() {
    ui::info(format!("no cached .cwasm for '{name}'"));
    return Ok(());
  }

  std::fs::remove_file(&cwasm)?;
  ui::success(format!("cleared cache for '{name}' ({})", cwasm.display()));
  Ok(())
}

// ─── All plugins ──────────────────────────────────────────────────────────────

fn clean_all() -> Result<()> {
  let cache_dir = config::cache_dir();
  if !cache_dir.exists() {
    ui::info("cache directory is empty — nothing to clean");
    return Ok(());
  }

  // Collect .cwasm files before asking
  let files: Vec<_> = std::fs::read_dir(&cache_dir)?
    .filter_map(|e| e.ok())
    .map(|e| e.path())
    .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("cwasm"))
    .collect();

  if files.is_empty() {
    ui::info("no .cwasm files found — nothing to clean");
    return Ok(());
  }

  ui::warn(format!("this will delete {} .cwasm file(s) from {}", files.len(), cache_dir.display()));
  ui::hint("plugins will recompile (~12 s) on next run");

  let confirmed = Confirm::new()
    .with_prompt("continue?")
    .default(false)
    .interact()
    .unwrap_or(false);

  if !confirmed {
    ui::info("aborted");
    return Ok(());
  }

  let mut removed = 0usize;
  for path in &files {
    match std::fs::remove_file(path) {
      Ok(()) => removed += 1,
      Err(e) => ui::warn(format!("failed to remove {}: {e}", path.display())),
    }
  }

  ui::success(format!("removed {removed} .cwasm file(s)"));
  Ok(())
}
