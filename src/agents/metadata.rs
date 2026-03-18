use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

/// Parsed `metadata.yaml` for an agent package.
/// Acts as the source of truth for what the agent is allowed to do.
#[derive(Debug, Deserialize)]
pub struct AgentMetadata {
  pub name: String,
  #[serde(default)]
  pub version: String,
  #[serde(default)]
  pub description: String,
  #[serde(default)]
  pub author: String,
  /// Tools the agent is permitted to reference and use.
  #[serde(default)]
  pub tools: Vec<String>,
  /// MCP plugins the agent is permitted to use.
  #[serde(default)]
  pub mcp: Vec<String>,
  /// Environment variables the agent requires at runtime.
  #[serde(default)]
  pub env: Vec<String>,
  /// Glob patterns restricting file access.
  #[serde(default)]
  pub allowed_paths: Vec<String>,
  /// Shell commands the agent is permitted to run.
  #[serde(default)]
  pub allowed_commands: Vec<String>,
}

impl AgentMetadata {
  pub fn load(path: &Path) -> Result<Self> {
    let raw = std::fs::read_to_string(path)
      .with_context(|| format!("reading {}", path.display()))?;
    serde_yml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
  }
}
