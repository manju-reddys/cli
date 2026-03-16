use anyhow::Result;
use clap::Subcommand;

pub mod build;
pub mod install;
pub mod list;
pub mod new;
pub mod plugin_lang;
pub mod remove;
pub mod run;
pub mod update;

#[derive(Subcommand)]
pub enum McpCommand {
  /// Run a named MCP plugin — proxies stdio to the daemon (LLM agent entry point)
  Run {
    /// Plugin name (e.g. `craft mcp run jira-connector`)
    name: String,
  },
  /// Install a plugin from a local path or URL; prompts for credentials
  Install { source: String },
  /// Re-install from registered source; regenerates .cwasm if wasmtime version changed
  Update { name: String },
  /// Remove a plugin and notify the daemon to evict it
  Remove { name: String },
  /// List installed plugins (name, type, version, cache status)
  List,
  /// Scaffold a new plugin project
  New {
    /// Language: python, js, rust, go
    lang: String,
    /// Plugin name (prompted if omitted)
    name: Option<String>,
  },
  /// Analyse and compile a plugin to .wasm (run from plugin project directory)
  Build {
    /// Project directory (defaults to current directory)
    #[arg(long, short)]
    dir: Option<std::path::PathBuf>,
  },
}

impl McpCommand {
  pub async fn run(self) -> Result<()> {
    match self {
      // Hot path — errors become JSON-RPC error objects (never silent pipe close)
      McpCommand::Run { name } => {
        if let Err(e) = run::run(&name).await {
          crate::error::CraftError::DaemonUnavailable(e.to_string()).write_jsonrpc_error(&name);
          std::process::exit(1);
        }
        Ok(())
      }
      McpCommand::Install { source } => install::install(&source).await,
      McpCommand::Update { name } => update::update(&name).await,
      McpCommand::Remove { name } => remove::remove(&name).await,
      McpCommand::List => list::list().await,
      McpCommand::New { lang, name } => new::run(&lang, name.as_deref()).await,
      McpCommand::Build { dir } => build::run(dir.as_deref()).await,
    }
  }
}
