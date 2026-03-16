//! Language-specific plugin scaffold and build pipeline.
//!
//! To add a new language:
//!   1. Create `plugin_lang/<lang>.rs`
//!   2. Add `pub mod <lang>;` below
//!   3. Add an arm to `detect`, `scaffold`, `analyse`, and `build`

pub mod python;

use anyhow::{bail, Context, Result};
use std::io::{self, BufRead, Write as _};
use std::path::{Path, PathBuf};

// ── Shared types ──────────────────────────────────────────────────────────────

/// A finding from static analysis.
pub struct Finding {
    pub file:     PathBuf,
    pub line_no:  usize,
    pub line:     String,   // source line (for display)
    pub message:  String,
    pub is_error: bool,
}

// ── Language detection ────────────────────────────────────────────────────────

/// Detect which language a plugin project directory uses.
/// Returns a lowercase language identifier or `None` if unrecognised.
pub fn detect(dir: &Path) -> Option<&'static str> {
    if dir.join("wasm_entry.py").exists() || dir.join("app.py").exists() {
        return Some("python");
    }
    if dir.join("package.json").exists() || dir.join("tsconfig.json").exists() {
        return Some("js");
    }
    if dir.join("Cargo.toml").exists() {
        return Some("rust");
    }
    if dir.join("go.mod").exists() {
        return Some("go");
    }
    None
}

// ── Dispatchers ───────────────────────────────────────────────────────────────

/// Scaffold a new plugin project for `lang` into the current directory.
/// The `name` directory is created by the lang module.
pub fn scaffold(lang: &str, name: &str) -> Result<()> {
    match lang {
        "python" => python::scaffold(name),
        other => bail!(
            "unsupported language '{other}'\n\
             Supported: python  (js, rust, go — coming soon)"
        ),
    }
}

/// Run static analysis on the plugin project in `dir`.
pub fn analyse(lang: &str, dir: &Path) -> Result<Vec<Finding>> {
    match lang {
        "python" => python::analyse(dir),
        other => bail!("unsupported language '{other}'"),
    }
}

/// Compile the plugin project in `dir` to `<name>.wasm`.
pub fn build(lang: &str, dir: &Path, name: &str) -> Result<()> {
    match lang {
        "python" => python::build(dir, name),
        other => bail!("unsupported language '{other}'"),
    }
}

// ── Shared I/O helpers (used by lang modules) ─────────────────────────────────

/// Print a prompt and read one line from stdin.
pub fn prompt(msg: &str) -> Result<String> {
    print!("{msg}");
    io::stdout().flush().context("flushing stdout")?;
    let mut line = String::new();
    io::stdin()
        .lock()
        .read_line(&mut line)
        .context("reading stdin")?;
    Ok(line.trim().to_string())
}

/// Write `contents` to `path`, creating the file.
pub fn write_file(path: impl AsRef<Path>, contents: impl AsRef<str>) -> Result<()> {
    let path = path.as_ref();
    std::fs::write(path, contents.as_ref())
        .with_context(|| format!("writing {}", path.display()))
}

/// Read `name = "..."` from `manifest.toml`, or fall back to the directory name.
pub fn read_plugin_name(dir: &Path) -> Result<String> {
    let manifest = dir.join("manifest.toml");
    if manifest.exists() {
        let src = std::fs::read_to_string(&manifest)
            .with_context(|| format!("reading {}", manifest.display()))?;
        for line in src.lines() {
            let t = line.trim();
            if t.starts_with("name") {
                if let Some(eq) = t.find('=') {
                    let val = t[eq + 1..].trim().trim_matches('"').to_string();
                    if !val.is_empty() {
                        return Ok(val);
                    }
                }
            }
        }
    }
    dir.file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .with_context(|| format!("cannot determine plugin name from path {}", dir.display()))
}
