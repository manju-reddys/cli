use anyhow::Result;
use clap::Subcommand;

pub mod auth;
pub mod pulls;
pub mod repos;

#[derive(Subcommand)]
pub enum GithubCommand {
  /// List accessible repositories
  Repos,
  /// List / manage pull requests
  Prs {
    #[arg(long)]
    repo: Option<String>,
  },
}

impl GithubCommand {
  pub async fn run(self) -> Result<()> {
    match self {
      GithubCommand::Repos => repos::list().await,
      GithubCommand::Prs { repo } => pulls::list(repo.as_deref()).await,
    }
  }
}
