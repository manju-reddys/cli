# Plugin Specification

Authoritative reference for every plugin type the craft daemon supports.
Covers the shared transport contract, per-language build rules, host-provided
interfaces, and hard limits.

---

## 1. Plugin kinds

| Kind | Engine | Entry | File |
|---|---|---|---|
| `wasm` | wasmtime 42 (WASI P2) | `wasi:cli/run@0.2.0#run` | `plugin.wasm` |
| `js`   | rquickjs (QuickJS)    | top-level async script   | `plugin.js`  |

TypeScript must be compiled to `.js` before install. Go and Rust produce `.wasm`.

---

## 2. Transport contract (all kinds)

The host communicates with every plugin over **stdio only**.

| Stream | Direction | Content |
|---|---|---|
| stdin  | host → plugin | MCP JSON-RPC 2.0 frames, newline-delimited |
| stdout | plugin → host | MCP JSON-RPC 2.0 frames, newline-delimited |
| stderr | plugin → host | Captured (64 KB buffer), logged by daemon, never forwarded |

The plugin MUST implement the MCP stdio server protocol and MUST NOT assume
any other IPC (no loopback servers, no sockets-to-self, no shared memory).

---

## 3. Host-provided interfaces

### 3.1 HTTP and HTTPS (both kinds)

All outbound HTTP/HTTPS is domain-checked before leaving the host.
Plugins never touch TLS — the host owns the certificate chain.

- Allowlist is suffix-matched: entry `api.example.com` permits
  `sub.api.example.com` but blocks `evil-api.example.com`.
- Empty `allowed_domains` → all HTTP/HTTPS denied.

**WASM** — via `wasi:http/outgoing-handler`; intercepted in `send_request`.
**JS** — via `globalThis.fetch(url, opts?)`; intercepted in `Fetcher::fetch`.

### 3.2 WebSocket (both kinds)

**WASM** — WebSocket via HTTP upgrade goes through `send_request`
(hostname check). WebSocket via raw TCP goes through `socket_addr_check`
(IP check).
**JS** — via `globalThis.WebSocket(url)`; intercepted in `WebSocketFactory`.

### 3.3 Raw TCP / UDP (WASM only)

`wasi:sockets/tcp` and `wasi:sockets/udp` are available.
Every `connect` is checked by `socket_addr_check` against the resolved IP.
Bind is always allowed. JS plugins do not have access to raw sockets.

**Limitation**: the allowlist is matched against the resolved IP, not the
hostname. Plugins that need hostname-based gating for raw sockets must use
HTTP instead.

### 3.4 DNS (WASM only)

`wasi:sockets/ip-name-lookup` is available. Resolution uses the host
system resolver.

### 3.5 Environment variables (both kinds)

Credentials are injected as WASI env vars (WASM) or `process.env` keys (JS).
Values are fetched from the OS keychain at run time; the plugin never stores
them. Keys are declared in `manifest.toml` under `env_vars`.

### 3.6 Clocks and random (WASM only)

`wasi:clocks` (wall + monotonic) and `wasi:random` are available.
JS plugins use `Date.now()` and `Math.random()` provided by QuickJS.

---

## 4. What the host does NOT provide (hard limits)

These are permanently unavailable. No workaround exists at the host layer.

| API | Applies to | Reason |
|---|---|---|
| `socketpair()` / `os.pipe()` | WASM | POSIX syscall; no WASI P2 equivalent; wasi-libc aborts before reaching host |
| Signal handlers | WASM | No signal model in WASI P2 |
| `os.fork()` / `subprocess` | WASM | Process creation not in WASI P2 |
| Thread spawning from plugin code | WASM | `wasi-threads` proposal not enabled in craft; `std::thread::spawn` / `threading.Thread` / goroutines trap. The host daemon runs each plugin Store on its own OS thread and can run multiple plugin invocations in parallel — but that concurrency is managed by the host, not the plugin |
| Filesystem read/write | WASM | No preopened directories granted |
| Inbound socket server (accept) | WASM | Bind allowed; accept not exposed |
| `require()` / `import()` | JS | rquickjs has no module loader; all code must be in one file |
| Node.js built-ins (`fs`, `path`, `crypto`, `Buffer`) | JS | QuickJS runtime, not Node |
| `setTimeout` / `setInterval` | JS | Not available in rquickjs |
| Shared state across plugin runs | both | Each invocation is an isolated store / context |
| Inter-plugin calls | both | Plugins are fully isolated |

