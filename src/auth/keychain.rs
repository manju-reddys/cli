use anyhow::{Context, Result};
use keyring::Entry;

const SERVICE: &str = "craft";

/// Build the keyring entry key: `craft.<plugin>.<var_name>`
fn entry(plugin: &str, key: &str) -> Result<Entry> {
  Entry::new(&format!("{SERVICE}.{plugin}"), key)
    .with_context(|| format!("creating keyring entry for {plugin}/{key}"))
}

pub fn set(plugin: &str, key: &str, value: &str) -> Result<()> {
  entry(plugin, key)?
    .set_password(value)
    .with_context(|| format!("storing credential {plugin}/{key}"))?;
  crate::audit::log(crate::audit::Event::CredentialSet { plugin, key });
  Ok(())
}

pub fn get(plugin: &str, key: &str) -> Result<String> {
  entry(plugin, key)?.get_password().with_context(|| format!("reading credential {plugin}/{key}"))
}

pub fn delete(plugin: &str, key: &str) -> Result<()> {
  entry(plugin, key)?
    .delete_credential()
    .with_context(|| format!("deleting credential {plugin}/{key}"))?;
  crate::audit::log(crate::audit::Event::CredentialDeleted { plugin, key });
  Ok(())
}

/// Load all credentials for a plugin as (key, value) pairs.
/// `env_var_names` comes from the plugin's manifest `env_vars` field.
pub fn load_all(plugin: &str, env_var_names: &[String]) -> Result<Vec<(String, String)>> {
  env_var_names
    .iter()
    .map(|key| {
      let val = get(plugin, key)?;
      crate::audit::log(crate::audit::Event::CredentialAccessed { plugin, key });
      Ok((key.clone(), val))
    })
    .collect()
}
