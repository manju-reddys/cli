# craft CLI — MCP Host PRD v3.0

## 1. Problem Statement

The organization requires a custom Command Line Interface (CLI) to act as an orchestrator and host for Model Context Protocol (MCP) servers and API proxies. Developers will write these MCPs and lightweight API proxy servers in TypeScript, Python, Go, and Rust.

Currently to run each of these MCPs and lightweight API proxy servers, we need to install runtime for each of them and run them in a separate process. This is not efficient and consumes a lot of time to setup and run. The MCP's and API proxies can be written in any language and to use them we need to install the runtime for each of them.

`craft` is a single-user, per-machine CLI for macOS, Windows, and Linux developer workstations — not a multi-tenant or server-hosted service.

**LLM agent invocation pattern:**
```json
{
  "mcpServers": {
    "jira": { "command": "craft", "args": ["mcp", "jira-connector"], "type": "stdio" }
  }
}
```
Every `craft mcp <name>` call spawns a new OS process. The client must connect to a persistent daemon within milliseconds.

**Constraints:**
- Binary: single executable, 15–25 MB (wasmtime alone is 8–12 MB stripped; 5 MB floor is not achievable)
- Idle RAM: near-zero when no MCPs active
- Cold-start budget: <50 ms when daemon is running
- Network: sandboxed WASM/JS cannot open sockets — daemon proxies all I/O

---

## 2. Architecture: Client–Daemon Model

Single binary, two roles depending on invocation mode.

### 2.1 CLI Client

Responsibilities: connect to daemon, tunnel stdio, manage daemon lifecycle. No wasmtime/rquickjs dependency — contributes <1 MB.

**Startup sequence:**
1. Attempt `connect()` to `~/.craft/daemon.sock`
2. If refused → check `daemon.pid`; if stale/dead delete lock, spawn daemon detached
3. Poll 10× / 20 ms (200 ms budget); on timeout → write JSON-RPC error, exit 1
4. On connect → send nonce handshake (see §6.2)
5. Tunnel: forward stdin→IPC, IPC→stdout; on stdin EOF send FIN, exit

### 2.2 Daemon

Long-lived background process. Owns all execution engines, module cache, active connections.

**Startup:**
1. Acquire exclusive lock on `~/.craft/daemon.lock` — exit if held (prevents race on simultaneous first-launch)
2. Write PID to `~/.craft/daemon.pid`
3. Generate 32-byte nonce → write `~/.craft/daemon.nonce` (0600)
4. Init `wasmtime::Engine` + `Linker` (AOT-compatible, epoch interruption enabled)
5. Init `rquickjs::AsyncRuntime`
6. Scan `~/.craft/plugins/` → populate module cache
7. Bind socket (0600), begin accepting

**Idle auto-shutdown:**
- `AtomicU64 last_active` updated on every connection
- Background task checks every 60 s; if `now - last_active > idle_timeout` (default 300 s) → flush logs, remove socket, exit
- Never shuts down while a connection is active
- `craft daemon stop` → SIGTERM bypasses timer

**Crash recovery (client-side):**
- On refused connect: read `daemon.pid`, call `kill(pid, 0)`
- If dead/missing → delete `.pid` + `.lock` → re-spawn
- First call after crash pays one cold-start penalty; subsequent calls are fast

### 2.3 Ephemeral Instance Model

Only compiled read-only `Module` objects are cached. No stateful instances persist between calls.

| Artifact | Memory |
|---|---|
| `wasmtime::Engine` | ~2 MB, shared |
| `wasmtime::Module` per plugin | ~150 KB; 30 plugins ≈ 5 MB |
| `wasmtime::Store` + Instance | ~4–8 MB, freed on disconnect |
| `rquickjs::AsyncRuntime` | ~1 MB, shared |
| `rquickjs::AsyncContext` | ~500 KB, freed on disconnect |

