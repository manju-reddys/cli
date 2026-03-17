//! Shared terminal-output helpers — consistent colour/style across every command.
//!
//! Never use `println!` / `eprintln!` directly in command handlers; call these
//! instead so the entire CLI has a uniform look.
//!
//! Colour is automatically disabled when stdout/stderr is not a TTY (e.g. pipes,
//! CI) because the `console` crate respects `NO_COLOR` and `TERM=dumb`.

use console::style;

/// `✓ <msg>` — green bold tick for completed actions.
pub fn success(msg: impl std::fmt::Display) {
  println!("{} {msg}", style("✓").green().bold());
}

/// `  ↻ <msg>` — dim follow-up line (e.g. "notified daemon to reload").
pub fn detail(msg: impl std::fmt::Display) {
  println!("  {} {}", style("↻").dim(), style(msg).dim());
}

/// `→ <msg>` — cyan arrow for informational status messages.
pub fn info(msg: impl std::fmt::Display) {
  println!("{} {msg}", style("→").cyan().bold());
}

/// `· <msg>` — dim bullet for in-progress pipeline steps.
pub fn step(msg: impl std::fmt::Display) {
  println!("{} {}", style("·").dim(), style(msg).dim());
}

/// `WARN <msg>` — yellow label, written to stderr.
pub fn warn(msg: impl std::fmt::Display) {
  eprintln!("{} {msg}", style("WARN").yellow().bold());
}

/// `ERROR <file>:<line>  <msg>` — red label, written to stderr.
/// Use for structured finding output (static analysis).  For fatal errors
/// propagate via `anyhow::Result` instead.
pub fn error_finding(file: impl std::fmt::Display, line: usize, msg: &str, src: &str) {
  eprintln!(
    "  {} {}:{}  {}\n         {}",
    style("ERROR").red().bold(),
    file,
    line,
    msg,
    style(src.trim()).dim()
  );
}

/// `WARN  <file>:<line>  <msg>` — yellow label, written to stdout.
pub fn warn_finding(file: impl std::fmt::Display, line: usize, msg: &str, src: &str) {
  println!(
    "  {} {}:{}  {}\n         {}",
    style("WARN ").yellow().bold(),
    file,
    line,
    msg,
    style(src.trim()).dim()
  );
}

/// `  <msg>` — dim indented hint / next-step text.
pub fn hint(msg: impl std::fmt::Display) {
  println!("  {}", style(msg).dim());
}

/// Bold underlined section header with a preceding blank line.
pub fn section(title: impl std::fmt::Display) {
  println!("\n{}", style(title).bold().underlined());
}

/// `  <key>:  <value>` — left column bold, for status/info panels.
pub fn kv(key: &str, value: impl std::fmt::Display) {
  println!("  {}  {value}", style(format!("{key}:")).bold());
}

/// Bold table header row followed by a dim separator rule.
/// `cols` is a slice of `(header_label, column_width)` pairs.
pub fn table_header(cols: &[(&str, usize)]) {
  let row: String =
    cols.iter().map(|(h, w)| format!("{:<w$}", h, w = *w)).collect::<Vec<_>>().join(" ");
  println!("{}", style(row).bold());
  let width: usize = cols.iter().map(|(_, w)| w + 1).sum::<usize>().saturating_sub(1);
  println!("{}", style("─".repeat(width)).dim());
}

/// Print a plain line unchanged (log passthrough, TOML dumps, raw output, etc.).
pub fn plain(msg: impl std::fmt::Display) {
  println!("{msg}");
}
