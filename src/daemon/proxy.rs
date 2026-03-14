//! HTTP proxy server — binds a local port and dispatches every incoming HTTP
//! request through a named WASM or JS plugin.
//!
//! **Plugin stdio protocol** (newline-terminated JSON):
//!
//! Stdin → plugin:
//!   `{"method":"GET","url":"/path","headers":{"Foo":"bar"},"body":"<base64>"}`
//!
//! Plugin → stdout:
//!   `{"status":200,"headers":{"Content-Type":"text/plain"},"body":"<base64>"}`
//!
//! The `body` field is standard base64. An empty body should be `""`.

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::oneshot;

use crate::daemon::server::DaemonState;

// ── RunningProxy handle ───────────────────────────────────────────────────────

pub struct RunningProxy {
    pub port: u16,
    shutdown_tx: oneshot::Sender<()>,
}

impl RunningProxy {
    pub fn stop(self) {
        let _ = self.shutdown_tx.send(());
    }
}

// ── Port allocation ───────────────────────────────────────────────────────────

/// Try each port in `[lo, hi]` until one binds; return it without holding the
/// socket (a brief TOCTOU window exists but is acceptable for local dev use).
pub fn find_free_port(lo: u16, hi: u16) -> Option<u16> {
    (lo..=hi).find(|&p| std::net::TcpListener::bind(("127.0.0.1", p)).is_ok())
}

// ── Proxy lifecycle ───────────────────────────────────────────────────────────

/// Bind `127.0.0.1:<port>`, spawn the accept loop, and return a handle.
pub async fn start(
    plugin_name: String,
    port: u16,
    state: Arc<DaemonState>,
) -> Result<RunningProxy> {
    let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound_port = listener.local_addr()?.port();
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    tokio::spawn(accept_loop(listener, plugin_name.clone(), state, shutdown_rx));

    tracing::info!(plugin = %plugin_name, port = bound_port, "HTTP proxy started");
    Ok(RunningProxy { port: bound_port, shutdown_tx })
}

// ── Accept loop ───────────────────────────────────────────────────────────────

async fn accept_loop(
    listener: tokio::net::TcpListener,
    plugin_name: String,
    state: Arc<DaemonState>,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                tracing::info!(plugin = %plugin_name, "proxy listener shutting down");
                break;
            }
            result = listener.accept() => {
                match result {
                    Ok((tcp, peer)) => {
                        tracing::debug!(plugin = %plugin_name, %peer, "proxy accepted connection");
                        let plugin = plugin_name.clone();
                        let state = state.clone();
                        tokio::spawn(async move {
                            if let Err(e) = serve_connection(tcp, plugin.clone(), state).await {
                                tracing::debug!(plugin = %plugin, error = %e, "proxy connection closed");
                            }
                        });
                    }
                    Err(e) => tracing::warn!(error = %e, "proxy accept error"),
                }
            }
        }
    }
}

// ── HTTP connection handler ───────────────────────────────────────────────────

async fn serve_connection(
    tcp: tokio::net::TcpStream,
    plugin_name: String,
    state: Arc<DaemonState>,
) -> Result<()> {
    use hyper::server::conn::http1;
    use hyper_util::rt::TokioIo;

    let io = TokioIo::new(tcp);
    let svc = ProxySvc { plugin_name, state };
    http1::Builder::new()
        .serve_connection(io, svc)
        .await
        .map_err(|e| anyhow::anyhow!("hyper: {e}"))
}

// ── Hyper service ─────────────────────────────────────────────────────────────

struct ProxySvc {
    plugin_name: String,
    state: Arc<DaemonState>,
}

impl hyper::service::Service<hyper::Request<hyper::body::Incoming>> for ProxySvc {
    type Response = hyper::Response<http_body_util::Full<hyper::body::Bytes>>;
    type Error = anyhow::Error;
    type Future =
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<Self::Response>> + Send>>;

    fn call(&self, req: hyper::Request<hyper::body::Incoming>) -> Self::Future {
        let plugin = self.plugin_name.clone();
        let state = self.state.clone();
        Box::pin(dispatch(req, plugin, state))
    }
}

// ── Request/response types ────────────────────────────────────────────────────

#[derive(serde::Serialize)]
struct PluginRequest {
    method: String,
    url: String,
    headers: HashMap<String, String>,
    body: String,
}

#[derive(serde::Deserialize)]
struct PluginResponse {
    status: u16,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    body: String,
}

// ── Dispatch ──────────────────────────────────────────────────────────────────