```rust
async fn handle_client(state: Arc<DaemonState>, tool: &str, socket: IpcSocket) -> Result<()> {
    state.last_active.store(now_secs(), Relaxed);

    // verify nonce handshake before anything else
    verify_nonce(&socket, &state.nonce).await?;

    let module = state.modules.read().await.get(tool).cloned()
        .ok_or(Error::PluginNotInstalled)?;

    let creds = load_plugin_credentials(tool)?; // from OS keychain

    let mut store = Store::new(&state.engine, StoreState {
        wasi: WasiCtxBuilder::new()
            .stdin(stdin_pipe).stdout(stdout_pipe)
            .envs(&creds)   // injected here, never exposed over IPC
            .build(),
        http: WasiHttpCtx::new(),
    });
    store.limiter(|s| &mut s.limits); // 32 MB ceiling

    let instance = state.linker.instantiate_async(&mut store, &module).await?;
    instance.get_typed_func::<(),()>(&mut store, "_start")?.call_async(&mut store, ()).await?;

    // store + instance drop here — RAM freed immediately
    Ok(())
}
```

### 2.4 Concurrency / Deadlock Prevention

MCP bidirectional JSON-RPC over stdio is susceptible to pipe deadlocks (>64 KB buffers).

`build_pipes_from_socket()` must implement:
1. Separate tokio tasks for Ingress (socket→WASM stdin) and Egress (WASM stdout→socket)
2. `tokio::sync::mpsc` channels buffer output — WASM drains immediately regardless of client read speed
3. `tokio_util::codec::Framed<LinesCodec>` prevents JSON-RPC fragmentation

---

## 3. Command Taxonomy

| Command | Mode | Description |
|---|---|---|
| `craft mcp <name>` | daemon | LLM agent entry point — proxies stdio to named MCP plugin |
| `craft proxy start <name> [--port N]` | daemon | Start named API proxy plugin (HTTP mode) |
| `craft proxy stop/status` | daemon | Manage running proxies |
| `craft install <source>` | standalone | Install plugin, prompt credentials, AOT compile |
| `craft update <name>` | standalone | Re-install, regenerate `.cwasm` if wasmtime version changed |
| `craft remove <name>` | standalone | Remove plugin files, notify daemon to evict module |
| `craft list` | standalone | List plugins, type, version, cache status |
| `craft daemon start/stop/status/logs` | standalone | Lifecycle management |
| `craft credentials set <plugin> <key> <val>` | standalone | Update a single credential post-install |
| `craft config` | standalone | Read/write `~/.craft/config.toml` |

---

## 4. Plugin Installation Flow

`craft install` is the primary path for credential setup — not `craft credentials set`.

```
craft install jira-connector

  Installing jira-connector v1.2.0...
  AOT compilation... done

  This plugin requires credentials:
    JIRA_API_TOKEN   Atlassian API token
    JIRA_BASE_URL    Your org base URL (e.g. https://acme.atlassian.net)

  Enter JIRA_API_TOKEN: ****************
  Enter JIRA_BASE_URL:  https://acme.atlassian.net

  Credentials saved to keychain.
  jira-connector installed. Allowed domains: atlassian.net, api.atlassian.com
```

**Steps:**
1. Validate source file (WASM magic bytes / JS syntax check)
2. Parse plugin manifest — required fields: `name`, `version`, `type`, `env_vars`, `allowed_domains`
3. AOT compile `.wasm` → `~/.craft/cache/<name>_<wasmtime_hash>.cwasm` — **pure machine code, no secrets**
4. Write manifest to `~/.craft/plugins/manifest.toml` including source BLAKE3 hash + wasmtime version hash
5. For each `env_vars` entry → prompt with masked input (`rpassword` crate, cross-platform)
6. Save to OS keychain under key `craft.<plugin>.<VAR_NAME>` — never written to disk as plaintext
7. If daemon running → send hot-reload signal; daemon acquires write lock, loads new Module, releases lock

**Credential storage is fully independent of the compiled cache.** The `.cwasm` file contains only machine code and is safe to distribute publicly.

---

## 5. Credential Injection

Credentials are loaded and injected by the **daemon**, not the CLI client. The client is a dumb pipe with no keychain access and no knowledge of plugin `env_vars`.

**Runtime flow:**
```
daemon handle_client()
  │
  ├── manifest lookup → env_var names: [JIRA_API_TOKEN, JIRA_BASE_URL]
  ├── keychain fetch  → values (fresh per call, never cached in heap)
  └── WasiCtxBuilder::envs([("JIRA_API_TOKEN", "xoxb-..."), ...])
                               │
                         WASM _start()  reads via std::env::var()
                         JS plugins     reads via process.env.JIRA_API_TOKEN
```

