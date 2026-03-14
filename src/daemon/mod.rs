use anyhow::Result;
use clap::Subcommand;

pub mod server;
pub mod nonce;

#[cfg(feature = "daemon")]
pub mod engine;
#[cfg(feature = "daemon")]
pub mod js;
#[cfg(feature = "daemon")]
pub mod handler;
#[cfg(feature = "daemon")]
pub mod network;
#[cfg(feature = "daemon")]
pub mod proxy;

#[derive(Subcommand)]
pub enum DaemonCommand {
  /// Start daemon explicitly (idempotent — connects to existing if already running)
  Start,
  /// Stop the running daemon (SIGTERM)
  Stop,
  /// Print PID, uptime, active connections, loaded modules
  Status,
  /// Tail ~/.craft/daemon.log
  Logs,
  /// Internal: enter the daemon accept loop (spawned by client; not for direct use)
  #[command(hide = true)]
  RunInternal,
}

impl DaemonCommand {
  pub async fn run(self) -> Result<()> {
    match self {
      DaemonCommand::Start => crate::ipc::connect().await.map(|_| ()),
      DaemonCommand::Stop => server::stop().await,
      DaemonCommand::Status => server::status().await,
      DaemonCommand::Logs => server::logs().await,
      DaemonCommand::RunInternal => server::run_daemon().await,
    }
  }
}
