//! 32-byte IPC nonce — generated at daemon boot, written to ~/.craft/daemon.nonce (0600).
//! Clients read and send it as the first 32 bytes of every connection.
//! Constant-time XOR comparison prevents timing side-channels.

use anyhow::Result;
use std::io::Write;

pub const NONCE_LEN: usize = 32;

pub fn generate_and_write() -> Result<[u8; NONCE_LEN]> {
    // rand 0.10: thread_rng() removed — use rand::rng()
    let mut nonce = [0u8; NONCE_LEN];
    rand::fill(&mut nonce);

    let path = crate::config::nonce_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true).create(true).truncate(true).mode(0o600)
            .open(&path)?;
        f.write_all(&nonce)?;
        f.flush()?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&path, &nonce)?;
    }

    Ok(nonce)
}

/// Constant-time comparison.
pub fn verify(stored: &[u8; NONCE_LEN], received: &[u8]) -> bool {
    if received.len() != NONCE_LEN { return false; }
    let mut diff = 0u8;
    for (a, b) in stored.iter().zip(received.iter()) { diff |= a ^ b; }
    diff == 0
}
