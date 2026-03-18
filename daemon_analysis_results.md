# Daemon Module Gap Analysis

After completing the core execution loops for both WASM (Phase 3) and JS (Phase 4), I reviewed the daemon module's architecture against the required PRD capabilities. While the runtimes are correctly instantiated and memory-sandboxed, several critical operational layer capabilities remain implemented as stubs (`TODO`s). 

If we move to production now, plugins will successfully instantiate but silently hang (no stdio), fail to make outbound requests (no fetch), and run forever if poorly written (no JS timeout).

Here are the detailed gaps organized by impact:

### 1. Critical: IPC ↔ Plugin Stdio Bridging (MCP Protocol)
The Model Context Protocol (MCP) relies entirely on `stdin` and `stdout` for JSON-RPC communication. Currently:
- **Missing IO Loops**: In [handle_run_mcp](file:///Users/e128151/projects/cli/src/daemon/server.rs#236-319) ([server.rs](file:///Users/e128151/projects/cli/src/daemon/server.rs)), we create Tokio duplex streams for WASM and JS, but we never pipe the `interprocess::local_socket::Stream` to them. Once the daemon sends `McpReady` to the client, the IPC stream must be split into read/write halves and piped (`tokio::io::copy`) to the plugin's `stdin`/`stdout`. Without this, the client and plugin can never exchange JSON-RPC messages.
- **JS Stdio Export Missing**: `wasmtime` correctly maps the duplex streams to the WASI context. However, in [js.rs](file:///Users/e128151/projects/cli/src/daemon/js.rs), the `_stdin` and `_stdout` channels are entirely ignored. `rquickjs` currently doesn't have host functions exposed to JS to let the script read/write to these streams. We need to implement a mechanism (like `console.log` intercept or explicit `Host.read()` / `Host.write()` async functions) bound to those channels.

### 2. High: JS Network Globals (Proxy Module)
The PRD requires JS plugins to have sandboxed network access via injected globals.
- **Stubbed Functions**: [network.rs](file:///Users/e128151/projects/cli/src/daemon/network.rs) provides empty stubs for [inject_fetch](file:///Users/e128151/projects/cli/src/daemon/network.rs#20-29) and [inject_websocket](file:///Users/e128151/projects/cli/src/daemon/network.rs#30-37).
- **Missing `reqwest` logic**: [fetch()](file:///Users/e128151/projects/cli/src/daemon/network.rs#20-29) needs to be mapped to an async Rust host function that leverages `reqwest` to make outbound HTTP calls, while enforcing the `manifest.allowed_domains` restrictions.
- **Missing `tokio-tungstenite` logic**: `WebSocket` needs bindings to `tokio-tungstenite` for things like Slack/Discord real-time plugins.

### 3. Medium: Execution Safety (JS Timeouts)
- While `wasmtime` has an epoch-based timeout ticker enforcing the 30-second execution cap, the `rquickjs` context currently only bounds memory (`32 MB`).
- **Gap**: If a JS script executes an infinite loop `while(true) {}`, the Toko worker thread will stall forever. We must utilize `rt.set_interrupt_handler` in `rquickjs` to trigger a termination if execution exceeds the timeout limit.

### 4. Medium: Observability & Telemetry (`mcp status`)
- **Missing Metrics**: [DaemonState](file:///Users/e128151/projects/cli/src/daemon/server.rs#12-21) specifies `// TODO: track with AtomicUsize` for `active_connections` and `loaded_modules`, but they remain fixed at `0`.
- **Status Reporting**: The CLI's `mcp status` currently just probes the [pid](file:///Users/e128151/projects/cli/src/config.rs#16-17). It should connect to the IPC socket, send a `StatusRequest`, and parse real telemetry (uptime, connection count, memory usage) via a newly implemented handler in [server.rs](file:///Users/e128151/projects/cli/src/daemon/server.rs).

### 5. Minor: Graceful Shutdown
- `DaemonState::shutdown` (`Arc<Notify>`) triggers a compiler warning about being unread. When a SIGTERM/SIGINT is received, we need a mechanism to broadcast this signal, allowing active plugin executions to cleanly abort and release file descriptors.

---

### Recommended Next Steps
To make the plugins actually functional over MCP, we should:
1. Implement the generic **IPC ↔ Stdio bridging** loops in [server.rs](file:///Users/e128151/projects/cli/src/daemon/server.rs).
2. Implement **JS [fetch](file:///Users/e128151/projects/cli/src/daemon/network.rs#20-29) proxying** in [network.rs](file:///Users/e128151/projects/cli/src/daemon/network.rs) (completing Phase 4 / Proxy Module).
3. Implement **JS Stdio native host functions** so JS scripts can read/write the bridged streams.
