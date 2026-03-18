use anyhow::{Context, Result};
use serde::Deserialize;

// ─── Top-level config ─────────────────────────────────────────────────────────

/// Parsed representation of a plugin's `craft.config.yaml`.
#[derive(Debug, Deserialize)]
pub struct CraftConfig {
  pub name: String,
  #[serde(default)]
  pub version: Option<String>,
  #[serde(default)]
  pub description: Option<String>,
  #[serde(default)]
  pub allowed_domains: Vec<String>,
  #[serde(default)]
  pub env: Vec<EnvDecl>,
}

// ─── ENV variable declaration ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct EnvDecl {
  /// Exact env var key injected into the sandbox at runtime.
  pub name: String,
  #[serde(rename = "type")]
  pub kind: EnvKind,
  #[serde(default)]
  pub description: Option<String>,
  #[serde(default)]
  pub example: Option<String>,
  /// Used by `fixed` (immutable author-set value) and optionally `preset`.
  #[serde(default)]
  pub value: Option<String>,
  /// Default for `preset` — user can override at install time.
  #[serde(default)]
  pub default: Option<String>,
  /// Auth sub-type (only for `kind: auth`).
  #[serde(default)]
  pub auth_method: Option<AuthMethod>,
  /// Step-by-step guidance shown before the masked prompt.
  #[serde(default)]
  pub instructions: Option<AuthInstructions>,
  /// OAuth provider settings (only when `auth_method: oauth`).
  #[serde(default)]
  pub oauth: Option<OauthConfig>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum EnvKind {
  Required,
  Fixed,
  Preset,
  Auth,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AuthMethod {
  Token,
  Pat,
  #[serde(rename = "apikey")]
  ApiKey,
  Basic,
  Oauth,
}

// ─── Auth instructions ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct AuthInstructions {
  #[serde(default)]
  pub summary: Option<String>,
  /// URL the user should open (e.g. to generate a token).
  #[serde(default)]
  pub url: Option<String>,
  #[serde(default)]
  pub steps: Vec<String>,
  /// Expected value format hint (e.g. "ghp_xxxxxxxxxxxx").
  #[serde(default)]
  pub format: Option<String>,
  #[serde(default)]
  pub note: Option<String>,
}

// ─── OAuth config ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct OauthConfig {
  pub provider: String,
  #[serde(default)]
  pub scopes: Vec<String>,
  #[serde(default)]
  pub client_id: Option<String>,
  #[serde(default)]
  pub auth_url: Option<String>,
  #[serde(default)]
  pub token_url: Option<String>,
  #[serde(default)]
  pub redirect_port: Option<u16>,
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Load and parse a `craft.config.yaml` file from disk.
pub fn load(path: &std::path::Path) -> Result<CraftConfig> {
  let raw =
    std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
  serde_yml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
}

/// Reject plugin names that could escape `~/.craft/plugins/` via path traversal.
pub fn validate_plugin_name(name: &str) -> Result<()> {
  if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
    anyhow::bail!(
      "invalid plugin name {:?}: must not be empty or contain '/', '\\\\', or '..'",
      name
    );
  }
  Ok(())
}
