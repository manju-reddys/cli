use anyhow::{Context, Result};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::sync::Notify;

use crate::config;
use crate::daemon::nonce;
use crate::ipc_proto;

const PLUGIN_IDLE_SECS: u64 = 300; // 5 minutes
const REAPER_INTERVAL_SECS: u64 = 60;

// ── Shared daemon state ──────────────────────────────────────────────────────

/// Shared state passed to every connection handler via Arc.
pub struct DaemonState {
  pub nonce: [u8; nonce::NONCE_LEN],
  pub start_time: std::time::Instant,
  pub active_connections: std::sync::atomic::AtomicUsize,
  pub loaded_modules: std::sync::atomic::AtomicUsize,
  /// Tracks last-used timestamp per loaded plugin. Used by the idle reaper.
  pub plugin_registry: tokio::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>,
  #[cfg(feature = "daemon")]
  pub engine: crate::daemon::engine::wasm::EngineBundle,
  #[cfg(feature = "daemon")]
  pub js_runtime: rquickjs::AsyncRuntime,
  #[cfg(feature = "daemon")]
  pub running_proxies:
    tokio::sync::Mutex<std::collections::HashMap<String, crate::daemon::proxy::RunningProxy>>,
}

// ── Boot sequence ────────────────────────────────────────────────────────────

