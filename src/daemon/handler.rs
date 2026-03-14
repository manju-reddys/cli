//! Per-connection StoreState — scoped to one plugin execution.
//!
//! wasmtime 42 API:
//!   - WasiView trait: table() + ctx() accessors
//!   - ResourceTable replaces the old Table type
//!   - ResourceLimiter trait for memory cap enforcement
//!   - WasiHttpView: table() + ctx() + send_request()

#[cfg(feature = "daemon")]
use wasmtime::ResourceLimiter;
#[cfg(feature = "daemon")]
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiView, WasiCtxView};
#[cfg(feature = "daemon")]
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

#[cfg(feature = "daemon")]
pub struct StoreState {
    pub wasi:         WasiCtx,
    pub wasi_table:   ResourceTable,
    pub http:         WasiHttpCtx,
    pub allowed_domains: Vec<String>,
    pub memory_limit: usize,          // bytes
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

    // send_request is required by the WasiHttpView trait in wasmtime v42.
    fn send_request(
        &mut self,
        request: hyper::Request<wasmtime_wasi_http::body::HyperOutgoingBody>,
        config: wasmtime_wasi_http::types::OutgoingRequestConfig,
    ) -> wasmtime_wasi_http::HttpResult<wasmtime_wasi_http::types::HostFutureIncomingResponse> {
        let host = request.uri().host().unwrap_or("").to_string();
        let allowed = self.allowed_domains.iter().any(|d| host.ends_with(d.as_str()));
        
        if !allowed {
            return Err(wasmtime_wasi_http::HttpError::trap(
                wasmtime::Error::msg(format!("HTTP request to {} denied by domain allowlist", host))
            ));
        }

        Ok(wasmtime_wasi_http::types::default_send_request(request, config))
    }
}

#[cfg(feature = "daemon")]
impl ResourceLimiter for StoreState {
    fn memory_growing(
        &mut self, _current: usize, desired: usize, _maximum: Option<usize>,
    ) -> std::result::Result<bool, wasmtime::Error> {
        Ok(desired <= self.memory_limit)
    }
    
    fn table_growing(
        &mut self, _current: usize, _desired: usize, _maximum: Option<usize>,
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
        let mut builder = WasiCtxBuilder::new();
        for (k, v) in env_vars {
            builder.env(k, v);
        }
        builder.stdin(AsyncStdinStream::new(stdin));
        builder.stdout(AsyncStdoutStream::new(8192, stdout));
        let wasi = builder.build();

        Ok(Self {
            wasi,
            wasi_table: ResourceTable::new(),
            http: WasiHttpCtx::new(),
            allowed_domains,
            memory_limit: memory_limit_mb as usize * 1024 * 1024,
        })
    }
}
