# Pending Features & Gaps

Derived from PRD v3.0 review on 2026-03-16.

---

## P0 — Blocks core install flow

| # | Item | File | Notes |
|---|------|------|-------|
| P0.1 | **Parse `craft.config.yaml` in `mcp install`** | `src/mcp/install.rs:54` | Currently skipped with a TODO; `env_vars` hardcoded to `vec![]`. Must read the config file shipped alongside the binary and drive all subsequent steps |
| P0.2 | **Credential prompting at install time** | `src/mcp/install.rs:53-56` | PRD §4 full prompt flow unimplemented. Needs per-type handling: `required` (plain prompt), `preset` (pre-filled editable), `fixed` (silent inject), `auth: token/pat/apikey` (masked + instructions), `auth: oauth` (browser flow) |
| P0.3 | **Display step-by-step auth instructions** | `src/mcp/install.rs` | `instructions.steps`, `instructions.url`, `instructions.format`, `instructions.note` from `craft.config.yaml` must be rendered in terminal before masked prompt |
| P0.4 | **Store credentials in OS keychain at install** | `src/mcp/install.rs` | `auth::keychain::set()` exists but is never called from install; `env_vars` written to manifest must come from `craft.config.yaml` declarations |
| P0.5 | **AOT compile at install time** | `src/mcp/install.rs` | PRD §4 step 3: "AOT compilation… done" shown to user. Currently deferred to first run (~12 s cold start). Should happen during `craft mcp install` so first invocation is fast |

---

## P1 — Security

| # | Item | File | Notes |
|---|------|------|-------|
| P1.1 | **Plugin name path traversal** | `src/mcp/install.rs:33-37` | Name derived from filename with no sanitization. `../../../evil.wasm` escapes `~/.craft/plugins/`. Reject any name containing `/`, `\`, or `..` before `plugin_dir()` is called |
| P1.2 | **JS empty `allowed_domains` = allow all** | `src/daemon/network.rs` | `if domains.is_empty() { true }` is inverted — empty list should deny all, matching WASM behaviour and PRD §8 spec |
| P1.3 | **Config `max_memory_mb` not enforced** | `src/daemon/engine.rs:140` | Hardcoded `64` MB regardless of `config.execution.max_memory_mb`. PRD §6.4 states this is configurable |

---

## P1 — PRD completeness gaps

| # | Item | File | Notes |
|---|------|------|-------|
| P1.4 | **`mcp install` URL sources** | `src/mcp/install.rs:23-24` | Only local paths work; `source_path.exists()` call rejects URLs. PRD §4 specifies URL fetch via `reqwest` |
| P1.5 | **`mcp list` missing version + cache status** | `src/mcp/list.rs`, `src/config.rs` | PRD §3: "type, version, cache status". `PluginManifest` has no `version` field; `.cwasm` existence check not displayed |
| P1.6 | **`craft config set` unimplemented** | `src/cli.rs:81` | Read path works; write path is a stub |

---

## P2 — Auth features

| # | Item | File | Notes |
|---|------|------|-------|
| P2.1 | **GitHub device flow** | `src/auth/github.rs` | Stub. Implement `oauth2` device_authorization_url flow; poll for token; store in keychain |
| P2.2 | **M365 PKCE flow** | `src/auth/m365.rs` | Stub. Implement PKCE authorization_code flow with `oauth2` crate; store refresh token in keychain |
| P2.3 | **`auth credentials list <plugin>`** | `src/auth/mod.rs:48` | Stub. Enumerate keychain entries for a plugin (keys only, never values) |
| P2.4 | **OAuth flow in `mcp install`** | `src/mcp/install.rs` | Depends on P2.1/P2.2. `auth: oauth` entries in `craft.config.yaml` should trigger the appropriate provider flow at install time |

---

## P3 — Quality & tests

| # | Item | File | Notes |
|---|------|------|-------|
| P3.1 | **Zero tests** | — | Unit tests needed: `nonce::verify`, `ipc_proto` encode/decode round-trip, `config::Config::load` defaults, `error::rpc_fields` mapping |
| P3.2 | **Integration test** | — | Spawn daemon → send `RunMcp` for a trivial echo WASM component → assert stdout round-trip |
| P3.3 | **Dead-code compiler warnings** | various | 10 warnings present (`unused variable: manifest`, `unused variable: state` ×2, unused functions in keychain/config, unused enum variants in error.rs) |

---

## Deferred / out of scope (from PRD appendix)

| Item | Status |
|---|---|
| GitHub commands (`repos`, `pulls`, `auth`) | Low priority — stubs exist, implement after P2 auth is done |
| Audit logging | Deferred |
| Plugin code signing | Deferred — BLAKE3 source hashes provide basic integrity |
| Multi-user / system-wide daemon | Out of scope |
| Remote plugin registry | Deferred |
| TypeScript transpilation in craft | Out of scope — publishers ship pre-compiled `.js` |
