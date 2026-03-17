/// JS networking injection — `fetch`, `WebSocket`, `process.env`.
///
/// These globals are injected into each fresh AsyncContext before script
/// evaluation. All HTTP is domain-checked before dispatching via reqwest.
/// WebSocket is tunnelled via tokio-tungstenite.
///
/// This module is only compiled with `--features daemon`.
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use rquickjs::function::Opt;
use rquickjs::{AsyncContext, Class, Ctx, Object, class::Trace};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

#[rquickjs::class]
#[derive(Clone, Trace)]
pub struct Fetcher {
  allowed_domains: Vec<String>,
  /// Shared reqwest client — built once per context with timeout baked in.
  /// `skip_trace` because Client is not a JS value.
  #[qjs(skip_trace)]
  client: Client,
}

unsafe impl<'js> rquickjs::JsLifetime<'js> for Fetcher {
  type Changed<'to> = Fetcher;
}

#[rquickjs::methods]
impl Fetcher {
  pub async fn fetch<'js>(
    &self,
    _ctx: Ctx<'js>,
    url: String,
    options: Opt<Object<'js>>,
  ) -> rquickjs::Result<String> {
    let domains = self.allowed_domains.clone();

    // Empty allowlist = deny all (matches WASM policy and PRD §8).
    let is_allowed = !domains.is_empty() && {
      let host = reqwest::Url::parse(&url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_lowercase()))
        .unwrap_or_default();
      domains.iter().any(|d| host == *d || host.ends_with(&format!(".{d}")))
    };

    if !is_allowed {
      let domain = reqwest::Url::parse(&url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_default();
      crate::audit::log(crate::audit::Event::NetworkBlocked {
        plugin: "js",
        url: &url,
        domain: &domain,
      });
      return Err(rquickjs::Error::Exception);
    }

    let domain = reqwest::Url::parse(&url)
      .ok()
      .and_then(|u| u.host_str().map(|h| h.to_string()))
      .unwrap_or_default();
    crate::audit::log(crate::audit::Event::NetworkAllowed { plugin: "js", domain: &domain });

    let client = &self.client;
    let mut req = client.get(&url);

    if let Some(opts) = options.0 {
      if let Ok(method) = opts.get::<_, String>("method") {
        req = match method.to_uppercase().as_str() {
          "POST" => client.post(&url),
          "PUT" => client.put(&url),
          "DELETE" => client.delete(&url),
          "PATCH" => client.patch(&url),
          _ => client.get(&url),
        };
      }
      if let Ok(headers) = opts.get::<_, HashMap<String, String>>("headers") {
        for (k, v) in headers {
          req = req.header(k, v);
        }
      }
      if let Ok(body) = opts.get::<_, String>("body") {
        req = req.body(body);
      }
    }

    let res = req.send().await.map_err(|_| rquickjs::Error::Exception)?;
    let status = res.status().as_u16();
    let ok = (200..300).contains(&status);
    let text = res.text().await.unwrap_or_default();

    let json = serde_json::json!({"status": status, "ok": ok, "text": text}).to_string();

    Ok(json)
  }
}

/// Inject all networking globals into a fresh JS context.
pub async fn inject(ctx: &AsyncContext, allowed_domains: Vec<String>, timeout_secs: u64) -> Result<()> {
  inject_fetch(ctx, allowed_domains.clone(), timeout_secs).await?;
  inject_websocket(ctx, allowed_domains.clone()).await?;
  inject_process_env(ctx).await?;
  Ok(())
}

/// Inject `fetch(url, opts?)` back-ended by the Rust `Fetcher`.
async fn inject_fetch(ctx: &AsyncContext, allowed_domains: Vec<String>, timeout_secs: u64) -> Result<()> {
  // Build the client once — rustls with webpki roots + per-plugin timeout.
  let client = Client::builder()
    .timeout(std::time::Duration::from_secs(timeout_secs))
    .build()
    .context("building reqwest client")?;

  ctx
    .with(|ctx| {
      let globals = ctx.globals();

      let fetcher = Fetcher { allowed_domains, client };
      let instance = Class::instance(ctx.clone(), fetcher)?;
      globals.set("__host_fetcher", instance)?;

      // Define standard fetch and Response polyfills
      ctx.eval::<(), _>(
        r#"
            class Response {
                constructor(status, ok, text) {
                    this.status = status;
                    this.ok = ok;
                    this._text = text;
                }
                async text() {
                    return this._text;
                }
                async json() {
                    return JSON.parse(this._text);
                }
            }
            globalThis.fetch = async (url, options) => {
                const responseJsonStr = await __host_fetcher.fetch(url, options || {});
                const resp = JSON.parse(responseJsonStr);
                return new Response(resp.status, resp.ok, resp.text);
            };
        "#,
      )?;

      Ok::<_, anyhow::Error>(())
    })
    .await
}

#[rquickjs::class]
#[derive(Clone, Trace)]
pub struct HostWebSocket {
  #[qjs(skip_trace)]
  tx: Arc<tokio::sync::mpsc::Sender<String>>,
  #[qjs(skip_trace)]
  rx: Arc<Mutex<tokio::sync::mpsc::Receiver<String>>>,
}

