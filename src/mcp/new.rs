//! `craft mcp new <lang> [name]` — scaffold a new plugin project.

use anyhow::{bail, Result};

use super::plugin_lang;

pub async fn run(lang: &str, name: Option<&str>) -> Result<()> {
    let name: String = match name {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => {
            let n = plugin_lang::prompt("Plugin name: ")?;
            if n.is_empty() {
                bail!("plugin name cannot be empty");
            }
            n
        }
    };

    plugin_lang::scaffold(lang, &name)
}