/// Start the daemon accept loop. Called when `daemon run-internal` is dispatched.
///
/// Startup sequence (PRD §2.2):
/// 1. Ensure ~/.craft/ exists
/// 2. Acquire exclusive lock on daemon.lock (flock)
/// 3. Write PID to daemon.pid
/// 4. Generate 32-byte nonce → write daemon.nonce (0600)
/// 5. Bind socket (0600), begin accepting
/// 6. On shutdown: remove PID/lock/socket/nonce files
pub async fn run_daemon() -> Result<()> {
  // ── 1. Ensure state directory exists ─────────────────────────────────
  let craft_dir = config::craft_dir();
  std::fs::create_dir_all(&craft_dir)
    .with_context(|| format!("creating {}", craft_dir.display()))?;

  // ── 2. Acquire exclusive lock ────────────────────────────────────────
  let lock = acquire_lock()?;

  // ── 3. Write PID ─────────────────────────────────────────────────────
  let pid = std::process::id();
  std::fs::write(config::pid_path(), pid.to_string()).context("writing daemon.pid")?;
  tracing::info!(pid, "daemon starting");

  // ── 4. Generate nonce ────────────────────────────────────────────────
  let nonce = nonce::generate_and_write()?;

  // ── 5. Init wasmtime + rquickjs engines ──────────────────────────────
  #[cfg(feature = "daemon")]
  let (engine_bundle, js_runtime) = {
    let bundle = crate::daemon::engine::wasm::init().context("initializing wasmtime engine")?;
    tracing::info!("wasmtime engine initialized");
    // Start epoch ticker — ticks every 100ms for timeout enforcement
    crate::daemon::engine::wasm::start_epoch_ticker(bundle.engine.clone());

    let js_rt = crate::daemon::js::build_runtime().await.context("initializing rquickjs engine")?;
    tracing::info!("rquickjs engine initialized");

    (bundle, js_rt)
  };

  // ── 6. Build shared state ────────────────────────────────────────────
  let shutdown = Arc::new(Notify::new());
  let state = Arc::new(DaemonState {
    nonce,
    start_time: std::time::Instant::now(),
    active_connections: std::sync::atomic::AtomicUsize::new(0),
    loaded_modules: std::sync::atomic::AtomicUsize::new(0),
    plugin_registry: tokio::sync::Mutex::new(std::collections::HashMap::new()),
    #[cfg(feature = "daemon")]
    engine: engine_bundle,
    #[cfg(feature = "daemon")]
    js_runtime,
    #[cfg(feature = "daemon")]
    running_proxies: tokio::sync::Mutex::new(std::collections::HashMap::new()),
  });

  // ── 6b. Idle-plugin reaper ───────────────────────────────────────────
  // Evicts any plugin not used for PLUGIN_IDLE_SECS (5 min) from the registry.
  {
    let state = state.clone();
    let reaper_shutdown = shutdown.clone();
    tokio::spawn(async move {
      let mut interval =
        tokio::time::interval(std::time::Duration::from_secs(REAPER_INTERVAL_SECS));
      loop {
        tokio::select! {
            _ = interval.tick() => {
                let mut reg = state.plugin_registry.lock().await;
                let idle = std::time::Duration::from_secs(PLUGIN_IDLE_SECS);
                let before = reg.len();
                reg.retain(|plugin, last_used| {
                    if last_used.elapsed() < idle {
                        true
                    } else {
                        tracing::info!(plugin, "plugin evicted (idle > {}s)", PLUGIN_IDLE_SECS);
                        false
                    }
                });
                let evicted = before - reg.len();
                if evicted > 0 {
                    state.loaded_modules.fetch_sub(evicted, Ordering::Relaxed);
                }
            }
            _ = reaper_shutdown.notified() => break,
        }
      }
    });
  }

  // ── 6. Bind socket ───────────────────────────────────────────────────
  let socket_path = config::socket_path();

  // Remove stale socket if it exists
  let _ = std::fs::remove_file(&socket_path);

  use interprocess::local_socket::{GenericFilePath, ListenerOptions, ToFsName};
  let name = socket_path.clone().to_fs_name::<GenericFilePath>()?;
  let listener = ListenerOptions::new()
    .name(name)
    .create_tokio()
    .with_context(|| format!("binding {}", socket_path.display()))?;

  // Set socket permissions to 0600 (owner only)
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;
  }

  tracing::info!(path = %socket_path.display(), "daemon listening");

  // ── 7. Register signal handler ───────────────────────────────────────
  // Register before spawning so failures surface as errors, not task panics.
  #[cfg(unix)]
  let (mut sigterm, mut sigint) = {
    use tokio::signal::unix::{SignalKind, signal};
    let sigterm = signal(SignalKind::terminate()).context("registering SIGTERM handler")?;
    let sigint = signal(SignalKind::interrupt()).context("registering SIGINT handler")?;
    (sigterm, sigint)
  };

  let sig_shutdown = shutdown.clone();
  tokio::spawn(async move {
    #[cfg(unix)]
    tokio::select! {
        _ = sigterm.recv() => tracing::info!("received SIGTERM"),
        _ = sigint.recv()  => tracing::info!("received SIGINT"),
    }
    #[cfg(not(unix))]
    {
      tokio::signal::ctrl_c().await.ok();
      tracing::info!("received Ctrl-C");
    }
    sig_shutdown.notify_waiters();
  });

  // ── 8. Accept loop ───────────────────────────────────────────────────
  use interprocess::local_socket::traits::tokio::Listener as _;
  loop {
    tokio::select! {
        result = listener.accept() => {
            match result {
                Ok(stream) => {
                    let state = state.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, state).await {
                            tracing::warn!(error = %e, "connection handler failed");
                        }
                    });
                }
                Err(e) => {
                    tracing::warn!(error = %e, "accept failed");
                }
            }
        }
        _ = shutdown.notified() => {
            tracing::info!("shutting down");
            break;
        }
    }
  }

  // ── 9. Cleanup ───────────────────────────────────────────────────────
  cleanup();
  drop(lock);
  tracing::info!("daemon stopped");
  Ok(())
}

// ── Connection handler ───────────────────────────────────────────────────────

