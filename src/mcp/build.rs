//! `craft mcp build` — static analysis + compile pipeline.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

use super::plugin_lang;

pub async fn run(dir: Option<&Path>) -> Result<()> {
    // ── 1. Resolve working dir ────────────────────────────────────────────
    let dir: PathBuf = match dir {
        Some(d) => d.to_path_buf(),
        None    => std::env::current_dir().context("getting current directory")?,
    };
    println!("Building in {}", dir.display());

    // ── 2. Detect language ────────────────────────────────────────────────
    let lang = plugin_lang::detect(&dir).ok_or_else(|| {
        anyhow::anyhow!(
            "could not detect plugin language in {}\n\
             Run `craft mcp new <lang>` to scaffold a project.",
            dir.display()
        )
    })?;
    println!("Detected language: {lang}");

    // ── 3. Read plugin name ───────────────────────────────────────────────
    let name = plugin_lang::read_plugin_name(&dir)?;
    println!("Plugin name: {name}");

    // ── 4. Static analysis ────────────────────────────────────────────────
    println!("\nRunning static analysis…");
    let findings = plugin_lang::analyse(lang, &dir)?;

    let mut has_errors = false;
    for f in &findings {
        let rel = f.file.strip_prefix(&dir).unwrap_or(&f.file);
        if f.is_error {
            has_errors = true;
            eprintln!("  ERROR {}:{}: {}\n         {}",
                rel.display(), f.line_no, f.message, f.line.trim());
        } else {
            println!("  WARN  {}:{}: {}\n         {}",
                rel.display(), f.line_no, f.message, f.line.trim());
        }
    }
    if has_errors {
        bail!("static analysis found errors — fix them before building");
    }
    if findings.is_empty() {
        println!("  No issues found.");
    }

    // ── 5. Compile ────────────────────────────────────────────────────────
    plugin_lang::build(lang, &dir, &name)
}
