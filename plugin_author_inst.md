# Plugin Author Instructions

Complete reference for writing, configuring, and publishing a craft MCP plugin.
Covers `craft.config.yaml`, all ENV variable types, every auth method, and the
full install-time flow craft performs.

---

## 1. Overview

Every plugin distribution must include a `craft.config.yaml` file alongside the
compiled binary (`.wasm` or `.js`). This file is the single source of truth for:

- Plugin identity (name, version, description)
- Declared environment variables and credentials
- Outbound network allowlist
- Resource preferences

When a user runs `craft mcp install <path>`, craft reads `craft.config.yaml`,
walks the user through every credential prompt in order, stores secrets in the
OS keychain (never on disk in plaintext), and writes a runtime `manifest.toml`
to `~/.craft/plugins/<name>/`.

```
your-plugin/
├── craft.config.yaml   ← you write this
├── plugin.wasm         ← compiled binary  (or plugin.js)
└── README.md           ← optional
```

---

## 2. Top-level fields

```yaml
# ── Identity ──────────────────────────────────────────────────────────────────
name: jira-connector          # kebab-case; must match binary stem (jira-connector.wasm)
version: "1.2.0"              # semver string
description: |
  Connect to JIRA Cloud for issue tracking, sprint management,
  and project reporting via MCP tools.
author: Jane Doe <jane@example.com>
license: MIT
homepage: https://github.com/example/jira-connector   # optional

# ── Runtime ───────────────────────────────────────────────────────────────────
kind: wasm                    # wasm | js

# ── Outbound network (suffix-matched; empty = deny all) ───────────────────────
allowed_domains:
  - "*.atlassian.net"
  - "api.atlassian.com"

# ── Resource preferences (craft enforces up to the system max) ─────────────────
limits:
  memory_mb: 32               # wasm default: 64 MB  |  js default: 32 MB
  timeout_secs: 30            # default: 300 s

# ── Environment variables and credentials ─────────────────────────────────────
env:
  - ...                       # see Section 3
```

---

## 3. ENV variable types

There are four `type` values. Each maps to a distinct install-time and
runtime behaviour.

---

### 3.1 `required` — user must supply a value

Craft prompts the user for a value at install time. The prompt is free-text
with the description shown above it. The field is mandatory — install aborts
if left blank.

Stored in the OS keychain. Injected as an env var at plugin run time.

```yaml
env:
  - name: JIRA_BASE_URL
    type: required
    description: Your JIRA instance URL
    example: https://yourcompany.atlassian.net

  - name: JIRA_EMAIL
    type: required
    description: Email address of your Atlassian account
    example: you@company.com
```

**Rules:**
- `name` must be `UPPER_SNAKE_CASE`; alphanumeric and underscores only.
- `description` is shown as the prompt label — be specific and concise.
- `example` is shown as greyed placeholder text inside the prompt.

---

### 3.2 `fixed` — baked in by the plugin author, immutable

Value is set by the plugin author in `craft.config.yaml`. The user cannot see
or change it. Not stored in the keychain — injected directly as a constant at
run time.

Use this for internal API versions, feature flags, or any value that must be
consistent across all installs.

```yaml
env:
  - name: JIRA_API_VERSION
    type: fixed
    value: "3"
    description: JIRA REST API version — managed by this plugin, do not change.

  - name: PLUGIN_MODE
    type: fixed
    value: production
    description: Runtime mode flag.
```

**Rules:**
- `value` is always a string (quote numbers: `"3"` not `3`).
- Never put credentials in `fixed` — anyone who reads `craft.config.yaml` can
  see the value.

---

### 3.3 `preset` — plugin-authored default, user can override at install time only

Craft shows the default value pre-filled in the prompt and lets the user edit
it before confirming. Once installed the value is frozen — it cannot be changed
without reinstalling.

Stored in the OS keychain like `required`.

```yaml
env:
  - name: JIRA_MAX_RESULTS
    type: preset
    default: "50"
    description: Maximum number of results per paginated API call (1–100)
    example: "100"

  - name: JIRA_PROJECT_KEY
    type: preset
    default: ""
    description: Default project key to scope searches (leave blank for all projects)
    example: ENG
```

**When to use `preset` vs `required`:**
- Use `preset` when you have a sensible default that covers most users but
  power users may legitimately need to change.
- Use `required` when there is no meaningful default (URLs, usernames, IDs).

---

### 3.4 `auth` — credential stored securely in OS keychain

