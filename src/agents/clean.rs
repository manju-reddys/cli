use std::collections::HashSet;

use aho_corasick::AhoCorasick;

use super::metadata::AgentMetadata;
use crate::ui;

/// Clean an `.agent.md` before deployment.
///
/// Passes:
///   1. Strip HTML comments, the template instruction block, and unfilled
///      `{{placeholders}}`.
///   2. Remove tool bullet lines referencing tools not in `metadata.tools`.
///   3. Remove command entries not in `metadata.allowed_commands`.
///   4. Scan prose lines for mentions of disallowed tools and warn the user
///      (prose is not auto-removed, only flagged).
///
/// Every removal emits a `ui::warn` explaining why.
/// Returns the cleaned content.
pub fn clean(content: &str, metadata: &AgentMetadata) -> String {
  let tool_set: HashSet<&str> = metadata.tools.iter().map(|s| s.as_str()).collect();
  let cmd_set: HashSet<&str> = metadata.allowed_commands.iter().map(|s| s.as_str()).collect();

  // Build AhoCorasick automaton over disallowed tool names for prose scanning.
  // Only built when there's a non-empty allowlist so we know what's disallowed.
  let disallowed_ac = build_disallowed_ac(content, &tool_set);

  let mut out: Vec<&str> = Vec::new();
  let mut in_template_block = false;
  let mut in_code_block = false;
  let mut section = Section::Other;

  for line in content.lines() {
    let trimmed = line.trim();

    // ── Template instruction block ────────────────────────────────────────
    if trimmed.starts_with("<!--") && trimmed.contains("TEMPLATE INSTRUCTIONS") {
      in_template_block = true;
    }
    if in_template_block {
      if trimmed.ends_with("-->") {
        in_template_block = false;
      }
      continue;
    }

    // ── Single-line HTML comments ─────────────────────────────────────────
    if trimmed.starts_with("<!--") && trimmed.ends_with("-->") {
      continue;
    }

    // ── Unfilled placeholders (AhoCorasick pre-scan) ──────────────────────
    if has_placeholder(trimmed) {
      ui::warn(format!("removed unfilled placeholder: {trimmed}"));
      continue;
    }

    // ── Code fence tracking ───────────────────────────────────────────────
    if trimmed.starts_with("```") {
      in_code_block = !in_code_block;
      out.push(line);
      continue;
    }

    // ── Section detection ─────────────────────────────────────────────────
    if let Some(heading) = parse_heading(trimmed) {
      section = Section::from_heading(heading);
      out.push(line);
      continue;
    }

    // ── Tools section: validate bullet tool names ─────────────────────────
    if section == Section::Tools && !in_code_block {
      if let Some(tool) = extract_backtick_name(trimmed) {
        if !tool_set.is_empty() && !tool_set.contains(tool.as_str()) {
          ui::warn(format!(
            "removed tool '{tool}' — not in metadata.yaml tools allowlist"
          ));
          continue;
        }
      }
    }

    // ── Command allowlist block: validate each command ────────────────────
    if section == Section::CommandAllowlist && in_code_block && !trimmed.is_empty() {
      if !cmd_set.is_empty() && !is_command_allowed(trimmed, &cmd_set) {
        ui::warn(format!(
          "removed command '{trimmed}' — not in metadata.yaml allowed_commands"
        ));
        continue;
      }
    }

    // ── Prose: warn if a disallowed tool is mentioned ─────────────────────
    if section == Section::Other && !in_code_block {
      if let Some(ref ac) = disallowed_ac {
        if ac.is_match(trimmed) {
          ui::warn(format!(
            "line references a disallowed tool (review manually): {trimmed}"
          ));
        }
      }
    }

    out.push(line);
  }

  // Trim trailing blank lines
  while out.last().map(|l: &&str| l.trim().is_empty()).unwrap_or(false) {
    out.pop();
  }

  out.join("\n") + "\n"
}

// ─── AhoCorasick helpers ──────────────────────────────────────────────────────

/// Detect unfilled `{{...}}` placeholders using AhoCorasick.
fn has_placeholder(line: &str) -> bool {
  static AC: std::sync::OnceLock<AhoCorasick> = std::sync::OnceLock::new();
  let ac = AC.get_or_init(|| AhoCorasick::new(["{{", "}}"]).expect("valid patterns"));
  let mut found_open = false;
  let mut found_close = false;
  for mat in ac.find_iter(line) {
    if mat.pattern().as_usize() == 0 {
      found_open = true;
    } else {
      found_close = true;
    }
  }
  found_open && found_close
}

/// Build an AhoCorasick automaton over tool names that appear in `content`
/// but are absent from `allowed`. Returns `None` if no disallowed names found.
fn build_disallowed_ac(content: &str, allowed: &HashSet<&str>) -> Option<AhoCorasick> {
  if allowed.is_empty() {
    return None;
  }
  let mut disallowed: Vec<String> = Vec::new();
  let mut in_tools = false;
  for line in content.lines() {
    let t = line.trim();
    if let Some(h) = parse_heading(t) {
      in_tools = h.to_lowercase().contains("tools");
      continue;
    }
    if in_tools {
      if let Some(name) = extract_backtick_name(t) {
        if !allowed.contains(name.as_str()) {
          disallowed.push(name);
        }
      }
    }
  }
  if disallowed.is_empty() {
    return None;
  }
  AhoCorasick::new(&disallowed).ok()
}

// ─── Markdown / text helpers ──────────────────────────────────────────────────

#[derive(PartialEq)]
enum Section {
  Other,
  Tools,
  CommandAllowlist,
}

impl Section {
  fn from_heading(heading: &str) -> Self {
    match heading.to_lowercase().as_str() {
      h if h.contains("tools") => Section::Tools,
      h if h.contains("command allowlist") => Section::CommandAllowlist,
      _ => Section::Other,
    }
  }
}

/// Return the heading text if the line is an ATX heading (`#+ text`).
fn parse_heading(line: &str) -> Option<&str> {
  let stripped = line.trim_start_matches('#');
  if stripped.len() < line.len() && stripped.starts_with(' ') {
    Some(stripped.trim())
  } else {
    None
  }
}

/// Extract a name from a backtick-wrapped token: `- \`name\`` or `- \`name\` — desc`.
fn extract_backtick_name(line: &str) -> Option<String> {
  let line = line.trim_start_matches(['-', '*', ' ']);
  let inner = line.strip_prefix('`')?;
  let name = inner.split('`').next()?;
  if name.is_empty() { None } else { Some(name.to_string()) }
}

/// Return true if `cmd` exactly matches or prefix-matches any entry in `allowed`.
fn is_command_allowed(cmd: &str, allowed: &HashSet<&str>) -> bool {
  if allowed.contains(cmd) {
    return true;
  }
  allowed.iter().any(|a| cmd.starts_with(&format!("{a} ")))
}
