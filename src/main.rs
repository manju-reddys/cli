mod auth;
mod cli;
mod config;
mod daemon;
mod error;
mod github;
mod ipc;
mod ipc_proto;
mod mcp;
mod proxy;

use clap::Parser;
use cli::Craft;

#[tokio::main]
async fn main() {
  // Init tracing before anything else — writes to stderr, never pollutes the
  // JSON-RPC stdout pipe used by `craft mcp run`. Override with CRAFT_LOG or RUST_LOG.
  tracing_subscriber::fmt()
    .with_env_filter(
      tracing_subscriber::EnvFilter::try_from_env("CRAFT_LOG")
        .or_else(|_| tracing_subscriber::EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| "warn".into()),
    )
    .with_target(false)
    .compact()
    .init();

  if let Err(e) = Craft::parse().run().await {
    eprintln!("craft: {e:#}");
    std::process::exit(1);
  }
}