Used for all authentication secrets: API tokens, OAuth tokens, passwords, PATs.

Craft masks input (no echo), stores the value in the OS keychain, and injects
it at runtime. The plugin binary never sees the value during build — only during
execution.

`auth` entries require an `auth_method` field:

| `auth_method` | Description | Craft behaviour |
|---|---|---|
| `token` | API token generated manually by the user | Display step-by-step instructions, then prompt |
| `pat` | Personal Access Token (GitHub, GitLab, Azure DevOps) | Same as `token` |
| `apikey` | Service API key (Stripe, Twilio, SendGrid, etc.) | Same as `token` |
| `basic` | Username + password or app password | Masked prompt, no instructions needed |
| `oauth` | OAuth 2.0 — craft runs the authorization flow automatically | No prompt; craft handles the browser dance |

---

## 4. Auth methods in detail

---

### 4.1 `token` / `pat` / `apikey` — manual token generation

These three methods are identical at the craft level: show instructions, prompt
the user, mask input, store in keychain.

The `instructions` block is the most important part. Craft renders it as a
numbered guide directly in the terminal before asking for the value. Write it
as if explaining to a non-technical user.

```yaml
env:
  - name: JIRA_API_TOKEN
    type: auth
    auth_method: token
    description: JIRA API token used to authenticate REST requests
    instructions:
      summary: Generate a JIRA API token at Atlassian's security portal
      url: https://id.atlassian.com/manage-profile/security/api-tokens
      steps:
        - "Open https://id.atlassian.com/manage-profile/security/api-tokens
           in your browser (you must be signed in to Atlassian)"
        - "Click 'Create API token'"
        - "Enter a label — for example 'craft-mcp' — and click 'Create'"
        - "Copy the token shown. It will NOT be displayed again after you close this dialog"
      format: "Alphanumeric string, approximately 24 characters"
      note: >
        This token is tied to your Atlassian account and inherits your
        permissions. Revoke it from the same security portal page if you
        believe it has been compromised.
```

**`instructions` fields:**

| Field | Required | Description |
|---|---|---|
| `summary` | yes | One-line description shown as the section header |
| `steps` | yes | Ordered list; each step is a string |
| `url` | no | Direct link to the page where the token is generated. Craft will print this as a clickable hyperlink in supported terminals |
| `format` | no | Description of the expected format, shown near the input prompt |
| `note` | no | Warning or important context shown after the steps (yellow) |

**GitHub PAT example:**

```yaml
  - name: GITHUB_TOKEN
    type: auth
    auth_method: pat
    description: GitHub Personal Access Token with repo and read:user access
    instructions:
      summary: Generate a GitHub Personal Access Token
      url: https://github.com/settings/tokens/new
      steps:
        - "Go to GitHub → Settings → Developer settings → Personal access tokens → Tokens (classic)"
        - "Click 'Generate new token (classic)'"
        - "Give it a name like 'craft-mcp'"
        - "Under 'Select scopes' check: repo, read:user"
        - "Scroll to the bottom and click 'Generate token'"
        - "Copy the token — it starts with 'ghp_' and is only shown once"
      format: "Starts with ghp_ followed by alphanumeric characters"
      note: >
        Fine-grained tokens (starting with github_pat_) are also accepted
        if you grant equivalent repository permissions.
```

**Stripe API key example:**

```yaml
  - name: STRIPE_SECRET_KEY
    type: auth
    auth_method: apikey
    description: Stripe secret key for charge and subscription management
    instructions:
      summary: Locate your Stripe secret key in the Dashboard
      url: https://dashboard.stripe.com/apikeys
      steps:
        - "Go to https://dashboard.stripe.com/apikeys"
        - "Under 'Standard keys', click 'Reveal test key' (use test mode first)"
        - "Copy the key that starts with 'sk_test_' (or 'sk_live_' for production)"
      format: "Starts with sk_test_ or sk_live_ followed by alphanumeric characters"
      note: >
        Never use a live key during development. Start with sk_test_ and
        switch to sk_live_ only when you are ready for production charges.
```

---

### 4.2 `basic` — username and password

Craft prompts for the value with a masked input field. No `instructions` block
is needed unless the service uses app-specific passwords (e.g. Gmail, Confluence
Data Center).

