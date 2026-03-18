use std::path::Path;

use anyhow::{Context, Result};

use super::Assets;
use crate::ui;

/// Scaffold a new agent directory at `dest/<name>/`.
///
/// Creates:
///   <name>.agent.md   — filled from assets/agent_template.md
///   metadata.yaml     — filled from assets/agent_metadata.yaml
pub fn scaffold(name: &str, dest: &Path) -> Result<()> {
  let agent_dir = dest.join(name);
  std::fs::create_dir_all(&agent_dir)
    .with_context(|| format!("creating directory {}", agent_dir.display()))?;

  write_asset("agent_template.md", &format!("{name}.agent.md"), name, &agent_dir)?;
  write_asset("agent_metadata.yaml", "metadata.yaml", name, &agent_dir)?;

  ui::success(format!("created {}/", agent_dir.display()));
  ui::detail(format!("{name}.agent.md"));
  ui::detail("metadata.yaml");

  Ok(())
}

/// Read a bundled asset, replace `{{agent_name}}`, and write to `dest_dir/dest_name`.
fn write_asset(asset: &str, dest_name: &str, agent_name: &str, dest_dir: &Path) -> Result<()> {
  let file =
    Assets::get(asset).with_context(|| format!("bundled asset '{asset}' not found"))?;

  let content = std::str::from_utf8(file.data.as_ref())
    .with_context(|| format!("asset '{asset}' is not valid UTF-8"))?
    .replace("{{agent_name}}", agent_name);

  let dest_path = dest_dir.join(dest_name);
  std::fs::write(&dest_path, content)
    .with_context(|| format!("writing {}", dest_path.display()))
}
