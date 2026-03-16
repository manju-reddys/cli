//! `craft mcp new <lang> [name]` — scaffold a new plugin project.

use anyhow::Result;

use super::plugin_lang;

pub async fn run(lang: &str, name: Option<&str>) -> Result<()> {
    let name: String = match name {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => plugin_lang::input("Plugin name")?,
    };

    plugin_lang::scaffold(lang, &name)
}