```yaml
env:
  - name: CONFLUENCE_USERNAME
    type: required
    description: Confluence username (usually your email address)
    example: you@company.com

  - name: CONFLUENCE_PASSWORD
    type: auth
    auth_method: basic
    description: Confluence password or app-specific token
    instructions:
      summary: Use your Confluence login password
      note: >
        If your Confluence instance uses SSO, generate an API token instead
        at https://id.atlassian.com/manage-profile/security/api-tokens
        and enter it here as the password.
```

---

### 4.3 `oauth` — craft runs the authorization flow

Craft opens the authorization URL in the system browser, starts a local
redirect listener, exchanges the code for tokens, and stores the access and
refresh tokens in the keychain. The user never copies or pastes anything.

The plugin author declares the provider and required scopes. Craft handles
token refresh automatically before each plugin invocation.

```yaml
env:
  - name: GOOGLE_TOKEN
    type: auth
    auth_method: oauth
    description: Google OAuth 2.0 token with Calendar and Drive read access
    oauth:
      provider: google
      scopes:
        - https://www.googleapis.com/auth/calendar.readonly
        - https://www.googleapis.com/auth/drive.readonly
```

**Built-in providers** (craft handles client ID / endpoints automatically):

| `provider` | Scopes format | Notes |
|---|---|---|
| `github` | Short names: `repo`, `read:user`, `gist` | Uses GitHub device flow (no browser redirect) |
| `google` | Full scope URLs | Browser redirect; PKCE |
| `microsoft` | `https://graph.microsoft.com/...` | Browser redirect; PKCE |

**Custom / self-hosted OAuth providers:**

```yaml
  - name: MY_SERVICE_TOKEN
    type: auth
    auth_method: oauth
    description: OAuth token for MyService
    oauth:
      provider: custom
      client_id: your-client-id-here
      auth_url: https://auth.myservice.com/oauth/authorize
      token_url: https://auth.myservice.com/oauth/token
      scopes:
        - read
        - write
      redirect_port: 9753       # local port for redirect (default: random)
```

**What the plugin receives at runtime:**

The OAuth access token is injected under the `name` key you declared. Craft
handles refresh silently — if the access token has expired, craft exchanges the
refresh token before invoking the plugin.

---

## 5. Reading ENV variables in plugin code

Craft injects every declared `env` entry as a standard operating-system
environment variable before invoking the plugin. **No craft SDK, no special
import, no wrapper** — use the exact same API you would use for any other env
var in your language.

The key is always the literal `name` string from `craft.config.yaml`.

---

### 5.1 Python (WASM via componentize-py)

```python
import os

# Reading a required/preset/auth value — os.environ raises KeyError if missing,
# which is the correct behaviour: a missing credential is always a bug.
base_url    = os.environ["JIRA_BASE_URL"]
email       = os.environ["JIRA_EMAIL"]
api_token   = os.environ["JIRA_API_TOKEN"]
api_version = os.environ["JIRA_API_VERSION"]   # fixed — always present

# With a safe fallback for optional preset values:
max_results = int(os.environ.get("JIRA_MAX_RESULTS", "50"))
```

**Fail loudly on missing credentials.** Do not use `.get()` with an empty-string
default for `required` or `auth` keys — a missing credential means the user
did not complete installation. Let the `KeyError` propagate so the daemon logs
it clearly.

```python
# Bad — silently uses empty token, produces confusing 401s downstream
token = os.environ.get("JIRA_API_TOKEN", "")

# Good — crashes immediately with a clear message
token = os.environ["JIRA_API_TOKEN"]
```

---

### 5.2 JavaScript / TypeScript

```js
// Reading values — process.env is injected by the craft host
const baseUrl    = process.env["JIRA_BASE_URL"];
const email      = process.env["JIRA_EMAIL"];
const apiToken   = process.env["JIRA_API_TOKEN"];
const apiVersion = process.env["JIRA_API_VERSION"];  // fixed — always present

// Fail loudly on missing credentials
function requireEnv(key) {
  const v = process.env[key];
  if (!v) throw new Error(`Missing required env var: ${key} — was the plugin installed correctly?`);
  return v;
}

const token = requireEnv("JIRA_API_TOKEN");

// Optional preset with fallback
const maxResults = parseInt(process.env["JIRA_MAX_RESULTS"] ?? "50", 10);
```

TypeScript — same code, typed:

```ts
function requireEnv(key: string): string {
  const v = process.env[key];
  if (!v) throw new Error(`Missing required env var: ${key}`);
  return v;
}

const token: string = requireEnv("JIRA_API_TOKEN");
```

