# Host Socket Implementation Plan

## Goal

Replace the current ad-hoc domain-allowlist checks scattered across `handler.rs`
and `network.rs` with a single, unified enforcement layer that covers every
protocol a WASM or JS plugin can use — raw TCP, UDP, HTTP, HTTPS, and WebSocket —
without the daemon owning any socket implementation.

---

## Current state (what exists and why it is incomplete)

### WASM path (`handler.rs` + `engine.rs`)

**`StoreState::send_request`** (in `handler.rs`)
Intercepts `wasi:http/outgoing-handler` calls. Domain check is a substring
`ends_with` against the URI host. Covers HTTP and HTTPS only.

**`wasmtime_wasi::p2::add_to_linker_async`** (in `engine.rs`)
Links ALL standard WASI interfaces including `wasi:sockets/tcp` and
`wasi:sockets/udp` — with **no domain check**. A plugin can open a raw TCP or
UDP connection to any address right now.

### JS path (`network.rs`)

**`Fetcher::fetch`** — domain check in Rust before dispatching via reqwest.
Covers `fetch()` in JS plugins.

**`WebSocketFactory::create`** — domain check before opening a
tokio-tungstenite connection. Covers `new WebSocket(url)` in JS plugins.

Both checks convert the URL string to a host and call `.ends_with(d)`. This is
a **substring check, not a proper suffix check**. `evil-api.com` would pass the
check for an allowlist entry of `api.com` if the host were `evil-api.com` — that
would need to be `host == d || host.ends_with(&format!(".{d}"))`.

---

## What to change

### 1. `src/daemon/handler.rs`

#### Remove
- `allowed_domains: Vec<String>` field from `StoreState`.
- The entire `send_request` override body that does the domain check manually.
- The `allowed_domains` parameter from `StoreState::new`.

#### Add / Change
In `StoreState::new`, configure `WasiCtxBuilder` with two new calls:

```rust
// allow_ip_name_lookup — required for DNS resolution during connect()
builder.allow_ip_name_lookup(true);

// socket_addr_check — fires on every TCP/UDP connect and bind.
// Default is deny-all; we supply a per-plugin allowlist closure.
let domains = allowed_domains.clone();
builder.socket_addr_check(move |addr, use_| {
    let domains = domains.clone();
    Box::pin(async move {
        use wasmtime_wasi::sockets::SocketAddrUse;
        // Only check outbound connect attempts, not bind.
        match use_ {
            SocketAddrUse::TcpBind | SocketAddrUse::UdpBind => return true,
            _ => {}
        }
        // Empty allowlist = deny all outbound connections.
        if domains.is_empty() {
            return false;
        }
        // Reverse lookup: ip → hostname for suffix matching.
        // Falls back to IP-string comparison if lookup fails.
        let host = tokio::net::lookup_host(addr.to_string()).await
            .ok()
            .and_then(|mut it| it.next())
            .map(|_| addr.ip().to_string())
            .unwrap_or_else(|| addr.ip().to_string());
        domains.iter().any(|d| host == *d || host.ends_with(&format!(".{d}")))
    })
});
```

Keep `send_request` in `WasiHttpView` but replace the manual check with a
delegation to `default_send_request` after performing the same domain check.
`wasmtime-wasi-http`'s `default_send_request_handler` calls `TcpStream::connect`
directly — it intentionally bypasses `socket_addr_check` — so HTTP/HTTPS must
still be gated here:

```rust
fn send_request(
    &mut self,
    request: hyper::Request<wasmtime_wasi_http::body::HyperOutgoingBody>,
    config: wasmtime_wasi_http::types::OutgoingRequestConfig,
) -> wasmtime_wasi_http::HttpResult<wasmtime_wasi_http::types::HostFutureIncomingResponse> {
    let host = request.uri().host().unwrap_or("").to_lowercase();
    let allowed = self.allowed_domains.is_empty()
        || self.allowed_domains.iter().any(|d| host == *d || host.ends_with(&format!(".{d}")));
    if !allowed {
        return Err(wasmtime_wasi_http::HttpError::trap(
            wasmtime::Error::msg(format!("HTTP request to '{host}' denied by domain allowlist")),
        ));
    }
    Ok(wasmtime_wasi_http::types::default_send_request(request, config))
}
```

**Note:** `allowed_domains` must remain on `StoreState` for the `send_request`
HTTP check, because `socket_addr_check` does not fire on the HTTP path.

#### Summary of field changes

| Field | Before | After |
|---|---|---|
| `allowed_domains: Vec<String>` | used only in `send_request` | keep — still needed for `send_request` |
| `wasi: WasiCtx` | built without network config | built with `socket_addr_check` + `allow_ip_name_lookup` |

---

### 2. `src/daemon/engine.rs`

#### Remove
- `wasmtime_wasi::p2::add_to_linker_async` call — this links unrestricted
  `wasi:sockets`.

#### Add
Replace with the proxy-world linker which excludes raw sockets entirely,
then add back only the socket interfaces needed (tcp + udp create, network,
ip-name-lookup) so that `socket_addr_check` is the sole enforcement gate:

```rust
// Links clocks, random, stdio, env — but NOT filesystem or sockets.
wasmtime_wasi::p2::add_to_linker_proxy_interfaces_async(&mut linker)?;

// Add sockets explicitly so socket_addr_check fires on every connection.
use wasmtime_wasi::p2::bindings::sockets;
use wasmtime_wasi::sockets::WasiSockets;
sockets::tcp_create_socket::add_to_linker::<StoreState, WasiSockets>(&mut linker, |s| s.sockets())?;
sockets::udp_create_socket::add_to_linker::<StoreState, WasiSockets>(&mut linker, |s| s.sockets())?;
sockets::tcp::add_to_linker::<StoreState, WasiSockets>(&mut linker, |s| s.sockets())?;
sockets::udp::add_to_linker::<StoreState, WasiSockets>(&mut linker, |s| s.sockets())?;
sockets::network::add_to_linker::<StoreState, WasiSockets>(&mut linker, &Default::default(), |s| s.sockets())?;
sockets::instance_network::add_to_linker::<StoreState, WasiSockets>(&mut linker, |s| s.sockets())?;
sockets::ip_name_lookup::add_to_linker::<StoreState, WasiSockets>(&mut linker, |s| s.sockets())?;

// WASI HTTP — send_request in StoreState gates HTTP/HTTPS.
wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)?;
```

`StoreState` must implement `WasiSocketsView` (accessor trait):

```rust
// in handler.rs
use wasmtime_wasi::sockets::{WasiSocketsCtx, WasiSocketsCtxView, WasiSocketsView};

pub struct StoreState {
    pub wasi:         WasiCtx,
    pub wasi_sockets: WasiSocketsCtx,   // NEW — separate from WasiCtx
    pub wasi_table:   ResourceTable,
    pub http:         WasiHttpCtx,
    pub allowed_domains: Vec<String>,
    pub memory_limit: usize,
}

impl WasiSocketsView for StoreState {
    fn sockets(&mut self) -> WasiSocketsCtxView<'_> {
        WasiSocketsCtxView {
            ctx: &mut self.wasi_sockets,
            table: &mut self.wasi_table,
        }
    }
}
```

`WasiSocketsCtx` is initialized in `StoreState::new` with the allowlist closure
(see §1 above). `WasiCtxBuilder::build` produces a `WasiCtx` that does NOT
include sockets; sockets come from the separate `WasiSocketsCtx`.

---

### 3. `src/daemon/network.rs` (JS path)

#### Fix domain-check logic in two places

**`Fetcher::fetch`** — replace the current `ends_with(d)` check:

```rust
// BEFORE (substring match — wrong)
domains.iter().any(|d| host.ends_with(d.as_str()))

// AFTER (proper suffix match)
domains.iter().any(|d| host == *d || host.ends_with(&format!(".{d}")))
```

**`WebSocketFactory::create`** — same fix:

```rust
// BEFORE
domains.iter().any(|d| host == *d || host.ends_with(&format!(".{d}")))
// Already correct — no change needed here.
```

No structural changes to `network.rs`. The JS engine does not use
`wasi:sockets` so `socket_addr_check` does not apply. The reqwest-based
`Fetcher` and tungstenite-based `WebSocketFactory` remain the enforcement
points for the JS path.

---

### 4. `src/daemon/proxy.rs`

#### No changes required

`proxy.rs` does not perform any direct socket or HTTP operations itself — it
delegates to `engine::wasm::run_plugin` and `js::run_js`, both of which go
through `StoreState` (WASM) or `Fetcher`/`WebSocketFactory` (JS). The domain
check is enforced at those layers.

---

## Protocol coverage after this change

| Protocol | Enforcement point | How |
|---|---|---|
| Raw TCP (WASM) | `socket_addr_check` closure in `WasiCtxBuilder` | fires on `TcpConnect` + `UdpConnect` via `WasiSocketsCtx` |
| Raw UDP (WASM) | same | fires on `UdpBind`, `UdpConnect`, `UdpOutgoingDatagram` |
| HTTP (WASM) | `StoreState::send_request` | `wasi:http/outgoing-handler` bypasses sockets; checked here |
| HTTPS (WASM) | `StoreState::send_request` | same — TLS handled by host after domain check passes |
| WebSocket via raw TCP (WASM) | `socket_addr_check` | TCP connect intercepted before upgrade |
| WebSocket via wasi:http (WASM) | `StoreState::send_request` | HTTP upgrade request intercepted |
| HTTP/HTTPS (JS) | `Fetcher::fetch` in `network.rs` | reqwest call gated by domain check |
| WebSocket (JS) | `WebSocketFactory::create` in `network.rs` | tungstenite connect gated by domain check |
| stdio | `wasi:cli/stdin` + `wasi:cli/stdout` | not network — no check needed |

---

## What we do NOT own

- All TCP/UDP state machine logic (connect, bind, read, write, poll, shutdown)
  remains inside `wasmtime-wasi`. We supply only a closure.
- TLS for HTTPS remains inside `wasmtime-wasi-http`'s `default_send_request`.
- DNS resolution uses the system resolver via `tokio::net::lookup_host`.

---

## Files changed summary

| File | Change type | What |
|---|---|---|
| `src/daemon/handler.rs` | Modify | Add `wasi_sockets: WasiSocketsCtx` field; impl `WasiSocketsView`; wire `socket_addr_check` in `new()`; fix `send_request` domain check to proper suffix match |
| `src/daemon/engine.rs` | Modify | Replace `add_to_linker_async` with `add_to_linker_proxy_interfaces_async` + explicit socket interface registration |
| `src/daemon/network.rs` | Modify | Fix `Fetcher::fetch` domain check from substring to proper suffix match (the WebSocket check is already correct) |
| `src/daemon/proxy.rs` | None | No changes — delegates to the layers above |
| `Cargo.toml` | None | No new dependencies — `wasmtime-wasi` already exports `WasiSocketsCtx`, `WasiSocketsView`, `SocketAddrCheck` |