Credentials are:
- Loaded fresh from keychain on every invocation
- Exist in RAM only for the lifetime of one `WasiCtx` (one call)
- Never surfaced as raw values over the IPC socket
- Never embedded in `.cwasm` or any file on disk

---

## 6. Security Model

### 6.1 IPC Socket Permissions

- Unix: socket created at `~/.craft/daemon.sock` with mode `0600`
- Linux: `SO_PASSCRED` to verify connecting process UID matches daemon owner
- Windows: named pipe with DACL scoped to current user SID

### 6.2 Nonce Handshake (unauthorized app prevention)

Socket permissions alone are insufficient — any process running as the same user can connect. The nonce handshake prevents unauthorized apps from interacting with the daemon.

**Protocol:**
```
daemon boot:
  generate random 32-byte nonce
  write ~/.craft/daemon.nonce  (0600, owner-read only)

client connect:
  read ~/.craft/daemon.nonce
  send nonce as first 32 bytes over IPC socket

daemon:
  read first 32 bytes
  constant-time compare with stored nonce
  mismatch → close connection immediately, log attempt
  match    → proceed with tool execution
```

- Nonce is regenerated on every daemon boot — a nonce captured from a previous session is invalid
- Only processes that can read `~/.craft/daemon.nonce` (i.e. the owning user's processes with filesystem access) can connect — this meaningfully raises the bar above raw socket permissions
- Overhead: <1 ms per connection

**Blast radius limiting (defense in depth):**
Regardless of nonce bypass, raw credential values are never exposed over IPC. The daemon only accepts a tool name + stdin bytes. An unauthorized connection can at most trigger plugin execution, not extract secrets.

### 6.3 AOT Cache Integrity

- `~/.craft/cache/` created with mode `0700`
- Daemon verifies directory permissions on startup; refuses to load from world-readable/writable paths
- BLAKE3 source hash stored in `manifest.toml` verified against `.cwasm` at load time
- On wasmtime version mismatch → re-compile before accepting connections; stale `.cwasm` deleted after success

### 6.4 Execution Limits

- **RAM:** `wasmtime::Store` limiter enforces 32 MB heap per instance
- **CPU:** epoch-based interruption; daemon increments epoch every 100 ms; plugins trap after 300 epochs (30 s)
- **Connections:** max 64 simultaneous (configurable); excess connections receive immediate JSON-RPC error

---

## 7. Dual-Engine Runtime

### 7.1 JavaScript — rquickjs

- Plugin publishers ship pre-compiled `.js` (no `tsc`/`esbuild` in craft)
- One shared `AsyncRuntime` per daemon (~1 MB)
- Fresh `AsyncContext` per connection; dropped on disconnect
- Concurrent JS calls each get independent context on separate tokio tasks
- Injected globals: `fetch` (→ reqwest, domain-checked), `WebSocket` (→ tokio-tungstenite), `process.env`

### 7.2 WebAssembly — wasmtime + AOT

**AOT at install time:**
1. `engine.precompile_component()` → native machine code
2. Serialized to `~/.craft/cache/<name>_<wasmtime_hash>.cwasm`
3. Manifest records source hash + wasmtime version hash

**Cache invalidation:**
- On daemon startup and `craft update`: compare binary's embedded wasmtime hash vs manifest
- Mismatch → re-compile affected plugins before accepting connections
- Stale `.cwasm` deleted after successful re-compile
- Re-compile failure → plugin marked unavailable, error logged

**Linker setup:**
```rust
fn setup_linker(engine: &Engine) -> Linker<StoreState> {
    let mut l = wasmtime::component::Linker::new(engine);
    wasmtime_wasi::preview2::command::add_to_linker(&mut l).unwrap();       // fs, stdio, env
    wasmtime_wasi_http::proxy::add_only_http_to_linker(&mut l).unwrap();    // HTTP via reqwest
    wasmtime_wasi::preview2::bindings::sockets::tcp::add_to_linker(&mut l, |s| s).unwrap(); // TCP/TLS
    l
}

impl WasiHttpView for StoreState {
    fn send_request(&mut self, req: Request<...>, ...) -> Result<...> {
        let host = req.uri().host().unwrap_or("");
        if !self.allowed_domains.iter().any(|d| host.ends_with(d.as_str())) {
            return Err(ErrorCode::HttpRequestDenied); // domain allowlist enforced here
        }
        wasmtime_wasi_http::default_send_request(self, req, ...).await
    }
}
```

---

## 8. Network Proxying

All outbound network I/O is proxied by the daemon. Plugin authors declare permitted domains in the manifest; the daemon enforces this on every request.

**Manifest declaration:**
```toml
[plugins.jira-connector]
type = "wasm"
version = "1.2.0"
allowed_domains = ["your-org.atlassian.net", "api.atlassian.com"]
env_vars = ["JIRA_API_TOKEN", "JIRA_BASE_URL"]
```

| Interface | WASM | JS |
|---|---|---|
| HTTP/HTTPS | WASI HTTP → host reqwest | injected `fetch` → reqwest |
| WebSockets | — | injected `WebSocket` → tokio-tungstenite |
| TCP/TLS (DBs) | WASI sockets → host OS socket | — |
| Domain allowlist | `WasiHttpView::send_request` override | `fetch` wrapper check |

A plugin with empty `allowed_domains` cannot make any outbound requests.

---

## 9. API Proxy Mode

Plugins can run as lightweight HTTP servers (in addition to stdio MCP mode).

```
craft proxy start <name> [--port N]
craft proxy stop <name>
craft proxy status
```

- Daemon binds on `127.0.0.1` only (never `0.0.0.0`)
- Port specified or auto-assigned from `[7400, 7500]` range, recorded in `~/.craft/proxies.toml`
- Same ephemeral execution model as MCP: fresh Store/Context per request, dropped after response
- Same domain allowlist + credential injection

---

## 10. Error Handling

Daemon/client must never silently close the pipe. Always write a well-formed JSON-RPC error to stdout before exit.

**Error schema:**
```json
{
  "jsonrpc": "2.0", "id": null,
  "error": {
    "code": -32000,
    "message": "craft: plugin execution failed — jira-connector",
    "data": { "reason": "wasm_trap", "retry": true }
  }
}
```

| Reason | retry | Notes |
|---|---|---|
| `daemon_unavailable` | true | Daemon crashed; client re-spawns on next call |
| `plugin_not_installed` | false | Human intervention required |
| `wasm_trap` | true | Cap retries at 2 — most traps are deterministic |
| `credential_missing` | false | Run `craft credentials set` or re-install plugin |
| `network_denied` | false | Domain not in allowlist — plugin config issue |
| `timeout` | true | Exceeded 30 s execution limit |
| `auth_failed` | false | Nonce mismatch — unauthorized connection attempt |

---

## 11. Binary Size

```toml
[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
```

- Client mode: feature-flag excludes wasmtime + rquickjs → <1 MB contribution
- Audit with `cargo bloat` before each release
- Use core WASM modules (not Component Model) unless plugins require it — smaller linker footprint

---

## 12. Configuration

`~/.craft/config.toml` defaults:

```toml
[daemon]
idle_timeout_secs = 300
max_connections = 64
log_level = "info"
log_file = "~/.craft/daemon.log"

[execution]
max_memory_mb = 32
timeout_secs = 30

[proxy]
bind_address = "127.0.0.1"
default_port_range = [7400, 7500]
```

---

## Appendix: Deferred / Out of Scope

| Item | Status |
|---|---|
| Audit logging | Deferred — daemon connection handler is the right enforcement point |
| Plugin code signing | Deferred — BLAKE3 source hashes provide basic integrity |
| Multi-user / system-wide daemon | Out of scope — per-user installation only |
| Remote plugin registry | Deferred |
| TypeScript transpilation in craft | Out of scope — publishers ship pre-compiled `.js` |


The WASM sandbox has zero network access by default — it can't open a socket, make an HTTP call, or do anything outside its linear memory. Every bit of outbound I/O has to be explicitly granted by the host.

The way it works is the WASM module calls a WASI interface function (e.g. wasi:http/outgoing-handler) thinking it's making an HTTP request, but that function is actually a host import — it's a Rust function inside the daemon. The WASM module never touches the network directly.

```txt
WASM module
  │
  │  calls wasi:http/outgoing-handler   ← this is just a function import
  │  (thinks it's making HTTP request)
  ▼
wasmtime Linker  ←  host import trap
  │
  │  daemon checks: is this domain in allowed_domains?
  │  yes → dispatch via reqwest (real HTTP client in Rust)
  │  no  → return HttpRequestDenied error to WASM
  ▼
actual network  (TCP/TLS handled entirely by Rust host)