---

## 5. Language-specific rules

---

### 5.1 JavaScript (and TypeScript → JS)

**Engine**: rquickjs (QuickJS embedded in the daemon).
**File**: single `.js` file; the entire script is the plugin.

#### Runtime model

- The script is evaluated as a top-level `async` expression.
- The event loop runs until the returned Promise resolves.
- Single-threaded — no `Worker`, no `SharedArrayBuffer`.

#### Globals injected by the host

| Global | Type | Description |
|---|---|---|
| `fetch(url, opts?)` | `async (string, object?) → Response` | HTTP/HTTPS; domain-checked |
| `WebSocket` | class | `new WebSocket(url)` → instance with `send`, `recv`, `onmessage`, `onclose` |
| `process.env` | object | Credential key/values from keychain |

`Response` has `.status`, `.ok`, `.text()`, `.json()`.

#### TypeScript build

TypeScript must be compiled to a single ES2020 JS file before install.
Use `esbuild` (recommended) or `tsc` with `outFile`:

```sh
# esbuild — bundles all imports into one file
esbuild src/index.ts --bundle --platform=neutral --target=es2020 \
  --outfile=plugin.js

# tsc — only works if the plugin has no external npm dependencies
tsc --target ES2020 --module none --outFile plugin.js src/index.ts
```

#### What works

- `async/await`, `Promise`, `for await`
- `JSON.parse` / `JSON.stringify`
- `fetch()` for HTTP/HTTPS APIs
- `WebSocket` for streaming APIs
- `process.env` for secrets
- Standard JS built-ins: `Map`, `Set`, `URL`, `TextEncoder`, `crypto.getRandomValues`

#### What does not work

- `require()` / `import()` — no module loader; bundle everything first
- `fs`, `path`, `os`, `http`, `net` — Node.js built-ins unavailable
- `setTimeout` / `setInterval` — not available; use `async/await` instead
- Any npm package that uses Node.js built-ins
- `Buffer` — use `Uint8Array` instead

#### Minimal plugin skeleton

```js
// plugin.js — MCP stdio server in JavaScript
const readline = require === undefined
  ? { /* inline reader */ }
  : undefined; // never runs — illustration only

async function main() {
  const reader = { buf: "", lines: [] };

  process.stdin = { read: () => null }; // provided by host as raw bytes

  // read a line from stdin
  async function readLine() {
    // ... buffered stdin read
  }

  // respond
  async function reply(obj) {
    process.stdout.write(JSON.stringify(obj) + "\n");
  }

  while (true) {
    const line = await readLine();
    if (line === null) break;
    const msg = JSON.parse(line);
    if (msg.method === "initialize") {
      await reply({ jsonrpc: "2.0", id: msg.id, result: {
        protocolVersion: "2024-11-05",
        capabilities: {},
        serverInfo: { name: "my-plugin", version: "0.1.0" }
      }});
    }
    // ... handle tools/list, tools/call, etc.
  }
}

main();
```

---

### 5.2 Python (componentize-py)

**Engine**: wasmtime 42 via WASI P2.
**File**: `plugin.wasm` compiled by `componentize-py`.

#### The bundler problem

componentize-py bundles only statically-reachable imports. Libraries that load
backends by string (anyio, pydantic plugins) will be absent at runtime unless
explicitly imported in the entry module.

#### Mandatory entry shim

The componentize-py command MUST point to a shim module, NOT to the
application module:

```sh
# WRONG
componentize-py -d wit/ -w wasi:cli/command componentize app -o plugin.wasm

# CORRECT
componentize-py -d wit/ -w wasi:cli/command componentize wasm_entry -o plugin.wasm
```

