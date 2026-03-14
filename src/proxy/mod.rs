use anyhow::Result;
use clap::Subcommand;

pub mod start;
pub mod status;
pub mod stop;

#[derive(Subcommand)]
pub enum ProxyCommand {
  /// Start a named API proxy plugin
  Start {
    name: String,
    #[arg(long)]
    port: Option<u16>,
  },
  /// Stop a running proxy
  Stop { name: String },
  /// Show status of all running proxies
  Status,
}

impl ProxyCommand {
  pub async fn run(self) -> Result<()> {
    match self {
      ProxyCommand::Start { name, port } => start::start(&name, port).await,
      ProxyCommand::Stop { name } => stop::stop(&name).await,
      ProxyCommand::Status => status::status().await,
    }
  }
}
