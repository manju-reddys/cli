use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::{agents, auth, cache, config, daemon, github, mcp, proxy, ui};

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

  /// Scaffold a new agent, skill, or workflow
  Create {
    /// Scaffold a new agent with the given name
    #[arg(long, value_name = "NAME")]
    agent: Option<String>,
    /// Directory to create the scaffold in (default: current directory)
    #[arg(long, default_value = ".")]
    out: PathBuf,
  },

  /// Add an agent, skill, or workflow to the current project
  Add {
    /// Agent package name or path to add
    #[arg(long, value_name = "NAME")]
    agent: Option<String>,
    /// Target project root (default: current directory)
    #[arg(long, default_value = ".")]
    project: PathBuf,
  },

  /// Prepare the agent/skill/workflow in the current directory
  Prepare {
    /// Prepare the agent in the current directory
    #[arg(long, action = clap::ArgAction::SetTrue)]
    agent: bool,
    /// Target project root (default: current directory)
    #[arg(long, default_value = ".")]
    project: PathBuf,
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
      Command::Create { agent, out } => match agent {
        Some(name) => agents::AgentAction::Create { name, out }.run(),
        None => {
          ui::warn("specify what to create — e.g. --agent <name>");
          Ok(())
        }
      },
      Command::Add { agent, project } => match agent {
        Some(name) => agents::AgentAction::Add { name, project }.run(),
        None => {
          ui::warn("specify what to add — e.g. --agent <name>");
          Ok(())
        }
      },
      Command::Prepare { agent, project } => {
        if agent {
          agents::AgentAction::Prepare { project }.run()
        } else {
          ui::warn("specify what to prepare — e.g. --agent");
          Ok(())
        }
      }
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
