# Security & Optimization Analysis — 2026-03-15

40 findings across 4 severity tiers.

---

## Critical

| # | File | Issue |
|---|------|-------|
| C1 | `ipc.rs:74` | **Nonce TOCTOU race** — `pid_is_alive()` check and nonce file removal are not atomic; a malicious process can win the race window to install a forged nonce |
| C2 | `daemon/server.rs:136` | **No socket ACL on Windows** — socket permissions are Unix-only (`#[cfg(unix)]`); any local user can connect to the daemon on Windows |
| C3 | `ipc.rs:55` | **Nonce file trust** — client reads nonce from disk with no binding to the PID that wrote it; race between write and read allows nonce substitution |
| C4 | `daemon/server.rs:250` | **1 MB IPC request cap too large** — with 64 connections, attacker can force 64 MB allocation spike; no progressive reject on malformed length |

---

## High

| # | File | Issue |
|---|------|-------|
| H1 | `config.rs:40` | **Path traversal in plugin name** — `plugin_dir(name)` does no sanitization; `../../../etc` as a plugin name could escape `~/.craft/plugins/` |
| H2 | `daemon/server.rs:247` | **IPC length overflow** — `u32 as usize` cast on 32-bit platforms; no guard against allocation panic |
| H3 | `ipc.rs:63` | **Partial nonce write leaves connection inconsistent** — no cleanup on partial `write_all` during handshake; daemon and client stream states diverge |
| H4 | `daemon/nonce.rs:13` | **Thread-RNG not `OsRng`** — nonce uses `rand::fill()` (thread-local RNG); explicit `OsRng` should be used for security-critical nonces |
| H5 | `daemon/engine.rs:134` | **Silent credential loss** — `unwrap_or_default()` on keychain failure; plugins silently receive empty env vars with no warning |
| H6 | `daemon/handler.rs` (JS path) | **Empty `allowed_domains` = allow all in JS** — `if domains.is_empty() { true }` in JS network check is inverted vs WASM policy (empty = deny all); inconsistent sandbox |
| H7 | `daemon/server.rs:104` | **Eviction race** — idle reaper holds `plugin_registry` lock through full iteration; concurrent `handle_run_mcp` can contend and miss eviction |
| H8 | `daemon/server.rs:354` | **No IPC stream shutdown on plugin timeout** — `try_join!` exits early but `stream_read/stream_write` never flushed/shut down; client hangs indefinitely |
| H9 | `plugin_lang/python.rs:472` | **Silent analysis finding loss** — malformed analyser JSON silently drops findings via `.unwrap_or("")`; user believes code passed when it didn't |
| H10 | `mcp/install.rs:39` | **No install-time integrity check** — hash is stored but never verified against a known-good value; silent binary replacement goes undetected |
| H11 | `daemon/network.rs:208` | **No timeout on WebSocket `connect_async`** — slow/unresponsive targets block the spawned task indefinitely; DoS via slow WebSocket connections |
| H12 | `daemon/engine.rs:140` | **Config memory limits ignored** — WASM (64 MB) and JS (32 MB) memory caps are hardcoded; `max_memory_mb` config value is never read |
| H13 | `daemon/nonce.rs:20` | **Nonce file world-readable on Windows** — `std::fs::write()` creates file with default permissions; no `0o600` equivalent |
| H14 | `daemon/server.rs:148` | **Epoch deadline not reset on hot-reload** — a reloaded plugin inherits the previous execution's deadline instead of getting a fresh timeout |

---

## Medium

| # | File | Issue |
|---|------|-------|
| M1 | `config.rs:78` | **Manifest TOML not schema-validated** — `kind`/`name`/`source_hash` accepted verbatim; crafted manifest can panic daemon |
| M2 | `config.rs:139` | **Config values unclamped** — `max_connections: 0`, `idle_timeout_secs: u64::MAX` accepted silently |
| M3 | `daemon/server.rs:241` | **Auth failures not logged with peer address** — nonce mismatch logged without IP/timestamp; brute-force attempts are invisible |
| M4 | `daemon/server.rs:156` | **Windows shutdown race** — `ctrl_c()` handler exits immediately; cleanup of lock/nonce/pid files not guaranteed |
| M5 | `daemon/server.rs:101` | **Lock held across full reaper loop** — could starve concurrent `handle_run_mcp` if registry is large |
| M6 | `daemon/proxy.rs:172` | **3× memory overhead on request body** — raw bytes → base64 → JSON for every proxy request; 10 MB body = 30 MB transient spike |
| M7 | `daemon/proxy.rs:199` | **No timeout on `read_to_end`** — plugin hanging on output write holds proxy connection slot indefinitely |
| M8 | `daemon/proxy.rs:236` | **Plugin response headers forwarded without validation** — enables header injection (`Set-Cookie`, `Location`, etc.) from malicious plugins |
| M9 | `daemon/engine.rs:161` | **Missing export hint on component error** — user gets `"missing export"` with no information about what exports were found |
| M10 | `daemon/server.rs:174` | **No per-connection request rate limit** — unlimited `Status`/`HotReload` flood possible from local client |
| M11 | `config.rs:17` | **`home_dir()` result not validated** — if it returns a relative path, all `.join()` chains could be manipulated |

---

## Low / Optimizations

| # | File | Issue |
|---|------|-------|
| L1 | `daemon/server.rs:226` | **`Config::load()` called per-connection** — parses TOML from disk on every request; load once at startup, refresh on SIGHUP |
| L2 | `daemon/server.rs:177` | **Repeated `Arc::clone` in hot path** — minor; pass by ref where lifetime allows |
| L3 | `config.rs:96` | **`list_installed()` collects all manifests eagerly** — streaming iterator would avoid full Vec allocation |
| L4 | `mcp/install.rs:40` | **`blake3::hash` reads entire binary into memory** — use streaming hasher for large plugins |
| L5 | `daemon/proxy.rs:38` | **No `lo > hi` guard in `find_free_port`** — silent `None` return instead of clear error |
| L6 | `daemon/nonce.rs:37` | **Constant-time comparison undocumented** — security-critical path should have a comment explaining the threat model |
| L7 | various stubs | **7 `ui::info("not yet implemented")` calls** — unimplemented commands exit 0; should return `Err` so shell scripts can detect failure |

---

## Prioritized Action Order

1. **C1–C4** — nonce TOCTOU, Windows socket ACL, nonce file trust, IPC DoS
2. **H1** — plugin name path traversal (one-line fix: reject names containing `/` or `..`)
3. **H6** — JS domain allowlist inversion (logic bug, easy fix)
4. **H8** — plugin timeout stream cleanup
5. **M8** — proxy response header injection
6. **H12** — wire `max_memory_mb` config through to engine limits