---

### 5.3 Rust (WASM via wasm32-wasip2)

```rust
use std::env;

fn main() {
    // Panics if missing — correct behaviour for required/auth vars
    let base_url    = env::var("JIRA_BASE_URL").expect("JIRA_BASE_URL not set");
    let email       = env::var("JIRA_EMAIL").expect("JIRA_EMAIL not set");
    let api_token   = env::var("JIRA_API_TOKEN").expect("JIRA_API_TOKEN not set");
    let api_version = env::var("JIRA_API_VERSION").expect("JIRA_API_VERSION not set");

    // Optional preset with fallback
    let max_results: u32 = env::var("JIRA_MAX_RESULTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);
}
```

For plugins that propagate errors via `Result`:

```rust
use anyhow::{Context, Result};
use std::env;

fn load_config() -> Result<Config> {
    Ok(Config {
        base_url:  env::var("JIRA_BASE_URL").context("JIRA_BASE_URL not set")?,
        email:     env::var("JIRA_EMAIL").context("JIRA_EMAIL not set")?,
        api_token: env::var("JIRA_API_TOKEN").context("JIRA_API_TOKEN not set")?,
    })
}
```

---

### 5.4 Go (TinyGo, wasm32-wasip2)

```go
package main

import (
    "fmt"
    "os"
)

// requireEnv exits the plugin with a clear error if a key is missing.
func requireEnv(key string) string {
    v := os.Getenv(key)
    if v == "" {
        fmt.Fprintf(os.Stderr, "error: missing env var %s — was the plugin installed correctly?\n", key)
        os.Exit(1)
    }
    return v
}

func main() {
    baseURL    := requireEnv("JIRA_BASE_URL")
    email      := requireEnv("JIRA_EMAIL")
    apiToken   := requireEnv("JIRA_API_TOKEN")
    apiVersion := requireEnv("JIRA_API_VERSION")  // fixed — always present

    // Optional preset with fallback
    maxResults := os.Getenv("JIRA_MAX_RESULTS")
    if maxResults == "" {
        maxResults = "50"
    }

    _ = baseURL
    _ = email
    _ = apiToken
    _ = apiVersion
}
```

---

### 5.5 Summary table

| Language | Access pattern | Missing key behaviour |
|---|---|---|
| Python | `os.environ["KEY"]` | `KeyError` — let it propagate |
| JavaScript | `process.env["KEY"]` | `undefined` — check explicitly |
| TypeScript | `process.env["KEY"]` | `undefined` — check explicitly |
| Rust | `env::var("KEY")?` or `.expect(...)` | `VarError` — propagate or panic |
| Go | `os.Getenv("KEY")` | `""` — check explicitly |

No imports, no craft SDK, no adapter layer. The WASM sandbox receives env
vars exactly as if the process had been launched with them set in the shell.

---

## 7. Writing effective install instructions

For `token`, `pat`, and `apikey` methods, the quality of `instructions` directly
affects how successfully non-technical users can install your plugin. Follow
these guidelines:

**Be literal.** Write exactly what the user will see on the screen.
Use the exact button labels, menu paths, and field names as they appear in
the product UI.

```yaml
# Bad — vague
steps:
  - "Generate a token in the settings"

# Good — literal
steps:
  - "Click your avatar in the top-right corner and choose 'Profile Settings'"
  - "In the left sidebar, click 'Access Tokens'"
  - "In the 'Token name' field enter 'craft-mcp'"
  - "Under 'Select scopes' check 'api'"
  - "Set 'Expiration date' to at least 90 days from today"
  - "Click 'Create personal access token' and copy the value shown"
```

**Link directly.** The `url` field should go straight to the token generation
page, not the documentation homepage.

**Describe the format.** The `format` field is shown inline next to the prompt.
Users can self-validate before submitting.

```yaml
format: "40 hexadecimal characters (e.g. a1b2c3...)"
```

**Warn about one-time visibility.** Many services only show a token once. Always
note this so users know to copy it before dismissing the dialog.

```yaml
note: "This value is only shown once. If you lose it, revoke the old token and generate a new one."
```

**Separate credentials from configuration.** If the token belongs to a specific
user account, say so. Users in shared environments need to know whose account
is being authorized.

```yaml
note: >
  This token is scoped to your personal account. If your team needs
  shared access, ask your workspace admin to create a service account
  token instead.
```

---

## 8. Full worked examples

---

### 6.1 JIRA Cloud connector

