use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::{auth, cache, config, daemon, github, mcp, proxy, ui};

/// craft — MCP host and API proxy CLI
#[derive(Parser)]
#[command(name = "craft", version, about, propagate_version = true)]
pub struct Craft {
  #[command(subcommand)]
  pub cmd: Command,
}

#[derive(Subcommand)]
pub enum Command {
  /// Manage and run MCP plugins
  Mcp {
    #[command(subcommand)]
    cmd: mcp::McpCommand,
  },

  /// GitHub commands (repos, PRs, etc.)
  Github {
    #[command(subcommand)]
    cmd: github::GithubCommand,
  },

  /// Authentication and credentials management
  Auth {
    #[command(subcommand)]
    cmd: auth::AuthCommand,
  },

  /// Manage API proxy plugins
  Proxy {
    #[command(subcommand)]
    cmd: proxy::ProxyCommand,
  },

  /// Daemon lifecycle management
  Daemon {
    #[command(subcommand)]
    cmd: daemon::DaemonCommand,
  },

  /// Read / write ~/.craft/config.toml
  Config {
    #[command(subcommand)]
    cmd: Option<ConfigCommand>,
  },

  /// Manage the AOT compilation cache
  Cache {
    #[command(subcommand)]
    cmd: CacheCommand,
  },
}

#[derive(Subcommand)]
pub enum ConfigCommand {
  /// Print current config
  Show,
  /// Set a config value (e.g. daemon.idle_timeout_secs=600)
  Set { kv: String },
}

#[derive(Subcommand)]
pub enum CacheCommand {
  /// Remove compiled .cwasm file(s); plugin will recompile on next run
  Clean {
    /// Plugin name (omit to clean all)
    plugin: Option<String>,
  },
}

impl Craft {
  pub async fn run(self) -> Result<()> {
    match self.cmd {
      Command::Mcp { cmd } => cmd.run().await,
      Command::Github { cmd } => cmd.run().await,
      Command::Auth { cmd } => cmd.run().await,
      Command::Proxy { cmd } => cmd.run().await,
      Command::Daemon { cmd } => cmd.run().await,
      Command::Config { cmd } => run_config(cmd).await,
      Command::Cache { cmd } => match cmd {
        CacheCommand::Clean { plugin } => cache::clean(plugin.as_deref()).await,
      },
    }
  }
}

async fn run_config(cmd: Option<ConfigCommand>) -> Result<()> {
  match cmd {
    None | Some(ConfigCommand::Show) => {
      let cfg = config::Config::load()?;
      ui::plain(toml::to_string_pretty(&cfg)?);
    }
    Some(ConfigCommand::Set { kv }) => {
      ui::info(format!("craft config set {kv}: not yet implemented"));
    }
  }
  Ok(())
}
