use crate::shop::{Protocol, Shop};
use crate::station::Station;
use anyhow::{Context, Result};
use futures_util::FutureExt;
use serde::Serialize;
use std::collections::HashSet;
use std::panic::AssertUnwindSafe;
use std::sync::{Mutex, OnceLock};
use tokio::sync::mpsc::UnboundedSender;

pub(crate) static TOOLLESS_MODELS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

pub(crate) fn is_toolless(model: &str) -> bool {
    TOOLLESS_MODELS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map(|s| s.contains(model))
        .unwrap_or(false)
}

pub(crate) fn mark_toolless(model: &str) {
    if let Ok(mut s) = TOOLLESS_MODELS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
    {
        s.insert(model.to_string());
    }
}

pub(crate) fn is_tool_unsupported_msg(msg: &str) -> bool {
    let s = msg.to_lowercase();
    s.contains("tool")
        && (s.contains("not supported")
            || s.contains("unsupported")
            || s.contains("does not support"))
}
#[derive(Debug, Clone, Serialize)]
pub struct ApiToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiMessage {
    pub role: String,
    pub content: String,
    pub images: Vec<String>,
    pub tool_calls: Vec<ApiToolCall>,
    pub tool_call_id: String,
    pub tool_result: String,
}
#[derive(Debug)]
pub enum StreamEvent {
    Delta {
        text: String,
    },
    Brain {
        text: String,
    },
    Heart {
        text: String,
    },
    ToolCall {
        name: Option<String>,
    },
    ToolResult {
        call_id: String,
        name: String,
        arguments: String,
        output: String,
    },
    ResponseId {
        id: String,
    },
    WindowUnsupported {
        shop: String,
    },
    Usage {
        input: u64,
        output: u64,
    },
    Done,
    Error {
        message: String,
    },
}
#[derive(Clone)]
pub struct Client {
    pub(crate) http: reqwest::Client,
}
impl Client {
    pub fn new() -> Result<Self> {
        let mut builder =
            reqwest::Client::builder().user_agent(concat!("wryme/", env!("CARGO_PKG_VERSION")));
        if let Some(proxy) = Self::read_proxy_settings() {
            builder = builder.proxy(proxy);
        }
        let http = builder.build().context("building http client")?;
        Ok(Self { http })
    }

    fn read_proxy_settings() -> Option<reqwest::Proxy> {
        let proxy_url = std::env::var("HTTP_PROXY")
            .or_else(|_| std::env::var("http_proxy"))
            .or_else(|_| std::env::var("HTTPS_PROXY"))
            .or_else(|_| std::env::var("https_proxy"))
            .ok()?;

        let proxy = match reqwest::Proxy::all(&proxy_url) {
            Ok(proxy) => proxy,
            Err(e) => {
                eprintln!("wryme: ignoring invalid proxy URL {proxy_url:?}: {e}");
                return None;
            }
        };
        let mut proxy = proxy;
        if let Ok(no_proxy_str) = std::env::var("NO_PROXY").or_else(|_| std::env::var("no_proxy")) {
            let no_proxy = reqwest::NoProxy::from_string(&no_proxy_str);
            proxy = proxy.no_proxy(no_proxy);
        }
        Some(proxy)
    }

    pub async fn stream_completion(
        &self,
        shop: Shop,
        station: Station,
        messages: Vec<ApiMessage>,
        previous_response_id: Option<String>,
        engine: std::sync::Arc<std::sync::Mutex<crate::book::Engine>>,
        tx: UnboundedSender<StreamEvent>,
    ) {
        let r = AssertUnwindSafe(self.dispatch(
            shop,
            station,
            messages,
            previous_response_id,
            engine,
            tx.clone(),
        ))
        .catch_unwind()
        .await;
        if r.is_err() {
            tracing::error!("turn task panicked, turn closed");
            let _ = tx.send(StreamEvent::Error {
                message: "internal: turn task panicked, turn closed".into(),
            });
            let _ = tx.send(StreamEvent::Done);
        }
    }
    async fn dispatch(
        &self,
        shop: Shop,
        station: Station,
        messages: Vec<ApiMessage>,
        previous_response_id: Option<String>,
        engine: std::sync::Arc<std::sync::Mutex<crate::book::Engine>>,
        tx: UnboundedSender<StreamEvent>,
    ) {
        let span = tracing::info_span!(
            "turn",
            model = %station.model,
            shop = %shop.name,
            protocol = ?shop.protocol,
            window = ?shop.window,
            response_id = tracing::field::Empty
        );
        let _guard = span.enter();
        let result = match shop.protocol {
            Protocol::Demo => {
                let prompt = messages
                    .iter()
                    .rev()
                    .find(|m| m.role == "user")
                    .map(|m| m.content.as_str())
                    .unwrap_or("");
                crate::demo::stream(prompt, tx.clone()).await;
                Ok(())
            }
            Protocol::ChatCompletions => {
                crate::api_chat::stream(self, &shop, &station, messages, engine, &tx).await
            }
            Protocol::Responses => {
                crate::api_responses::stream(
                    self,
                    &shop,
                    &station,
                    messages,
                    previous_response_id,
                    engine,
                    &tx,
                )
                .await
            }
        };
        if let Err(e) = result {
            let _ = tx.send(StreamEvent::Error {
                message: format!("{:#}", e),
            });
        }
        let _ = tx.send(StreamEvent::Done);
    }
}
pub(crate) struct Boundary {
    pub body_len: usize,
    pub end: usize,
}
pub(crate) fn find_event_boundary(buf: &[u8]) -> Option<Boundary> {
    for i in 0..buf.len().saturating_sub(1) {
        if buf[i] == b'\n' && buf[i + 1] == b'\n' {
            return Some(Boundary {
                body_len: i,
                end: i + 2,
            });
        }
        if i + 3 < buf.len()
            && buf[i] == b'\r'
            && buf[i + 1] == b'\n'
            && buf[i + 2] == b'\r'
            && buf[i + 3] == b'\n'
        {
            return Some(Boundary {
                body_len: i,
                end: i + 4,
            });
        }
    }
    None
}
pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut out = s.chars().take(max).collect::<String>();
        out.push('…');
        out
    }
}
pub fn image_data_url(path: &str) -> Option<(String, String)> {
    let mime = match std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => return None,
    };
    let bytes = std::fs::read(path).ok()?;
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Some((mime.to_string(), b64))
}