async fn handle_connection(
  mut stream: interprocess::local_socket::tokio::Stream,
  state: Arc<DaemonState>,
) -> Result<()> {
  use tokio::io::{AsyncReadExt, AsyncWriteExt};

  struct ConnectionGuard {
    state: Arc<DaemonState>,
  }
  impl ConnectionGuard {
    fn new(state: Arc<DaemonState>) -> Self {
      state.active_connections.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
      Self { state }
    }
  }
  impl Drop for ConnectionGuard {
    fn drop(&mut self) {
      self.state.active_connections.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
  }
  // Enforce connection limit before accepting the connection.
  let max_connections = config::Config::load().map(|c| c.daemon.max_connections).unwrap_or(64);
  let current = state.active_connections.load(std::sync::atomic::Ordering::Relaxed);
  if current >= max_connections {
    tracing::warn!(current, max_connections, "connection limit reached, rejecting");
    return Ok(());
  }

  let _guard = ConnectionGuard::new(state.clone());

  // ── Nonce handshake ──────────────────────────────────────────────────
  let mut received = [0u8; nonce::NONCE_LEN];
  stream.read_exact(&mut received).await.context("reading nonce from client")?;

  if !nonce::verify(&state.nonce, &received) {
    stream.write_all(&[0x00]).await.ok(); // reject
    anyhow::bail!("nonce mismatch — unauthorized connection");
  }
  stream.write_all(&[0x01]).await?; // accept

  // ── Read IPC request frame ───────────────────────────────────────────
  let mut len_buf = [0u8; 4];
  stream.read_exact(&mut len_buf).await.context("reading request length")?;
  let len = u32::from_le_bytes(len_buf) as usize;

  if len > 1024 * 1024 {
    anyhow::bail!("IPC request too large: {len} bytes");
  }

  let mut json_buf = vec![0u8; len];
  stream.read_exact(&mut json_buf).await.context("reading request body")?;

  let request: ipc_proto::IpcRequest =
    serde_json::from_slice(&json_buf).context("parsing IPC request")?;

  tracing::debug!(?request, "received IPC request");

  // ── Dispatch ─────────────────────────────────────────────────────────
  match request {
    ipc_proto::IpcRequest::RunMcp { plugin } => handle_run_mcp(&mut stream, &state, &plugin).await,
    ipc_proto::IpcRequest::Status => handle_status(&mut stream, &state).await,
    ipc_proto::IpcRequest::HotReload { plugin } => {
      tracing::info!(plugin, "hot-reload requested");
      // Register / refresh plugin so its idle timer resets.
      {
        let mut reg = state.plugin_registry.lock().await;
        let is_new = reg.insert(plugin.clone(), std::time::Instant::now()).is_none();
        if is_new {
          state.loaded_modules.fetch_add(1, Ordering::Relaxed);
        }
      }
      let resp = ipc_proto::IpcResponse::HotReloaded;
      let frame = ipc_proto::encode_response(&resp)?;
      stream.write_all(&frame).await?;
      Ok(())
    }
    ipc_proto::IpcRequest::Evict { plugin } => {
      tracing::info!(plugin, "evict requested");
      {
        let mut reg = state.plugin_registry.lock().await;
        if reg.remove(&plugin).is_some() {
          state.loaded_modules.fetch_sub(1, Ordering::Relaxed);
        }
      }
      let resp = ipc_proto::IpcResponse::Evicted;
      let frame = ipc_proto::encode_response(&resp)?;
      stream.write_all(&frame).await?;
      Ok(())
    }
    ipc_proto::IpcRequest::StartProxy { plugin, port } => {
      handle_start_proxy(&mut stream, &state, &plugin, port).await
    }
    ipc_proto::IpcRequest::StopProxy { plugin } => {
      handle_stop_proxy(&mut stream, &state, &plugin).await
    }
  }
}

// ── Command handlers ─────────────────────────────────────────────────────────

async fn handle_run_mcp(
  stream: &mut interprocess::local_socket::tokio::Stream,
  state: &DaemonState,
  plugin: &str,
) -> Result<()> {
  use tokio::io::AsyncWriteExt;

  // Verify plugin is installed
  let manifest = match config::PluginManifest::load(plugin) {
    Ok(m) => m,
    Err(_) => {
      let resp = ipc_proto::IpcResponse::Error {
        reason: "plugin_not_installed".into(),
        detail: format!("plugin '{plugin}' is not installed"),
        retry: false,
      };
      let frame = ipc_proto::encode_response(&resp)?;
      stream.write_all(&frame).await?;
      return Ok(());
    }
  };

  // Register / refresh plugin in activity registry.
  {
    let mut reg = state.plugin_registry.lock().await;
    let is_new = reg.insert(plugin.to_string(), std::time::Instant::now()).is_none();
    if is_new {
      state.loaded_modules.fetch_add(1, Ordering::Relaxed);
    }
  }

  tracing::info!(plugin, "RunMcp — sending McpReady");
  let resp = ipc_proto::IpcResponse::McpReady;
  let frame = ipc_proto::encode_response(&resp)?;
  stream.write_all(&frame).await?;

  // Create duplex streams for piping IPC ↔ WASM stdio
  // The IPC stream carries raw bytes after the McpReady frame.
  // We pipe: IPC read → WASM stdin, WASM stdout → IPC write.
  #[cfg(feature = "daemon")]
  {
    let cfg = config::Config::load().unwrap_or_else(|e| {
      tracing::warn!(error = %e, "failed to load config, using defaults");
      config::Config::default()
    });

    match manifest.kind {
      config::PluginKind::Wasm => {
        // Create duplex channels for stdin/stdout piping
        let (plugin_stdin, mut stdin_writer) = tokio::io::duplex(8192);
        let (mut stdout_reader, plugin_stdout) = tokio::io::duplex(8192);

        let (mut stream_read, mut stream_write) = tokio::io::split(stream);

        // Run the WASM plugin and IO bridging concurrently
        let run_future = crate::daemon::engine::wasm::run_plugin(
          &state.engine,
          plugin,
          plugin_stdin,
          plugin_stdout,
          cfg.execution.timeout_secs,
        );

        let in_future = tokio::io::copy(&mut stream_read, &mut stdin_writer);
        let out_future = tokio::io::copy(&mut stdout_reader, &mut stream_write);

        if let Err(e) = tokio::try_join!(
          async { run_future.await.map_err(std::io::Error::other) },
          in_future,
          out_future
        ) {
          tracing::error!(plugin, error = %e, "WASM plugin failed or disconnected");
        }
      }
      config::PluginKind::Js => {
        // Create duplex channels for stdin/stdout piping
        let (plugin_stdin, mut stdin_writer) = tokio::io::duplex(8192);
        let (mut stdout_reader, plugin_stdout) = tokio::io::duplex(8192);

        let (mut stream_read, mut stream_write) = tokio::io::split(stream);

        // Run the JS plugin and IO bridging concurrently
        let run_future = crate::daemon::js::run_js(
          state,
          plugin,
          plugin_stdin,
          plugin_stdout,
          cfg.execution.timeout_secs,
        );

        let in_future = tokio::io::copy(&mut stream_read, &mut stdin_writer);
        let out_future = tokio::io::copy(&mut stdout_reader, &mut stream_write);

        if let Err(e) = tokio::try_join!(
          async { run_future.await.map_err(std::io::Error::other) },
          in_future,
          out_future
        ) {
          tracing::error!(plugin, error = %e, "JS plugin failed or disconnected");
        }
      }
    }
  }

  Ok(())
}

async fn handle_status(
  stream: &mut interprocess::local_socket::tokio::Stream,
  state: &DaemonState,
) -> Result<()> {
  use tokio::io::AsyncWriteExt;

  #[cfg(feature = "daemon")]
  let running_proxies = {
    let guard = state.running_proxies.lock().await;
    guard
      .iter()
      .map(|(name, p)| ipc_proto::ProxyInfo { plugin: name.clone(), port: p.port })
      .collect()
  };
  #[cfg(not(feature = "daemon"))]
  let running_proxies: Vec<ipc_proto::ProxyInfo> = vec![];

  let status = ipc_proto::DaemonStatus {
    pid: std::process::id(),
    uptime_secs: state.start_time.elapsed().as_secs(),
    active_connections: state.active_connections.load(std::sync::atomic::Ordering::Relaxed),
    loaded_modules: state.loaded_modules.load(std::sync::atomic::Ordering::Relaxed),
    running_proxies,
  };
  let resp = ipc_proto::IpcResponse::Status(status);
  let frame = ipc_proto::encode_response(&resp)?;
  stream.write_all(&frame).await?;
  Ok(())
}

async fn handle_start_proxy(
  stream: &mut interprocess::local_socket::tokio::Stream,
  state: &Arc<DaemonState>,
  plugin: &str,
  port: Option<u16>,
) -> Result<()> {
  use tokio::io::AsyncWriteExt;

  #[cfg(feature = "daemon")]
  {
    // Verify plugin is installed
    if crate::config::PluginManifest::load(plugin).is_err() {
      let resp = ipc_proto::IpcResponse::Error {
        reason: "plugin_not_installed".into(),
        detail: format!("plugin '{plugin}' is not installed"),
        retry: false,
      };
      stream.write_all(&ipc_proto::encode_response(&resp)?).await?;
      return Ok(());
    }

    // Check if already running
    {
      let guard = state.running_proxies.lock().await;
      if let Some(existing) = guard.get(plugin) {
        let resp = ipc_proto::IpcResponse::ProxyStarted { port: existing.port };
        stream.write_all(&ipc_proto::encode_response(&resp)?).await?;
        return Ok(());
      }
    }

    // Resolve port
    let cfg = crate::config::Config::load().unwrap_or_else(|e| {
      tracing::warn!(error = %e, "failed to load config, using defaults");
      crate::config::Config::default()
    });
    let bind_port = match port {
      Some(p) => p,
      None => {
        let [lo, hi] = cfg.proxy.default_port_range;
        match crate::daemon::proxy::find_free_port(lo, hi) {
          Some(p) => p,
          None => {
            let resp = ipc_proto::IpcResponse::Error {
              reason: "no_free_port".into(),
              detail: format!("no free port found in range {lo}–{hi}"),
              retry: false,
            };
            stream.write_all(&ipc_proto::encode_response(&resp)?).await?;
            return Ok(());
          }
        }
      }
    };

    match crate::daemon::proxy::start(plugin.to_string(), bind_port, state.clone()).await {
      Ok(handle) => {
        let bound_port = handle.port;
        state.running_proxies.lock().await.insert(plugin.to_string(), handle);
        let resp = ipc_proto::IpcResponse::ProxyStarted { port: bound_port };
        stream.write_all(&ipc_proto::encode_response(&resp)?).await?;
      }
      Err(e) => {
        let resp = ipc_proto::IpcResponse::Error {
          reason: "bind_failed".into(),
          detail: e.to_string(),
          retry: false,
        };
        stream.write_all(&ipc_proto::encode_response(&resp)?).await?;
      }
    }
  }

  #[cfg(not(feature = "daemon"))]
  {
    let _ = (plugin, port);
    let resp = ipc_proto::IpcResponse::Error {
      reason: "feature_disabled".into(),
      detail: "proxy requires a daemon build (--features daemon)".into(),
      retry: false,
    };
    stream.write_all(&ipc_proto::encode_response(&resp)?).await?;
  }

  Ok(())
}

async fn handle_stop_proxy(
  stream: &mut interprocess::local_socket::tokio::Stream,
  state: &Arc<DaemonState>,
  plugin: &str,
) -> Result<()> {
  use tokio::io::AsyncWriteExt;

  #[cfg(feature = "daemon")]
  {
    let removed = state.running_proxies.lock().await.remove(plugin);
    if let Some(handle) = removed {
      handle.stop();
      tracing::info!(plugin, "proxy stopped");
      let resp = ipc_proto::IpcResponse::ProxyStopped;
      stream.write_all(&ipc_proto::encode_response(&resp)?).await?;
    } else {
      let resp = ipc_proto::IpcResponse::Error {
        reason: "not_running".into(),
        detail: format!("proxy '{plugin}' is not running"),
        retry: false,
      };
      stream.write_all(&ipc_proto::encode_response(&resp)?).await?;
    }
  }

  #[cfg(not(feature = "daemon"))]
  {
    let _ = plugin;
    let resp = ipc_proto::IpcResponse::ProxyStopped;
    stream.write_all(&ipc_proto::encode_response(&resp)?).await?;
  }

  Ok(())
}

// ── Lifecycle commands (called from client CLI) ──────────────────────────────

pub async fn stop() -> Result<()> {
  let path = config::pid_path();
  let pid_str = std::fs::read_to_string(&path).unwrap_or_default();
  let pid: u32 = pid_str.trim().parse().unwrap_or(0);

  if pid == 0 {
    crate::ui::info("craft daemon: not running");
    return Ok(());
  }

  #[cfg(unix)]
  unsafe {
    libc::kill(pid as libc::pid_t, libc::SIGTERM);
  }

  crate::ui::success(format!("sent SIGTERM to daemon (pid={pid})"));
  Ok(())
}

pub async fn status() -> Result<()> {
  let path = config::pid_path();
  if !path.exists() {
    crate::ui::info("craft daemon: not running");
    return Ok(());
  }
  let pid_str = std::fs::read_to_string(&path).unwrap_or_default();
  let pid: u32 = pid_str.trim().parse().unwrap_or(0);
  if pid == 0 || !crate::ipc::pid_is_alive(pid) {
    crate::ui::info("craft daemon: not running (stale PID file)");
    return Ok(());
  }
  // connect via IPC and send Status request for uptime/module count
  let mut stream = match crate::ipc::connect().await {
    Ok(s) => s,
    Err(e) => {
      crate::ui::warn(format!("craft daemon: running (pid={pid}), but unreachable via IPC: {e}"));
      return Ok(());
    }
  };

  use tokio::io::{AsyncReadExt, AsyncWriteExt};
  let req = ipc_proto::IpcRequest::Status;
  let frame = ipc_proto::encode(&req)?;
  stream.write_all(&frame).await?;

  let mut len_buf = [0u8; 4];
  if stream.read_exact(&mut len_buf).await.is_err() {
    crate::ui::warn(format!("craft daemon: running (pid={pid}) - disconnected abruptly"));
    return Ok(());
  }

  let len = u32::from_le_bytes(len_buf) as usize;
  let mut json_buf = vec![0u8; len];
  stream.read_exact(&mut json_buf).await?;

  let resp: ipc_proto::IpcResponse = serde_json::from_slice(&json_buf)?;

  if let ipc_proto::IpcResponse::Status(st) = resp {
    crate::ui::success("craft daemon: running");
    crate::ui::kv("PID", st.pid);
    crate::ui::kv("Uptime", format!("{}s", st.uptime_secs));
    crate::ui::kv("Active Connections", st.active_connections);
    crate::ui::kv("Loaded Modules", st.loaded_modules);
    if !st.running_proxies.is_empty() {
      crate::ui::kv("Running Proxies", "");
      for p in st.running_proxies {
        println!("      {} (port: {})", p.plugin, p.port);
      }
    }
  } else {
    crate::ui::warn(format!("craft daemon: running (pid={pid}) - failed to get telemetry payload"));
  }

  Ok(())
}

pub async fn logs() -> Result<()> {
  let log_file = config::log_path();
  if !log_file.exists() {
    crate::ui::info(format!("craft daemon: no log file found at {}", log_file.display()));
    return Ok(());
  }

  // Print last 50 lines
  let content = std::fs::read_to_string(&log_file)?;
  let lines: Vec<&str> = content.lines().collect();
  let start = if lines.len() > 50 { lines.len() - 50 } else { 0 };
  for line in &lines[start..] {
    crate::ui::plain(line);
  }
  Ok(())
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn cleanup() {
  let _ = std::fs::remove_file(config::socket_path());
  let _ = std::fs::remove_file(config::pid_path());
  let _ = std::fs::remove_file(config::nonce_path());
  let _ = std::fs::remove_file(config::lock_path());
}

/// Acquire an exclusive flock on daemon.lock to prevent concurrent daemon starts.
fn acquire_lock() -> Result<std::fs::File> {
  let lock_path = config::lock_path();
  let file = std::fs::OpenOptions::new()
    .write(true)
    .create(true)
    .truncate(false)
    .open(&lock_path)
    .with_context(|| format!("opening {}", lock_path.display()))?;

  #[cfg(unix)]
  {
    use std::os::unix::io::AsRawFd;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
      anyhow::bail!("another daemon is already running (lock held on {})", lock_path.display());
    }
  }

  Ok(file)
}
