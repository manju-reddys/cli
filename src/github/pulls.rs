use anyhow::Result;
pub async fn list(repo: Option<&str>) -> Result<()> {
    // TODO: octocrab pulls list
    crate::ui::info(format!("craft github prs: not yet implemented (repo={repo:?})"));
    Ok(())
}