The shim MUST contain exactly:

```python
# wasm_entry.py

# 1. Force anyio asyncio backend into the bundle.
#    anyio loads backends by string — bundler never sees this import.
#    Without it: ModuleNotFoundError: No module named 'anyio._backends'
import anyio._backends._asyncio  # noqa: F401

# 2. Patch asyncio self-pipe.
#    asyncio calls socketpair() to wake up the event loop from threads.
#    socketpair() does not exist in WASI P2; wasi-libc aborts before host.
import asyncio.selector_events as _se

class _DummySock:
    def fileno(self): return -1
    def close(self): pass
    def write(self, b): pass
    def flush(self): pass
    def read(self, n): return b""

def _wasi_make_self_pipe(self):
    self._ssock = _DummySock()
    self._csock = _DummySock()
    self._internal_fds += 1

def _wasi_close_self_pipe(self):
    for attr in ("_ssock", "_csock"):
        s = getattr(self, attr, None)
        if s is not None:
            s.close()
            setattr(self, attr, None)
    self._internal_fds -= 1

_se.BaseSelectorEventLoop._make_self_pipe  = _wasi_make_self_pipe
_se.BaseSelectorEventLoop._close_self_pipe = _wasi_close_self_pipe
_se.BaseSelectorEventLoop._write_to_self   = lambda self: None
_se.BaseSelectorEventLoop._read_from_self  = lambda self: None

# 3. Patch anyio thread runner.
#    WASM has no OS threads — run_sync would trap.
#    Replace with inline synchronous execution.
import anyio.to_thread as _att
import anyio._backends._asyncio as _anyio_asyncio

async def _wasm_run_sync(func, *args, limiter=None, cancellable=False,
                          abandon_on_cancel=None):
    return func(*args)

_att.run_sync = _wasm_run_sync
_anyio_asyncio.run_sync_in_worker_thread = _wasm_run_sync

# 4. Re-export Run for wasi:cli/run
from app import Run  # noqa: E402
__all__ = ["Run"]
```

#### Pydantic

pydantic v2 (`pydantic-core`) uses a Rust extension compiled for the host
platform. It must be recompiled for `wasm32-wasip2` or the plugin traps on
first model validation.

Options (in order of preference):
1. Compile `pydantic-core` from source targeting `wasm32-wasip2`.
2. Use pydantic v1 (pure Python, no Rust extension).
3. Replace pydantic with `dataclasses` + manual validation for WASM builds.

#### HTTP in Python

Use `httpx` with the WASI HTTP transport (if available for componentize-py),
or use the host's `wasi:http/outgoing-handler` via a thin WIT binding.
`requests` and `aiohttp` both require OS-level socket access and will fail.

#### What works

- `asyncio`, `anyio` (with shim applied)
- `json`, `base64`, `hashlib`, `hmac`, `struct`, `re`, `datetime`
- `logging` (output goes to stderr, captured by daemon)
- FastMCP over stdio (with shim + pydantic fix)
- Any pure-Python library with no OS thread or filesystem dependency

#### What does not work

- `threading.Thread` — traps; `wasi-threads` not enabled in craft
- `multiprocessing` — traps
- `asyncio.create_subprocess_*` — traps
- `socket.socketpair()` — aborts in wasi-libc; patch must be applied first
- `open()`, `pathlib.Path` — no filesystem
- `requests`, `aiohttp`, `urllib3` — use raw sockets; not WASI P2 compatible
- `sqlite3`, `shelve`, `pickle` with file paths — no filesystem

#### Build checklist

- [ ] Entry module is the shim (`wasm_entry.py`), not `app.py`
- [ ] Shim imports `anyio._backends._asyncio` explicitly
- [ ] Shim patches `_make_self_pipe` and `_close_self_pipe` to no-ops
- [ ] Shim patches `anyio.to_thread.run_sync` to inline runner
- [ ] No `threading.Thread`, `concurrent.futures`, `asyncio.to_thread`
- [ ] No `open()`, `pathlib`, filesystem writes
- [ ] No `subprocess`, `os.fork()`, `os.execve()`
- [ ] pydantic-core built for `wasm32-wasip2` or replaced
- [ ] `wasm-tools component wit plugin.wasm | grep "wasi:cli/run"` shows export
- [ ] `manifest.toml` lists all external domains in `allowed_domains`

