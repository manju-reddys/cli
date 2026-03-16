use anyhow::Result;
/// GitHub device flow auth — exchanges device code for token, stores in keychain.
pub async fn login() -> Result<()> {
  // TODO: oauth2 device_authorization_url flow
  crate::ui::info("craft auth github: not yet implemented");
  Ok(())
}
