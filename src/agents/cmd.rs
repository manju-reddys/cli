use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{add, scaffold};
use crate::ui;

/// All agent-related actions dispatched from root commands
/// (`craft create --agent`, `craft add --agent`, `craft prepare --agent`).
pub enum AgentAction {
  /// Scaffold a new agent package in `out/`.
  Create { name: String, out: PathBuf },
  /// Clean, deploy, and configure an existing agent package.
  Add { name: String, project: PathBuf },
  /// Prepare the agent in the current directory (validate + configure IDE only).
  Prepare { project: PathBuf },
}

impl AgentAction {
  pub fn run(self) -> Result<()> {
    match self {
      AgentAction::Create { name, out } => scaffold::scaffold(&name, &out),
      AgentAction::Add { name, project } => add::run(&name, &project),
      AgentAction::Prepare { project } => prepare(&project),
    }
  }
}

// ─── Prepare ──────────────────────────────────────────────────────────────────

/// `craft prepare --agent` — run from inside an agent package directory.
///
/// Validates that the current directory looks like an agent package
/// (has `metadata.yaml` and a `*.agent.md`), then configures IDE bridges
/// without deploying to `.github/agents/`.
fn prepare(project: &Path) -> Result<()> {
  let cwd = std::env::current_dir()?;

  // Must be run from within an agent package directory.
  let metadata_path = cwd.join("metadata.yaml");
  anyhow::ensure!(
    metadata_path.exists(),
    "no metadata.yaml found in current directory — run this from inside an agent package"
  );

  // Derive agent name from directory name.
  let name = cwd
    .file_name()
    .and_then(|n| n.to_str())
    .ok_or_else(|| anyhow::anyhow!("cannot determine agent name from current directory"))?
    .to_string();

  ui::section(format!("Preparing agent '{name}'"));
  add::run(&name, project)
}
