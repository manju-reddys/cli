//! JS engine lifecycle — rquickjs AsyncRuntime (shared) + AsyncContext (per-call).
//!
//! This module is only compiled with `--features daemon`.
//!
//! Execution model (PRD §7.1):
//! - One shared `AsyncRuntime` (thread-safe, ~1 MB) lives for the daemon lifetime.
//! - Each connection gets a fresh `AsyncContext` that is dropped on disconnect.
//! - Concurrent JS calls each get their own context on separate tokio tasks.
//! - Injected globals: `fetch`, `WebSocket`, `process.env` (via network.rs)

use anyhow::Result;
use rquickjs::{async_with, class::Trace, AsyncRuntime, AsyncContext, Class, Ctx, Value};
use std::sync::Arc;
use tokio::sync::Mutex;

#[rquickjs::class]
#[derive(Clone, Trace)]
pub struct ProcessStdin {
    #[qjs(skip_trace)]
    stream: Arc<Mutex<tokio::io::DuplexStream>>,
}

unsafe impl<'js> rquickjs::JsLifetime<'js> for ProcessStdin {
    type Changed<'to> = ProcessStdin;
}

#[rquickjs::methods]
impl ProcessStdin {
    pub async fn read<'js>(&self, _ctx: Ctx<'js>) -> rquickjs::Result<String> {
        use tokio::io::AsyncReadExt;
        let mut rx = self.stream.lock().await;
        let mut buf = vec![0; 4096];
        let n = rx.read(&mut buf).await.map_err(|_| rquickjs::Error::Exception)?;
        Ok(String::from_utf8_lossy(&buf[..n]).to_string())
    }
}

#[rquickjs::class]
#[derive(Clone, Trace)]
pub struct ProcessStdout {
    #[qjs(skip_trace)]
    stream: Arc<Mutex<tokio::io::DuplexStream>>,
}

unsafe impl<'js> rquickjs::JsLifetime<'js> for ProcessStdout {
    type Changed<'to> = ProcessStdout;
}

#[rquickjs::methods]
impl ProcessStdout {
    pub async fn write<'js>(&self, _ctx: Ctx<'js>, data: String) -> rquickjs::Result<()> {
        use tokio::io::AsyncWriteExt;
        let mut tx = self.stream.lock().await;
        tx.write_all(data.as_bytes()).await.map_err(|_| rquickjs::Error::Exception)?;
        Ok(())
    }
}

/// Initialize the shared JS runtime. Created once at daemon startup.
pub async fn build_runtime() -> Result<AsyncRuntime> {
    let rt = AsyncRuntime::new()?;
    // Set memory limit per PRD §6.4 (32 MB ceiling)
    rt.set_memory_limit(32 * 1024 * 1024).await;
    Ok(rt)
}

/// Create a fresh context for one connection. Injects networking globals.
/// The context is dropped at the end of the call — no state persists.
pub async fn fresh_context(
    rt: &AsyncRuntime,
    allowed_domains: Vec<String>,
    timeout_secs: u64,
) -> Result<AsyncContext> {
    let ctx = AsyncContext::full(rt).await?;

    // Inject networking globals (fetch, WebSocket) with per-plugin timeout.
    super::network::inject(&ctx, allowed_domains, timeout_secs).await?;

    Ok(ctx)
}

/// Run a JS plugin to completion in an ephemeral AsyncContext.
///
/// Called by daemon/server.rs dispatch when plugin.kind == PluginKind::Js.
pub async fn run_js(
    state: &super::server::DaemonState,
    plugin: &str,
    stdin: tokio::io::DuplexStream,
    stdout: tokio::io::DuplexStream,
    timeout_secs: u64,
) -> Result<()> {
    let manifest = crate::config::PluginManifest::load(plugin)?;

    // 1. Load credentials from keychain
    let creds = crate::auth::keychain::load_all(plugin, &manifest.env_vars)
        .unwrap_or_default();

    // 2. Create fresh context with injected network globals + fetch timeout.
    let ctx = fresh_context(&state.js_runtime, manifest.allowed_domains, timeout_secs).await?;

    let stdin_arc = Arc::new(Mutex::new(stdin));
    let stdout_arc = Arc::new(Mutex::new(stdout));

    // 3. Inject credentials as process.env properties
    ctx.with(|ctx| {
        let globals = ctx.globals();
        
        let process = rquickjs::Object::new(ctx.clone())?;
        let env = rquickjs::Object::new(ctx.clone())?;
        for (k, v) in creds {
            env.set(k, v)?;
        }
        process.set("env", env)?;

        let process_stdin = ProcessStdin { stream: stdin_arc };
        let process_stdout = ProcessStdout { stream: stdout_arc };
        
        process.set("stdin", Class::instance(ctx.clone(), process_stdin)?)?;
        process.set("stdout", Class::instance(ctx.clone(), process_stdout)?)?;

        globals.set("process", process)?;
        
        // Define simple console polyfill wrapping process.stdout
        ctx.eval::<(), _>(r#"
            globalThis.console = {
                log: (...args) => {
                    const msg = args.map(a => typeof a === 'object' ? JSON.stringify(a) : String(a)).join(' ');
                    process.stdout.write(msg + "\n");
                },
                error: (...args) => {
                    const msg = args.map(a => typeof a === 'object' ? JSON.stringify(a) : String(a)).join(' ');
                    process.stdout.write("[ERROR] " + msg + "\n");
                }
            };
        "#)?;
        
        Ok::<_, rquickjs::Error>(())
    }).await?;

    // 4. Read source & evaluate
    let plugin_dir = crate::config::plugin_dir(plugin);
    let js_path = plugin_dir.join("plugin.js");
    let source = std::fs::read_to_string(&js_path)?;

    tracing::info!(plugin, "evaluating JS source");

    // eval returns a Promise (the top-level async main().catch(…) expression).
    // We must drive that Promise to completion via into_future() inside
    // async_with! so the rquickjs event loop keeps spinning.
    let run_fut = async_with!(ctx => |ctx| {
        let promise = ctx.eval::<rquickjs::Promise, _>(source)
            .map_err(|e| anyhow::anyhow!("JS eval error: {:?}", e))?;
        promise
            .into_future::<Value>()
            .await
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("JS plugin error: {:?}", e))
    });

    match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), run_fut).await {
        Ok(res) => res?,
        Err(_) => return Err(anyhow::anyhow!("JS execution timed out after {}s", timeout_secs)),
    }
    Ok(())
}
