use anyhow::Result;
/// Microsoft 365 PKCE auth flow.
pub async fn login() -> Result<()> {
  // TODO: oauth2 PKCE flow (authorization_code + code_verifier)
  crate::ui::info("craft auth m365: not yet implemented");
  Ok(())
}
