use anyhow::Result;

/// Hot path: connect to daemon and tunnel stdin↔stdout.
///
/// Per PRD §2.1 the client is a dumb async pipe:
///   stdin → IPC socket → daemon → stdout
///
/// On stdin EOF, sends FIN on the IPC socket and exits.
pub async fn run(plugin_name: &str) -> Result<()> {
  use crate::ipc;
  use tokio::io::{self};

  let mut stream = ipc::connect().await?;

  // Send the RunMcp request frame (length-prefixed JSON)
  {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let req = crate::ipc_proto::IpcRequest::RunMcp { plugin: plugin_name.to_string() };
    let frame = crate::ipc_proto::encode(&req)?;
    stream.write_all(&frame).await?;

    // Read the IpcResponse::McpReady frame (required by the protocol before piping stdio).
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut json_buf = vec![0u8; len];
    stream.read_exact(&mut json_buf).await?;
    
    let resp: crate::ipc_proto::IpcResponse = serde_json::from_slice(&json_buf)?;
    match resp {
        crate::ipc_proto::IpcResponse::McpReady => {}
        crate::ipc_proto::IpcResponse::Error { reason, detail, .. } => {
            anyhow::bail!("{}: {}", reason, detail);
        }
        _ => anyhow::bail!("unexpected daemon response: {:?}", resp),
    }
  }

  // Decouple ingress and egress to prevent pipe deadlocks (PRD §2.4).
  // Each direction runs in its own tokio task with an mpsc channel buffer.
  let (mut read_half, mut write_half) = tokio::io::split(stream);

  let stdin_to_ipc = tokio::spawn(async move {
    let mut stdin = io::stdin();
    io::copy(&mut stdin, &mut write_half).await
  });

  let ipc_to_stdout = tokio::spawn(async move {
    let mut stdout = io::stdout();
    io::copy(&mut read_half, &mut stdout).await
  });

  // 5.2 Graceful EOF handling (PRD §2.4)
  // Use select! so we exit as soon as either side finishes.
  // Specifically, if the daemon closes the IPC socket (plugin ends), we exit
  // even if stdin is still open.
  tokio::select! {
    res = stdin_to_ipc => {
        match res {
            Ok(Ok(_)) => {
                // stdin reached EOF
            }
            Ok(Err(e)) => return Err(anyhow::anyhow!("stdin to IPC failed: {e}")),
            Err(e) => return Err(anyhow::anyhow!("stdin task panicked: {e}")),
        }
    }
    res = ipc_to_stdout => {
        match res {
            Ok(Ok(_)) => {
                // IPC socket closed (daemon finished)
                return Ok(());
            }
            Ok(Err(e)) => return Err(anyhow::anyhow!("IPC to stdout failed: {e}")),
            Err(e) => return Err(anyhow::anyhow!("stdout task panicked: {e}")),
        }
    }
  }

  Ok(())
}