async fn dispatch(
    req: hyper::Request<hyper::body::Incoming>,
    plugin_name: String,
    state: Arc<DaemonState>,
) -> Result<hyper::Response<http_body_util::Full<hyper::body::Bytes>>> {
    use base64::Engine as _;
    use http_body_util::BodyExt as _;

    let method = req.method().as_str().to_string();
    let url = req.uri().to_string();

    let mut headers = HashMap::new();
    for (k, v) in req.headers() {
        if let Ok(s) = v.to_str() {
            headers.insert(k.as_str().to_string(), s.to_string());
        }
    }

    let body_bytes = req.collect().await?.to_bytes();
    let body = base64::engine::general_purpose::STANDARD.encode(&body_bytes);

    let plugin_req = PluginRequest { method, url, headers, body };
    let mut req_line = serde_json::to_vec(&plugin_req)?;
    req_line.push(b'\n');

    // Pipe the request through a fresh plugin execution
    let (plugin_stdin, mut stdin_writer) = tokio::io::duplex(65536);
    let (mut stdout_reader, plugin_stdout) = tokio::io::duplex(65536);

    let cfg = crate::config::Config::load().unwrap_or_else(|e| {
        tracing::warn!(error = %e, "failed to load config, using defaults");
        crate::config::Config::default()
    });

    let run_fut = run_plugin(&state, &plugin_name, plugin_stdin, plugin_stdout, cfg.execution.timeout_secs);

    let write_fut = async {
        use tokio::io::AsyncWriteExt as _;
        stdin_writer.write_all(&req_line).await?;
        drop(stdin_writer); // EOF → signals end of input to plugin
        Ok::<_, anyhow::Error>(())
    };

    let read_fut = async {
        use tokio::io::AsyncReadExt as _;
        let mut buf = Vec::new();
        stdout_reader.read_to_end(&mut buf).await?;
        Ok::<_, anyhow::Error>(buf)
    };

    let (run_res, write_res, read_res) = tokio::join!(run_fut, write_fut, read_fut);

    if let Err(e) = write_res {
        tracing::warn!(plugin = %plugin_name, error = %e, "writing to plugin stdin failed");
        return Ok(error_response(500, "internal error"));
    }
    if let Err(e) = run_res {
        tracing::warn!(plugin = %plugin_name, error = %e, "plugin execution failed");
        return Ok(error_response(500, "plugin execution failed"));
    }

    let stdout = read_res?;
    let line = stdout.split(|&b| b == b'\n').find(|l| !l.is_empty());
    let Some(line) = line else {
        return Ok(error_response(502, "plugin returned no response"));
    };

    let plugin_resp: PluginResponse = match serde_json::from_slice(line) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(plugin = %plugin_name, error = %e, "plugin response parse error");
            return Ok(error_response(502, "plugin returned invalid JSON response"));
        }
    };

    let resp_body = match base64::engine::general_purpose::STANDARD.decode(&plugin_resp.body) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(plugin = %plugin_name, error = %e, "plugin returned invalid base64 body");
            return Ok(error_response(502, "plugin returned invalid response body"));
        }
    };

    let mut builder = hyper::Response::builder().status(plugin_resp.status);
    for (k, v) in &plugin_resp.headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    Ok(builder.body(http_body_util::Full::new(hyper::body::Bytes::from(resp_body)))?)
}

// ── Plugin execution ──────────────────────────────────────────────────────────

async fn run_plugin(
    state: &DaemonState,
    plugin_name: &str,
    stdin: tokio::io::DuplexStream,
    stdout: tokio::io::DuplexStream,
    timeout_secs: u64,
) -> Result<()> {
    let manifest = crate::config::PluginManifest::load(plugin_name)?;
    match manifest.kind {
        crate::config::PluginKind::Wasm => {
            crate::daemon::engine::wasm::run_plugin(
                &state.engine,
                plugin_name,
                stdin,
                stdout,
                timeout_secs,
            )
            .await
        }
        crate::config::PluginKind::Js => {
            crate::daemon::js::run_js(state, plugin_name, stdin, stdout, timeout_secs).await
        }
    }
}

// ── Error helpers ─────────────────────────────────────────────────────────────

fn error_response(
    status: u16,
    msg: &str,
) -> hyper::Response<http_body_util::Full<hyper::body::Bytes>> {
    hyper::Response::builder()
        .status(status)
        .header("content-type", "text/plain")
        .body(http_body_util::Full::new(hyper::body::Bytes::from(msg.to_string())))
        .expect("valid error response")
}
