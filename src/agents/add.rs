use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use super::clean::clean;
use super::metadata::AgentMetadata;
use crate::ui;

/// Entry point for `craft add --agent <name>`.
///
/// 1. Locate the agent package directory.
/// 2. Load and validate `metadata.yaml`.
/// 3. Read, clean, and deploy `<name>.agent.md` → `<project>/.github/agents/`.
/// 4. Write IDE bridge configs with restrictions from `metadata.yaml`.
pub fn run(name: &str, project: &Path) -> Result<()> {
  // ── 1. Locate agent package ───────────────────────────────────────────
  let agent_dir = locate_agent(name)?;

  // ── 2. Load metadata ──────────────────────────────────────────────────
  let metadata = AgentMetadata::load(&agent_dir.join("metadata.yaml"))?;

  // ── 3. Clean and deploy .agent.md ─────────────────────────────────────
  let agent_md_path = find_agent_md(&agent_dir, name)?;
  let raw = std::fs::read_to_string(&agent_md_path)
    .with_context(|| format!("reading {}", agent_md_path.display()))?;

  ui::section(format!("Adding agent '{name}'"));
  let cleaned = clean(&raw, &metadata);

  let agents_dir = project.join(".github").join("agents");
  std::fs::create_dir_all(&agents_dir)?;
  let dest = agents_dir.join(format!("{name}.agent.md"));
  std::fs::write(&dest, &cleaned)
    .with_context(|| format!("writing {}", dest.display()))?;
  ui::success(format!("{name} → {}", dest.display()));

  // ── 4. IDE bridge configs ─────────────────────────────────────────────
  ui::section("Configuring IDE bridges");
  write_claude_settings(&metadata, project)?;
  write_vscode_settings(name, &metadata, project)?;

  Ok(())
}

// ─── Agent package location ───────────────────────────────────────────────────

/// Resolve an agent package directory from a name or path.
///
/// Resolution order:
///   1. Treat `name` as a literal path — use if it exists.
///   2. Look for `./<name>/` in the current working directory.
fn locate_agent(name: &str) -> Result<PathBuf> {
  let as_path = PathBuf::from(name);
  if as_path.exists() && as_path.is_dir() {
    return Ok(as_path);
  }

  let cwd_relative = std::env::current_dir()?.join(name);
  if cwd_relative.exists() && cwd_relative.is_dir() {
    return Ok(cwd_relative);
  }

  anyhow::bail!(
    "agent package '{name}' not found — \
     expected a directory at './{name}/' containing metadata.yaml"
  )
}

/// Find `<name>.agent.md` inside `pkg_dir`, falling back to any `*.agent.md`.
fn find_agent_md(pkg_dir: &Path, name: &str) -> Result<PathBuf> {
  let exact = pkg_dir.join(format!("{name}.agent.md"));
  if exact.exists() {
    return Ok(exact);
  }
  for entry in std::fs::read_dir(pkg_dir)? {
    let entry = entry?;
    if entry.file_name().to_string_lossy().ends_with(".agent.md") {
      return Ok(entry.path());
    }
  }
  anyhow::bail!(
    "no .agent.md file found in agent package '{name}' ({})",
    pkg_dir.display()
  )
}

// ─── IDE bridge: Claude Code ──────────────────────────────────────────────────

/// Merge agent restrictions into `<project>/.claude/settings.json`.
///
/// Adds `permissions.allow` entries derived from `metadata.yaml`:
///   allowed_paths    → `Read(<path>)` and `Write(<path>)` rules
///   allowed_commands → `Bash(<command>)` rules
fn write_claude_settings(metadata: &AgentMetadata, project: &Path) -> Result<()> {
  let settings_dir = project.join(".claude");
  std::fs::create_dir_all(&settings_dir)?;
  let settings_path = settings_dir.join("settings.json");

  let mut root = load_json_or_default(&settings_path)?;
  let root_obj = root.as_object_mut().context("settings.json must be a JSON object")?;

  let permissions = root_obj.entry("permissions").or_insert_with(|| json!({}));
  let perms_obj = permissions.as_object_mut().context("permissions must be an object")?;

  let allow = perms_obj.entry("allow").or_insert_with(|| json!([]));
  let allow_arr = allow.as_array_mut().context("permissions.allow must be an array")?;

  for path in &metadata.allowed_paths {
    let read_rule = format!("Read({path})");
    let write_rule = format!("Write({path})");
    insert_unique(allow_arr, read_rule);
    insert_unique(allow_arr, write_rule);
  }

  for cmd in &metadata.allowed_commands {
    insert_unique(allow_arr, format!("Bash({cmd})"));
  }

  write_json(&settings_path, &root)?;
  ui::success(format!("Claude Code → {}", settings_path.display()));
  Ok(())
}

// ─── IDE bridge: VS Code ──────────────────────────────────────────────────────

/// Merge agent instruction pointer into `<project>/.vscode/settings.json`.
fn write_vscode_settings(name: &str, metadata: &AgentMetadata, project: &Path) -> Result<()> {
  let settings_dir = project.join(".vscode");
  std::fs::create_dir_all(&settings_dir)?;
  let settings_path = settings_dir.join("settings.json");

  let mut root = load_json_or_default(&settings_path)?;
  let root_obj = root.as_object_mut().context("settings.json must be a JSON object")?;

  let instructions = root_obj
    .entry("github.copilot.chat.codeGeneration.instructions")
    .or_insert_with(|| json!([]));
  let arr = instructions.as_array_mut().context(
    "github.copilot.chat.codeGeneration.instructions must be an array",
  )?;

  let file = format!(".github/agents/{name}.agent.md");
  let already_present =
    arr.iter().any(|v| v.get("file").and_then(|f| f.as_str()) == Some(&file));
  if !already_present {
    arr.push(json!({ "file": file }));
  }

  // MCP entries declared in metadata
  if !metadata.mcp.is_empty() {
    let mcp_servers = root_obj
      .entry("github.copilot.chat.mcpServers")
      .or_insert_with(|| json!({}));
    let servers = mcp_servers.as_object_mut().context("mcpServers must be an object")?;
    for mcp_name in &metadata.mcp {
      servers.entry(mcp_name).or_insert_with(|| {
        json!({ "command": "craft", "args": ["mcp", "run", mcp_name] })
      });
    }
  }

  write_json(&settings_path, &root)?;
  ui::success(format!("VS Code     → {}", settings_path.display()));
  Ok(())
}

// ─── JSON helpers ─────────────────────────────────────────────────────────────

fn load_json_or_default(path: &Path) -> Result<Value> {
  if !path.exists() {
    return Ok(json!({}));
  }
  let raw = std::fs::read_to_string(path)
    .with_context(|| format!("reading {}", path.display()))?;
  serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
  let pretty = serde_json::to_string_pretty(value)?;
  std::fs::write(path, pretty).with_context(|| format!("writing {}", path.display()))
}

fn insert_unique(arr: &mut Vec<Value>, entry: String) {
  if !arr.iter().any(|v| v.as_str() == Some(&entry)) {
    arr.push(json!(entry));
  }
}