```yaml
name: jira-connector
version: "1.0.0"
description: |
  Connect to JIRA Cloud for issue tracking, sprint planning,
  and project reporting. Supports creating, updating, and
  querying issues via MCP tools.
author: Acme Corp <plugins@acme.com>
license: MIT
homepage: https://github.com/acme/craft-jira

kind: wasm

allowed_domains:
  - "*.atlassian.net"
  - "api.atlassian.com"

limits:
  memory_mb: 32
  timeout_secs: 30

env:
  - name: JIRA_BASE_URL
    type: required
    description: Your JIRA Cloud instance URL (no trailing slash)
    example: https://yourcompany.atlassian.net

  - name: JIRA_EMAIL
    type: required
    description: Email address of the Atlassian account to authenticate as
    example: you@yourcompany.com

  - name: JIRA_API_TOKEN
    type: auth
    auth_method: token
    description: JIRA API token for Basic Auth (email:token)
    instructions:
      summary: Generate a JIRA API token
      url: https://id.atlassian.com/manage-profile/security/api-tokens
      steps:
        - "Sign in at https://id.atlassian.com/manage-profile/security/api-tokens"
        - "Click 'Create API token'"
        - "Enter 'craft-mcp' as the label and click 'Create'"
        - "Click 'Copy' — the token will not be shown again"
      format: "Alphanumeric string, ~24 characters"
      note: >
        The token authenticates as the account that generated it and
        inherits its JIRA permissions. Revoke from the same page if
        compromised.

  - name: JIRA_API_VERSION
    type: fixed
    value: "3"
    description: JIRA REST API version (managed by plugin)

  - name: JIRA_MAX_RESULTS
    type: preset
    default: "50"
    description: Maximum issues returned per query (1–100)
    example: "25"
```

---

### 6.2 GitHub connector (OAuth)

```yaml
name: github-connector
version: "2.1.0"
description: |
  GitHub integration — list repos, open and review pull requests,
  manage issues, and query Actions workflows.
author: Acme Corp
license: MIT

kind: js

allowed_domains:
  - api.github.com
  - uploads.github.com

limits:
  memory_mb: 16
  timeout_secs: 20

env:
  - name: GITHUB_TOKEN
    type: auth
    auth_method: oauth
    description: GitHub OAuth token
    oauth:
      provider: github
      scopes:
        - repo
        - read:user
        - read:org

  - name: GITHUB_DEFAULT_ORG
    type: preset
    default: ""
    description: Default GitHub organisation to scope queries (leave blank to search all)
    example: my-org
```

---

### 6.3 Internal service with API key and configurable base URL

```yaml
name: my-internal-api
version: "0.3.1"
description: Connect to our internal data platform API.
author: Platform Team
license: Proprietary

kind: wasm

allowed_domains:
  - "*.internal.example.com"

limits:
  memory_mb: 32
  timeout_secs: 60

env:
  - name: API_BASE_URL
    type: required
    description: Base URL of your deployment (no trailing slash)
    example: https://data.internal.example.com

  - name: API_KEY
    type: auth
    auth_method: apikey
    description: API key issued by the platform team
    instructions:
      summary: Obtain an API key from the platform team
      steps:
        - "Open a ticket at https://internal.example.com/platform/access"
        - "Request 'craft-mcp API access' and include your team name"
        - "The platform team will respond with a key within one business day"
        - "Paste the key exactly as received (no extra spaces)"
      format: "32 hexadecimal characters"

  - name: API_VERSION
    type: fixed
    value: "v2"
    description: API version this plugin was tested against

  - name: REQUEST_TIMEOUT_MS
    type: preset
    default: "10000"
    description: Per-request timeout in milliseconds
    example: "5000"
```

---

## 9. What craft does at install time

When `craft mcp install <path>` is run:

1. **Reads `craft.config.yaml`** from the same directory as the binary.
2. **Validates** `name`, `kind`, fields and ENV declarations.
3. **Copies** the binary to `~/.craft/plugins/<name>/plugin.{wasm,js}`.
4. **Computes** BLAKE3 hash of the binary.
5. **For each ENV entry**, in order:
   - `fixed` — stored inline; user never sees a prompt.
   - `preset` — shows the default value pre-filled; user can edit or press Enter to accept.
   - `required` — shows a plain text prompt; rejects empty input.
   - `auth` with `token` / `pat` / `apikey` — renders the `instructions` block, then shows a masked input prompt.
   - `auth` with `basic` — shows a masked input prompt (plus optional instructions if provided).
   - `auth` with `oauth` — opens the browser, waits for the redirect, exchanges code for tokens silently.
   - All values except `fixed` are stored in the OS keychain under the key `craft/<plugin-name>/<ENV_NAME>`.