---

### 5.3 Rust

**Engine**: wasmtime 42 via WASI P2.
**File**: `plugin.wasm` compiled with `cargo build --target wasm32-wasip2`.

#### Target setup

```sh
rustup target add wasm32-wasip2
```

#### Cargo.toml

```toml
[package]
name = "my-plugin"

[[bin]]
name = "my-plugin"
path = "src/main.rs"

[dependencies]
wit-bindgen = "0.36"   # WIT interface bindings
serde       = { version = "1", features = ["derive"] }
serde_json  = "1"
```

#### Export the run interface

```rust
// src/main.rs
wit_bindgen::generate!({
    world: "wasi:cli/command",
    // or use a local WIT file for MCP-specific bindings
});

struct Plugin;

impl exports::wasi::cli::run::Guest for Plugin {
    fn run() -> Result<(), ()> {
        // run the MCP stdio server synchronously to completion
        run_mcp_server().map_err(|_| ())
    }
}

export!(Plugin);
```

#### Async

A single WASM instance runs on one thread — the `wasi-threads` proposal (which
would allow spawning OS threads from inside WASM) is not enabled in craft.
`tokio` with the multi-thread scheduler will not compile because it requires
`std::thread::spawn`. Use the single-threaded scheduler or a synchronous loop:

```toml
# Cargo.toml
tokio = { version = "1", features = ["rt", "io-util", "sync"] }
# Do NOT include "rt-multi-thread"
```

```rust
// single-threaded tokio runtime
fn run_mcp_server() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .build()?
        .block_on(async_main())
}
```

#### HTTP

Use `wasi-http` bindings or a crate that targets WASI P2:

```toml
# waki — a minimal HTTP client for wasm32-wasip2
waki = "0.4"
```

Avoid `reqwest` — it depends on `tokio` multi-thread and native TLS.

#### What works

- Single-threaded `tokio` (current_thread)
- `serde_json`, `serde`
- Pure-Rust crates with no OS dependencies
- `waki` or `wasi-http` bindings for HTTP
- `wit-bindgen` for WIT interface generation
- `rand` with `wasm32` feature

#### What does not work

- `std::thread::spawn` — traps; `wasi-threads` not enabled in craft
- `tokio` multi-thread runtime — traps for the same reason; use `new_current_thread()`
- `reqwest` (default) — requires native TLS + multi-thread tokio
- `std::fs` — no filesystem
- `std::process::Command` — no subprocess
- `std::net::TcpListener` / `UdpSocket` — use `wasi:sockets` bindings instead

#### Build and package

```sh
cargo build --target wasm32-wasip2 --release

# Wrap as a WASI P2 component (required — raw WASM is not a component)
wasm-tools component new \
  target/wasm32-wasip2/release/my-plugin.wasm \
  -o plugin.wasm

# Verify export
wasm-tools component wit plugin.wasm | grep "wasi:cli/run"
```

---

### 5.4 Go (TinyGo)

**Engine**: wasmtime 42 via WASI P2.
**File**: `plugin.wasm` compiled with TinyGo targeting `wasip2`.

The standard Go toolchain (`GOOS=wasip1`) targets WASI Preview 1, which is
not compatible with the daemon (WASI P2 / component model). TinyGo is required.

#### TinyGo setup

```sh
# Install TinyGo >= 0.33 (first release with wasip2 support)
brew install tinygo   # macOS
```

#### Export the run interface

TinyGo with `wasip2` target automatically satisfies `wasi:cli/run@0.2.0`:

