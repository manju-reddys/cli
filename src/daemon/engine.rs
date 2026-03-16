//! wasmtime 42 Engine + component Linker — initialised once at daemon start.
//!
//! wasmtime 42 API notes:
//!   - `wasmtime_wasi::add_to_linker_async`      — standard WASI
//!   - `wasmtime_wasi_http::add_only_http_to_linker` — WASI HTTP (no TCP sockets added)
//!   - `wasmtime::component::Linker<T>`           — component model linker
//!   - Epoch interruption works the same; ticker must be started separately.

#[cfg(feature = "daemon")]
pub mod wasm {
  use crate::config;
  use crate::daemon::handler::StoreState;
  use anyhow::{Context, Result};
  use std::path::PathBuf;
  use wasmtime::{
    Config, Engine, OptLevel, Store,
    component::{Component, Linker},
  };

  pub struct EngineBundle {
    pub engine: Engine,
    pub linker: Linker<StoreState>,
  }

  pub fn init() -> Result<EngineBundle> {
    let mut cfg = Config::new();
    cfg
      .epoch_interruption(true)
      .cranelift_opt_level(OptLevel::SpeedAndSize)
      .wasm_component_model(true);

    let engine = Engine::new(&cfg)?;
    let mut linker: Linker<StoreState> = Linker::new(&engine);

    // Standard WASI: fs, stdio, env, clocks, random
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;

    // WASI HTTP: all guest HTTP intercepted by StoreState::send_request
    wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)?;

    Ok(EngineBundle { engine, linker })
  }

  /// Increment epoch every 100 ms.
  /// Plugins configured with deadline=300 trap after 30 s CPU time.
  pub fn start_epoch_ticker(engine: Engine) {
    tokio::spawn(async move {
      let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
      loop {
        interval.tick().await;
        engine.increment_epoch();
      }
    });
  }

  // ── AOT compile & cache ──────────────────────────────────────────────

  /// Returns the cache path for an AOT-compiled component.
  /// Key: BLAKE3(source) — stored as `~/.craft/cache/<hash>.cwasm`.
  fn cwasm_path(source_hash: &str) -> PathBuf {
    config::cache_dir().join(format!("{source_hash}.cwasm"))
  }

  /// AOT compile a .wasm file to .cwasm and cache it.
  /// Returns the cached .cwasm path.
  pub fn compile_and_cache(
    engine: &Engine,
    wasm_bytes: &[u8],
    source_hash: &str,
  ) -> Result<PathBuf> {
    let cache_path = cwasm_path(source_hash);

    let cwasm = engine.precompile_component(wasm_bytes)?;

    let dir = config::cache_dir();
    std::fs::create_dir_all(&dir)?;
    std::fs::write(&cache_path, &cwasm)
      .with_context(|| format!("writing {}", cache_path.display()))?;

    tracing::info!(path = %cache_path.display(), "cached AOT-compiled component");
    Ok(cache_path)
  }

  /// Load a component — from .cwasm cache if available, otherwise AOT compile first.
  pub fn load_component(engine: &Engine, plugin_name: &str) -> Result<Component> {
    let manifest = config::PluginManifest::load(plugin_name)
      .with_context(|| format!("loading manifest for '{plugin_name}'"))?;

    let cache_path = cwasm_path(&manifest.source_hash);

    if cache_path.exists() {
      tracing::debug!(plugin = plugin_name, "loading from AOT cache");
      // SAFETY: we trust our own cache — the .cwasm was produced by
      // the same engine configuration via precompile_component.
      let component = unsafe { Component::deserialize_file(engine, &cache_path) }?;
      return Ok(component);
    }

    // No cache — compile from source
    tracing::info!(plugin = plugin_name, "AOT compiling (first run or cache miss)");
    let plugin_dir = config::plugin_dir(plugin_name);
    let wasm_path = plugin_dir.join("plugin.wasm");
    let wasm_bytes =
      std::fs::read(&wasm_path).with_context(|| format!("reading {}", wasm_path.display()))?;

    compile_and_cache(engine, &wasm_bytes, &manifest.source_hash)?;

    // SAFETY: we just wrote this cwasm ourselves
    let component = unsafe { Component::deserialize_file(engine, &cache_path) }?;
    Ok(component)
  }

  // ── Ephemeral plugin execution ───────────────────────────────────────

  /// Run a WASM plugin to completion in an ephemeral Store.
  ///
  /// 1. Load plugin manifest → get allowed_domains, env_vars
  /// 2. Load credentials from OS keychain
  /// 3. Load (or AOT compile) the Component
  /// 4. Create Store with WasiCtx (env vars, stdin/stdout piped to IPC)
  /// 5. Set epoch deadline for timeout enforcement
  /// 6. Instantiate and call the WASI command entry point
  pub async fn run_plugin(
    bundle: &EngineBundle,
    plugin_name: &str,
    stdin: tokio::io::DuplexStream,
    stdout: tokio::io::DuplexStream,
    timeout_secs: u64,
  ) -> Result<()> {
    let manifest = config::PluginManifest::load(plugin_name)?;

    // Load credentials from keychain
    let creds =
      crate::auth::keychain::load_all(plugin_name, &manifest.env_vars).unwrap_or_default();

    // Read memory limit from config; fall back to 64 MB if config is unavailable.
    let memory_limit_mb = config::Config::load()
      .map(|c| c.execution.max_memory_mb as u32)
      .unwrap_or(64);

    // Create StoreState with piped stdio + credentials as env vars
    let store_state = StoreState::new(
      manifest.allowed_domains.clone(),
      &creds,
      memory_limit_mb,
      stdin,
      stdout,
    )?;

    let mut store = Store::new(&bundle.engine, store_state);

    // Epoch deadline: 100ms ticks × 10 ticks/sec × timeout_secs
    store.set_epoch_deadline(timeout_secs * 10);

    // Set resource limiter
    store.limiter(|state| state);

    // Load component (from cache or AOT compile)
    let component = load_component(&bundle.engine, plugin_name)?;

    // Instantiate
    let instance = bundle.linker.instantiate_async(&mut store, &component).await?;

    // Traverse the component export tree to reach wasi:cli/run@0.2.0#run.
    // get_export_index returns a ComponentExportIndex usable with get_typed_func.
    let iface_idx = instance
      .get_export_index(&mut store, None, "wasi:cli/run@0.2.0")
      .ok_or_else(|| anyhow::anyhow!("WASM component missing 'wasi:cli/run@0.2.0' export"))?;
    let func_idx = instance
      .get_export_index(&mut store, Some(&iface_idx), "run")
      .ok_or_else(|| anyhow::anyhow!("'wasi:cli/run@0.2.0' missing 'run' function"))?;

    // WASI command run: () -> result<(), ()>  maps to Rust (Result<(), ()>,)
    type RunResult = (std::result::Result<(), ()>,);
    let run_func = instance
      .get_typed_func::<(), RunResult>(&mut store, func_idx)?;

    let run_result = run_func.call_async(&mut store, ()).await;

    // Flush captured stderr — surfaces Python tracebacks on failure.
    let stderr_bytes = store.data().stderr_capture.contents();
    if !stderr_bytes.is_empty() {
        let stderr_text = String::from_utf8_lossy(&stderr_bytes);
        tracing::error!(plugin = plugin_name, stderr = %stderr_text, "WASM stderr");
    }

    let (result,) = run_result?;
    result.map_err(|()| anyhow::anyhow!("WASM run() returned Err"))?;

    Ok(())
  }
}