6. **Writes** `~/.craft/plugins/<name>/manifest.toml` with `env_vars` (key names only, never values) and `allowed_domains`.
7. **Hot-reloads** the daemon if it is running.

After install, values **cannot be changed** through craft. To update a
credential, uninstall and reinstall:

```sh
craft mcp remove jira-connector
craft mcp install ./jira-connector.wasm
```

---

## 10. Security model — what the plugin sees

| Storage | Who can read | When written |
|---|---|---|
| OS keychain (macOS Keychain, Windows Credential Manager, Linux Secret Service) | Only the `craft` process running as the same OS user | At `craft mcp install` |
| `manifest.toml` | Any process with read access to `~/.craft/` | At `craft mcp install` |

**manifest.toml stores key names only.** The file contains:

```toml
env_vars = ["JIRA_BASE_URL", "JIRA_EMAIL", "JIRA_API_TOKEN"]
```

Never values. Values are fetched from the keychain by the daemon immediately
before each plugin invocation and injected as WASI env vars (WASM) or
`process.env` (JS). The plugin binary sees them as plain strings at runtime.

The plugin does not have access to the keychain itself — only to the values
craft has explicitly injected for its declared keys.

---

### 10.1 The injected key name IS the `name` field

**The exact string you write in `env[].name` in `craft.config.yaml` is the
environment variable key your plugin binary reads at runtime.** Nothing is
transformed, prefixed, or lowercased. What you declare is what you get.

```yaml
# craft.config.yaml
env:
  - name: JIRA_API_TOKEN
    type: auth
    auth_method: token
    ...
```

```python
# Inside the WASM plugin at runtime
import os
token = os.environ["JIRA_API_TOKEN"]   # ← exactly this string
```

```js
// Inside the JS plugin at runtime
const token = process.env["JIRA_API_TOKEN"];  // ← exactly this string
```

```rust
// Inside the Rust/WASM plugin at runtime
let token = std::env::var("JIRA_API_TOKEN").unwrap();  // ← exactly this string
```

```go
// Inside the TinyGo/WASM plugin at runtime
token := os.Getenv("JIRA_API_TOKEN")  // ← exactly this string
```

**Consistency rule:** keep the `name` in `craft.config.yaml` in sync with
every `os.environ["..."]` / `process.env["..."]` call in your plugin source.
If they diverge, the plugin receives an empty string and will fail silently
or panic — there is no rename layer between the config and the sandbox.

---

## 11. Pre-publish checklist

```
Identity
  [ ] name is kebab-case and matches the binary stem exactly
  [ ] version is a valid semver string
  [ ] description explains what the plugin does and what service it connects to

ENV declarations
  [ ] Every env var the plugin reads from the environment is declared
  [ ] No secrets are in `fixed` (those values are visible in craft.config.yaml)
  [ ] preset defaults are safe and sensible for a first-time user
  [ ] required entries have a clear description and example

Auth instructions (for token / pat / apikey)
  [ ] url links directly to the token generation page, not docs
  [ ] steps use exact UI labels as they appear in the product
  [ ] format describes what a valid value looks like
  [ ] note warns if the token is only shown once
  [ ] note explains whose account the token is scoped to

Network
  [ ] allowed_domains lists every external domain the plugin contacts
  [ ] No wildcard at TLD level (*.com is rejected; *.atlassian.net is fine)
  [ ] Domains that are only contacted by the host (OAuth redirects) are not needed

Runtime
  [ ] limits.memory_mb is set to the minimum your plugin actually needs
  [ ] limits.timeout_secs covers your worst-case API latency with headroom
```

---

## 10. Naming conventions

| Field | Convention | Example |
|---|---|---|
| `name` | kebab-case, service-prefixed | `jira-connector`, `github-issues`, `stripe-billing` |
| `env[].name` | `UPPER_SNAKE_CASE`, service-prefixed | `JIRA_API_TOKEN`, `GITHUB_TOKEN` |
| `allowed_domains` | Minimal wildcard — prefer exact subdomains | `api.atlassian.com` over `*.com` |
| Binary filename | Must match `name` with extension | `jira-connector.wasm` |
