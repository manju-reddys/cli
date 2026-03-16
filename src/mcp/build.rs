//! `craft mcp build` — static analysis + compile pipeline.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

use super::plugin_lang;
use crate::ui;

pub async fn run(dir: Option<&Path>) -> Result<()> {
    // ── 1. Resolve working dir ────────────────────────────────────────────
    let dir: PathBuf = match dir {
        Some(d) => d.to_path_buf(),
        None    => std::env::current_dir().context("getting current directory")?,
    };
    ui::info(format!("Building in {}", dir.display()));

    // ── 2. Detect language ────────────────────────────────────────────────
    let lang = plugin_lang::detect(&dir).ok_or_else(|| {
        anyhow::anyhow!(
            "could not detect plugin language in {}\n\
             Run `craft mcp new <lang>` to scaffold a project.",
            dir.display()
        )
    })?;
    ui::step(format!("Detected language: {lang}"));

    // ── 3. Read plugin name ───────────────────────────────────────────────
    let name = plugin_lang::read_plugin_name(&dir)?;
    ui::step(format!("Plugin name: {name}"));

    // ── 4. Static analysis ────────────────────────────────────────────────
    ui::section("Static analysis");
    let findings = plugin_lang::analyse(lang, &dir)?;

    let mut has_errors = false;
    for f in &findings {
        let rel = f.file.strip_prefix(&dir).unwrap_or(&f.file);
        if f.is_error {
            has_errors = true;
            ui::error_finding(rel.display(), f.line_no, &f.message, &f.line);
        } else {
            ui::warn_finding(rel.display(), f.line_no, &f.message, &f.line);
        }
    }
    if has_errors {
        bail!("static analysis found errors — fix them before building");
    }
    if findings.is_empty() {
        ui::success("No issues found.");
    }

    // ── 5. Compile ────────────────────────────────────────────────────────
    plugin_lang::build(lang, &dir, &name)
}
