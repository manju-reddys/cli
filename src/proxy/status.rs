use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{ipc, ipc_proto};

pub async fn status() -> Result<()> {
    let mut stream = ipc::connect().await?;

    let req = ipc_proto::IpcRequest::Status;
    let frame = ipc_proto::encode(&req)?;
    stream.write_all(&frame).await?;

    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut json_buf = vec![0u8; len];
    stream.read_exact(&mut json_buf).await?;

    let resp: ipc_proto::IpcResponse = serde_json::from_slice(&json_buf)?;
    match resp {
        ipc_proto::IpcResponse::Status(st) => {
            if st.running_proxies.is_empty() {
                println!("no proxies running");
            } else {
                println!("{:<24} {}", "PLUGIN", "PORT");
                for p in &st.running_proxies {
                    println!("{:<24} {}", p.plugin, p.port);
                }
            }
        }
        ipc_proto::IpcResponse::Error { detail, .. } => {
            anyhow::bail!("{detail}");
        }
        _ => anyhow::bail!("unexpected response from daemon"),
    }
    Ok(())
}