```go
// main.go
package main

import (
    "bufio"
    "encoding/json"
    "fmt"
    "os"
)

func main() {
    scanner := bufio.NewScanner(os.Stdin)
    writer  := bufio.NewWriter(os.Stdout)

    for scanner.Scan() {
        var req map[string]any
        if err := json.Unmarshal(scanner.Bytes(), &req); err != nil {
            continue
        }
        // handle MCP methods: initialize, tools/list, tools/call …
        resp := handleRequest(req)
        out, _ := json.Marshal(resp)
        fmt.Fprintf(writer, "%s\n", out)
        writer.Flush()
    }
}
```

#### Build and package

```sh
tinygo build -target=wasip2 -o raw.wasm .

# Wrap as a WASI P2 component
wasm-tools component new raw.wasm -o plugin.wasm

# Verify
wasm-tools component wit plugin.wasm | grep "wasi:cli/run"
```

#### HTTP

TinyGo's `net/http` does not work under `wasip2`. Use `wasi:http` WIT bindings
generated by `wit-bindgen-go`, or write directly to the `wasi:http` interface:

```sh
# Generate Go bindings from WIT
wit-bindgen-go generate --world wasi:http/imports wit/
```

#### What works

- `encoding/json`, `fmt`, `strings`, `bytes`, `math`
- `bufio.Scanner` over `os.Stdin` / `os.Stdout`
- Pure-Go libraries with no OS thread or network dependency
- `wasi:http` via generated WIT bindings
- `sync.Mutex` (safe to use; no-op in practice since a single WASM instance has no concurrent goroutines)

#### What does not work

- `go` keyword (goroutines) — TinyGo compiles them but `wasi-threads` is not
  enabled in craft, so there are no real OS threads; goroutines that block
  waiting for OS events will deadlock
- `net/http` — not available under `wasip2` in TinyGo
- `os.Open`, `os.Create` — no filesystem
- `os/exec` — no subprocess
- `reflect` — partially supported in TinyGo; test carefully
- CGo — not supported with `wasip2` target

---

## 6. manifest.toml reference

```toml
name            = "my-plugin"
kind            = "wasm"          # or "js"
source          = "/path/to/plugin.wasm"
source_hash     = "<blake3 hex>"  # recomputed on `craft mcp update`
env_vars        = ["API_KEY"]     # values fetched from OS keychain at runtime
allowed_domains = ["api.example.com"]
# suffix match: sub.api.example.com ✓ — evil-api.example.com ✗
# empty list = no outbound network allowed
```

---

## 7. AOT cache

WASM plugins are AOT-compiled on first run (~12 s for a 60 MB Python component)
and cached at `~/.craft/cache/<blake3>.cwasm`. Subsequent runs load from cache
(< 100 ms). The cache is keyed by `source_hash`; changing the binary or the
daemon binary invalidates it automatically. If the cache becomes stale
(engine config change), delete `~/.craft/cache/` to force recompilation.

JS plugins are not AOT-compiled — QuickJS evaluates the source directly.

---

## 8. Runtime limits

| Limit | Value | Notes |
|---|---|---|
| Memory (WASM) | 64 MB | Hard cap via ResourceLimiter |
| CPU timeout (WASM) | 300 s | Epoch-based; configurable per plugin in future |
| Idle eviction | 5 min | Plugin state cleared; reloaded on next call |
| stderr capture | 64 KB | Truncated if exceeded; logged on exit |
| Outbound domains | per manifest | Empty = deny all |
| Concurrent runs of same plugin | 1 | Sequential; daemon serialises |

---

## 9. Protocol coverage

| Protocol | JS | WASM | Enforcement |
|---|---|---|---|
| HTTP outbound | yes | yes | `allowed_domains` hostname suffix |
| HTTPS outbound | yes | yes | same; TLS owned by host |
| WebSocket | yes | yes | hostname suffix (JS) / IP check (raw TCP, WASM) |
| Raw TCP outbound | no | yes | `allowed_domains` IP match |
| Raw UDP outbound | no | yes | same |
| DNS | no | yes | host system resolver |
| Filesystem | no | no | no preopened dirs |
| Subprocess | no | no | not in WASI P2 |
| Inbound server (accept) | no | no | bind allowed; accept not exposed |