unsafe impl<'js> rquickjs::JsLifetime<'js> for HostWebSocket {
  type Changed<'to> = HostWebSocket;
}

#[rquickjs::methods]
impl HostWebSocket {
  pub async fn send<'js>(&self, _ctx: Ctx<'js>, data: String) -> rquickjs::Result<()> {
    let _ = self.tx.send(data).await;
    Ok(())
  }

  pub async fn recv<'js>(&self, _ctx: Ctx<'js>) -> rquickjs::Result<Option<String>> {
    let mut rx = self.rx.lock().await;
    Ok(rx.recv().await)
  }
}

#[rquickjs::class]
#[derive(Clone, Trace)]
pub struct WebSocketFactory {
  allowed_domains: Vec<String>,
}

unsafe impl<'js> rquickjs::JsLifetime<'js> for WebSocketFactory {
  type Changed<'to> = WebSocketFactory;
}

#[rquickjs::methods]
impl WebSocketFactory {
  pub async fn create<'js>(
    &self,
    ctx: Ctx<'js>,
    url: String,
  ) -> rquickjs::Result<Class<'js, HostWebSocket>> {
    let domains = self.allowed_domains.clone();

    // Empty allowlist = deny all (matches WASM policy and PRD §8).
    let is_allowed = !domains.is_empty() && {
      // Strip ws:// / wss:// and reuse the same URL host parsing as fetch.
      let http_url = url.replacen("ws://", "http://", 1).replacen("wss://", "https://", 1);
      let host = reqwest::Url::parse(&http_url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_lowercase()))
        .unwrap_or_default();
      domains.iter().any(|d| host == *d || host.ends_with(&format!(".{d}")))
    };

    if !is_allowed {
      let http_url2 = url.replacen("ws://", "http://", 1).replacen("wss://", "https://", 1);
      let domain = reqwest::Url::parse(&http_url2)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_default();
      crate::audit::log(crate::audit::Event::NetworkBlocked {
        plugin: "js",
        url: &url,
        domain: &domain,
      });
      return Err(rquickjs::Error::Exception);
    }

    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<String>(32);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel::<String>(32);

    tokio::spawn(async move {
      if let Ok((ws_stream, _)) = connect_async(&url).await {
        let (mut write, mut read) = ws_stream.split();

        let in_tx_clone = in_tx.clone();
        let read_task = tokio::spawn(async move {
          while let Some(msg) = read.next().await {
            if let Ok(Message::Text(text)) = msg {
              let _ = in_tx_clone.send(text.to_string()).await;
            }
          }
        });

        let write_task = tokio::spawn(async move {
          while let Some(text) = out_rx.recv().await {
            let _ = write.send(Message::Text(text.into())).await;
          }
        });

        let _ = tokio::join!(read_task, write_task);
      }
    });

    let ws = HostWebSocket { tx: Arc::new(out_tx), rx: Arc::new(Mutex::new(in_rx)) };

    Class::instance(ctx.clone(), ws)
  }
}

/// Inject `WebSocket` class backed by tokio-tungstenite.
async fn inject_websocket(ctx: &AsyncContext, allowed_domains: Vec<String>) -> Result<()> {
  ctx
    .with(|ctx| {
      let globals = ctx.globals();

      let factory = WebSocketFactory { allowed_domains };
      let instance = Class::instance(ctx.clone(), factory)?;
      globals.set("__host_websocket_factory", instance)?;

      ctx.eval::<(), _>(
        r#"
            globalThis.WebSocket = class WebSocket {
                constructor(url) {
                    this.url = url;
                    this.readyState = 0; // CONNECTING
                    this._initAsync(url);
                }
                
                async _initAsync(url) {
                    try {
                        this._host = await __host_websocket_factory.create(url);
                        this.readyState = 1; // OPEN
                        if (this.onopen) this.onopen();
                        this._loop();
                    } catch(e) {
                        this.readyState = 3; // CLOSED
                        if (this.onerror) this.onerror(e);
                    }
                }
                
                async _loop() {
                    while (this.readyState === 1 && this._host) {
                        try {
                            const msg = await this._host.recv();
                            if (msg === null) break;
                            if (this.onmessage) {
                                this.onmessage({ data: msg });
                            }
                        } catch(e) {
                            break;
                        }
                    }
                    this.readyState = 3; // CLOSED
                    if (this.onclose) this.onclose();
                }
                
                send(data) {
                    if (this.readyState === 1 && this._host) {
                        this._host.send(String(data));
                    }
                }
                
                close() {
                    this.readyState = 3; // CLOSED
                }
            };
        "#,
      )?;
      Ok::<_, anyhow::Error>(())
    })
    .await
}

/// Inject `process.env` as a plain object (credentials added by handler later).
async fn inject_process_env(ctx: &AsyncContext) -> Result<()> {
  ctx
    .with(|ctx| {
      let globals = ctx.globals();
      if !globals.contains_key("process")? {
        let process = Object::new(ctx.clone())?;
        let env = Object::new(ctx.clone())?;
        process.set("env", env)?;
        globals.set("process", process)?;
      }
      Ok::<_, anyhow::Error>(())
    })
    .await
}
