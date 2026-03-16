//! Per-connection StoreState — scoped to one plugin execution.
//!
//! wasmtime 42 API:
//!   - WasiView trait: table() + ctx() accessors
//!   - ResourceTable replaces the old Table type
//!   - ResourceLimiter trait for memory cap enforcement
//!   - WasiHttpView: table() + ctx() + send_request()
//!
//! Socket enforcement:
//!   - socket_addr_check closure in WasiCtxBuilder fires on every TCP/UDP
//!     connect/bind — covers wasi:sockets raw TCP and UDP.
//!   - send_request override gates HTTP/HTTPS because wasmtime-wasi-http's
//!     default_send_request calls TcpStream::connect directly, bypassing
//!     socket_addr_check.

#[cfg(feature = "daemon")]
use wasmtime::ResourceLimiter;
#[cfg(feature = "daemon")]
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiView, WasiCtxView};
#[cfg(feature = "daemon")]
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

#[cfg(feature = "daemon")]
pub struct StoreState {
    pub wasi:            WasiCtx,
    pub wasi_table:      ResourceTable,
    pub http:            WasiHttpCtx,
    pub allowed_domains: Vec<String>,
    pub memory_limit:    usize, // bytes
    /// In-memory capture of the WASM component's stderr.
    /// Read after run() returns to surface Python tracebacks in daemon logs.
    pub stderr_capture:  wasmtime_wasi::p2::pipe::MemoryOutputPipe,
}

#[cfg(feature = "daemon")]
impl WasiView for StoreState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.wasi_table,
        }
    }
}

#[cfg(feature = "daemon")]
impl WasiHttpView for StoreState {
    fn ctx(&mut self) -> &mut WasiHttpCtx { &mut self.http }
    fn table(&mut self) -> &mut ResourceTable { &mut self.wasi_table }

    // HTTP/HTTPS must be gated here because wasmtime-wasi-http's
    // default_send_request calls TcpStream::connect directly, bypassing
    // socket_addr_check. An empty allowlist denies all outbound HTTP.
    fn send_request(
        &mut self,
        request: hyper::Request<wasmtime_wasi_http::body::HyperOutgoingBody>,
        config: wasmtime_wasi_http::types::OutgoingRequestConfig,
    ) -> wasmtime_wasi_http::HttpResult<wasmtime_wasi_http::types::HostFutureIncomingResponse> {
        let host = request.uri().host().unwrap_or("").to_lowercase();
        let allowed = self.allowed_domains.is_empty()
            || self
                .allowed_domains
                .iter()
                .any(|d| host == *d || host.ends_with(&format!(".{d}")));
        if !allowed {
            return Err(wasmtime_wasi_http::HttpError::trap(wasmtime::Error::msg(
                format!("HTTP request to '{host}' denied by domain allowlist"),
            )));
        }
        Ok(wasmtime_wasi_http::types::default_send_request(request, config))
    }
}

#[cfg(feature = "daemon")]
impl ResourceLimiter for StoreState {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> std::result::Result<bool, wasmtime::Error> {
        Ok(desired <= self.memory_limit)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        _desired: usize,
        _maximum: Option<usize>,
    ) -> std::result::Result<bool, wasmtime::Error> {
        Ok(true)
    }
}

#[cfg(feature = "daemon")]
impl StoreState {
    pub fn new(
        allowed_domains: Vec<String>,
        env_vars: &[(String, String)],
        memory_limit_mb: u32,
        stdin: tokio::io::DuplexStream,
        stdout: tokio::io::DuplexStream,
    ) -> anyhow::Result<Self> {
        use wasmtime_wasi::cli::{AsyncStdinStream, AsyncStdoutStream};
        use wasmtime_wasi::p2::pipe::MemoryOutputPipe;
        let mut builder = WasiCtxBuilder::new();
        for (k, v) in env_vars {
            builder.env(k, v);
        }
        // Capture WASM stderr into an in-memory buffer; flushed to tracing
        // after the plugin exits so Python exceptions are visible in logs.
        let stderr_pipe = MemoryOutputPipe::new(65536);
        let stderr_capture = stderr_pipe.clone();

        builder.stdin(AsyncStdinStream::new(stdin));
        builder.stdout(AsyncStdoutStream::new(8192, stdout));
        builder.stderr(stderr_pipe);

        // Allow DNS resolution for wasi:sockets connections.
        builder.allow_ip_name_lookup(true);

        // socket_addr_check fires on every TCP/UDP connect and bind.
        // Default is deny-all; we supply a per-plugin allowlist closure.
        // Only outbound connect attempts are checked — bind is always allowed.
        let domains = allowed_domains.clone();
        builder.socket_addr_check(move |addr, use_| {
            use wasmtime_wasi::sockets::SocketAddrUse;
            let domains = domains.clone();
            Box::pin(async move {
                match use_ {
                    SocketAddrUse::TcpBind | SocketAddrUse::UdpBind => return true,
                    _ => {}
                }
                if domains.is_empty() {
                    return false;
                }
                // addr is a resolved SocketAddr (IP:port). Compare against
                // the allowlist by IP string — raw TCP plugins must specify
                // IPs or rely on prior DNS. Hostname allowlist applies to
                // HTTP/HTTPS via send_request (the WASI-HTTP path).
                let ip = addr.ip().to_string();
                domains.iter().any(|d| ip == *d || ip.ends_with(&format!(".{d}")))
            })
        });

        let wasi = builder.build();

        Ok(Self {
            wasi,
            wasi_table: ResourceTable::new(),
            http: WasiHttpCtx::new(),
            allowed_domains,
            memory_limit: memory_limit_mb as usize * 1024 * 1024,
            stderr_capture,
        })
    }
}
