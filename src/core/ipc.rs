use crate::{config, error::CraftError};
use anyhow::Result;
use interprocess::local_socket::tokio::Stream as LocalSocketStream;
use interprocess::local_socket::traits::tokio::Stream as _;
use interprocess::local_socket::{GenericFilePath, ToFsName};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const POLL_ATTEMPTS: u32 = 10;
const POLL_INTERVAL_MS: u64 = 20;
const NONCE_LEN: usize = 32;

/// Connect to the daemon, performing nonce handshake.
/// Spawns the daemon and retries if the socket is not yet available.
pub async fn connect() -> Result<LocalSocketStream> {
  // Fast path — daemon already running
  if let Ok(stream) = try_connect().await {
    return Ok(stream);
  }

  // Check if a stale PID exists and clean it up
  maybe_cleanup_stale_daemon();

  // Spawn daemon detached
  spawn_daemon()?;

  // Poll up to 200 ms
  for _ in 0..POLL_ATTEMPTS {
    tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
    if let Ok(stream) = try_connect().await {
      return Ok(stream);
    }
  }

  Err(
    CraftError::DaemonUnavailable(format!(
      "daemon did not respond within {}ms",
      POLL_ATTEMPTS as u64 * POLL_INTERVAL_MS
    ))
    .into(),
  )
}

async fn try_connect() -> Result<LocalSocketStream> {
  let socket = config::socket_path();
  // Use interprocess v2 traits to build the filesystem name
  let name = socket.to_fs_name::<GenericFilePath>()?;
  let mut stream = LocalSocketStream::connect(name).await?;
  handshake(&mut stream).await?;
  Ok(stream)
}

/// Send the 32-byte nonce from ~/.craft/daemon.nonce as the first bytes.
async fn handshake(stream: &mut LocalSocketStream) -> Result<()> {
  let nonce_path = config::nonce_path();
  let nonce = tokio::fs::read(&nonce_path).await.map_err(|_| {
    // nonce file missing means daemon wrote nothing yet — treat as unavailable
    std::io::Error::new(std::io::ErrorKind::NotFound, "daemon.nonce not found")
  })?;
  if nonce.len() != NONCE_LEN {
    anyhow::bail!("malformed daemon.nonce");
  }
  stream.write_all(&nonce).await?;

  // Read 1-byte ack: 0x01 = ok, 0x00 = rejected
  let mut ack = [0u8; 1];
  stream.read_exact(&mut ack).await?;
  if ack[0] != 0x01 {
    return Err(CraftError::AuthFailed.into());
  }
  Ok(())
}

fn maybe_cleanup_stale_daemon() {
  let pid_path = config::pid_path();
  if let Ok(raw) = std::fs::read_to_string(&pid_path)
    && let Ok(pid) = raw.trim().parse::<u32>()
    && !pid_is_alive(pid)
  {
    let _ = std::fs::remove_file(&pid_path);
    let _ = std::fs::remove_file(config::lock_path());
    let _ = std::fs::remove_file(config::nonce_path());
  }
}

#[cfg(unix)]
pub fn pid_is_alive(pid: u32) -> bool {
  // 0 signal checks if the process exists and we have permission to signal it
  unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
pub fn pid_is_alive(pid: u32) -> bool {
  use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
  unsafe {
    OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
      .map(|h| {
        let _ = windows::Win32::Foundation::CloseHandle(h);
        true
      })
      .unwrap_or(false)
  }
}

fn spawn_daemon() -> Result<()> {
  let exe = std::env::current_exe()?;
  #[cfg(unix)]
  {
    use std::os::unix::process::CommandExt as _;
    std::process::Command::new(&exe)
      .arg0("craft-daemon")
      .arg("daemon")
      .arg("run-internal")
      .stdin(std::process::Stdio::null())
      .stdout(std::process::Stdio::null())
      .stderr(std::process::Stdio::null())
      .spawn()?;
  }
  #[cfg(not(unix))]
  {
    std::process::Command::new(&exe)
      .arg("daemon")
      .arg("run-internal")
      .stdin(std::process::Stdio::null())
      .stdout(std::process::Stdio::null())
      .stderr(std::process::Stdio::null())
      .spawn()?;
  }
  Ok(())
}
