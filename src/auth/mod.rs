use anyhow::Result;
use clap::Subcommand;

pub mod github;
pub mod keychain;
pub mod m365;

#[derive(Subcommand)]
pub enum AuthCommand {
  /// Authenticate with GitHub (device flow)
  Github,
  /// Authenticate with Microsoft 365 (PKCE flow)
  M365,
  /// Manage plugin credentials in the OS keychain
  Credentials {
    #[command(subcommand)]
    cmd: CredentialsCommand,
  },
}

#[derive(Subcommand)]
pub enum CredentialsCommand {
  /// Store or update a credential for a plugin
  Set { plugin: String, key: String, value: String },
  /// Remove a credential for a plugin
  Delete { plugin: String, key: String },
  /// List credential keys for a plugin (values never shown)
  List { plugin: String },
}

impl AuthCommand {
  pub async fn run(self) -> Result<()> {
    match self {
      AuthCommand::Github => github::login().await,
      AuthCommand::M365 => m365::login().await,
      AuthCommand::Credentials { cmd } => match cmd {
        CredentialsCommand::Set { plugin, key, value } => {
          keychain::set(&plugin, &key, &value)?;
          crate::ui::success(format!("credential saved: {plugin}/{key}"));
          Ok(())
        }
        CredentialsCommand::Delete { plugin, key } => {
          keychain::delete(&plugin, &key)?;
          crate::ui::success(format!("credential deleted: {plugin}/{key}"));
          Ok(())
        }
        CredentialsCommand::List { plugin } => {
          crate::ui::info(format!("Credentials for plugin '{plugin}': (list not yet implemented)"));
          Ok(())
        }
      },
    }
  }
}
