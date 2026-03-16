use anyhow::Result;
pub async fn login() -> Result<()> {
    // TODO: prompt for GitHub token, store via keychain
    crate::ui::info("craft github auth: not yet implemented");
    Ok(())
}
