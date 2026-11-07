use crate::shop::{Protocol, Shop, Tool};
use crate::station::Station;
use anyhow::{Context, Result};
use futures_util::FutureExt;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::sync::{Mutex, OnceLock};
use tokio::sync::mpsc::UnboundedSender;

pub(crate) static TOOLLESS_MODELS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
pub(crate) static NOMULTI_MODELS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

type ShopToolMap = HashMap<(String, String), Vec<Tool>>;

pub(crate) static SHOP_TOOLS: OnceLock<Mutex<ShopToolMap>> = OnceLock::new();

pub(crate) const FINAL_ANSWER_NUDGE: &str = "You have gathered enough information for this request. Do not call any more tools. Write the final answer now, using what you already have. If something remains genuinely unknown, say so in one line instead of looking it up.";

pub(crate) fn is_toolless(shop: &str, model: &str) -> bool {
    TOOLLESS_MODELS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map(|s| s.contains(&toolless_key(shop, model)))
        .unwrap_or(false)
}

pub(crate) fn mark_toolless(shop: &str, model: &str) {
    if let Ok(mut s) = TOOLLESS_MODELS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
    {
        s.insert(toolless_key(shop, model));
    }
}

fn toolless_key(shop: &str, model: &str) -> String {
    format!("{shop}\u{1f}{model}")
}

pub(crate) fn is_nomulti(shop: &str, model: &str) -> bool {
    NOMULTI_MODELS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map(|s| s.contains(&toolless_key(shop, model)))
        .unwrap_or(false)
}

pub(crate) fn mark_nomulti(shop: &str, model: &str) {
    if let Ok(mut s) = NOMULTI_MODELS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
    {
        s.insert(toolless_key(shop, model));
    }
}

pub(crate) fn record_tools(shop: &str, model: &str, tools: Vec<Tool>) {
    if let Ok(mut m) = SHOP_TOOLS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
    {
        m.insert((shop.to_string(), model.to_string()), tools);
    }
}

pub(crate) fn tools_for(shop: &str, model: &str) -> Vec<Tool> {
    SHOP_TOOLS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .ok()
        .and_then(|m| m.get(&(shop.to_string(), model.to_string())).cloned())
        .unwrap_or_default()
}

pub(crate) fn owns_tool(shop: &str, model: &str, name: &str) -> bool {
    tools_for(shop, model).iter().any(|t| t.name == name)
}

pub(crate) type DueCall = (String, String, String, String);

pub(crate) fn due_calls(tx: &UnboundedSender<StreamEvent>) -> Vec<DueCall> {
    let check = crate::tools::check_name();
    let mut out: Vec<DueCall> = crate::jobs::claim_due()
        .into_iter()
        .map(|(id, output)| (format!("check_{id}"), check.clone(), format!("{{\"id\":{id}}}"), output))
        .collect();
    if let Some(output) = crate::windows::claim_due() {
        out.push((
            format!("resv_{}", crate::windows::next_msg_id()),
            crate::tools::WRYME_MSG_RESV.to_string(),
            "{}".to_string(),
            output,
        ));
    }
    for (call_id, name, arguments, output) in &out {
        let _ = tx.send(StreamEvent::ToolResult {
            call_id: call_id.clone(),
            name: name.clone(),
            arguments: arguments.clone(),
            output: output.clone(),
        });
    }
    out
}

pub(crate) fn client_can_run(name: &str) -> bool {
    client_runs_tools() || crate::tools::is_window_tool(name)
}

pub(crate) fn client_runs_tools() -> bool {
    !cfg!(target_arch = "wasm32")
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
    Wire {
        items: Vec<serde_json::Value>,
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
    ShopWorked {
        model: String,
        shop: String,
    },
    ShopsChecked {
        shops: Vec<Shop>,
        station: Station,
        origin: Option<String>,
    },
}

#[derive(Debug, Clone, Default)]
pub struct Wire {
    pub prior: Option<Vec<serde_json::Value>>,
    pub tail_n: usize,
}

fn is_answer(ev: &StreamEvent) -> bool {
    matches!(
        ev,
        StreamEvent::Delta { .. }
            | StreamEvent::Brain { .. }
            | StreamEvent::Heart { .. }
            | StreamEvent::ToolCall { .. }
    )
}
#[derive(Clone)]
pub struct Client {
    pub(crate) http: reqwest::Client,
}
impl Client {
    pub fn new() -> Result<Self> {
        #[cfg(not(target_arch = "wasm32"))]
        let builder = {
            let mut builder =
                reqwest::Client::builder().user_agent(concat!("wryme/", env!("CARGO_PKG_VERSION")));
            if let Some(proxy) = Self::read_proxy_settings() {
                builder = builder.proxy(proxy);
            }
            for var in &["NODE_EXTRA_CA_CERTS", "SSL_CERT_FILE"] {
                if let Ok(path) = std::env::var(var) {
                    if let Ok(pem) = std::fs::read(&path) {
                        if let Ok(cert) = reqwest::Certificate::from_pem(&pem) {
                            builder = builder.add_root_certificate(cert);
                        }
                    }
                }
            }
            builder
        };
        #[cfg(target_arch = "wasm32")]
        let builder = reqwest::Client::builder();

        let http = builder.build().context("building http client")?;
        Ok(Self { http })
    }

    #[cfg(not(target_arch = "wasm32"))]
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

    #[allow(clippy::too_many_arguments)]
    pub async fn stream_completion(
        &self,
        shops: Vec<Shop>,
        station: Station,
        messages: Vec<ApiMessage>,
        previous_response_id: Option<String>,
        wire: Wire,
        engine: std::sync::Arc<std::sync::Mutex<crate::book::Engine>>,
        tx: UnboundedSender<StreamEvent>,
    ) {
        let r = AssertUnwindSafe(self.dispatch(
            shops,
            station,
            messages,
            previous_response_id,
            wire,
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
    #[allow(clippy::too_many_arguments)]
    async fn dispatch(
        &self,
        shops: Vec<Shop>,
        station: Station,
        messages: Vec<ApiMessage>,
        previous_response_id: Option<String>,
        wire: Wire,
        engine: std::sync::Arc<std::sync::Mutex<crate::book::Engine>>,
        tx: UnboundedSender<StreamEvent>,
    ) {
        let lead = shops.first().cloned().unwrap_or_else(Shop::demo);
        let span = tracing::info_span!(
            "turn",
            model = %station.model,
            shop = %lead.name,
            protocol = ?lead.protocol,
            window = ?lead.window,
            response_id = tracing::field::Empty
        );
        let _guard = span.enter();
        let due = due_calls(&tx);
        let result = if shops.len() > 1 {
            self.try_shops(
                &shops,
                &station,
                messages,
                previous_response_id,
                wire,
                engine,
                &tx,
                &due,
            )
            .await
        } else {
            self.attempt(
                &lead,
                &station,
                messages,
                previous_response_id,
                wire,
                engine,
                &tx,
                &due,
            )
            .await
        };
        if let Err(e) = result {
            let _ = tx.send(StreamEvent::Error {
                message: format!("{:#}", e),
            });
        }
        let _ = tx.send(StreamEvent::Done);
    }

    #[allow(clippy::too_many_arguments)]
    async fn try_shops(
        &self,
        shops: &[Shop],
        station: &Station,
        messages: Vec<ApiMessage>,
        previous_response_id: Option<String>,
        wire: Wire,
        engine: std::sync::Arc<std::sync::Mutex<crate::book::Engine>>,
        tx: &UnboundedSender<StreamEvent>,
        due: &[DueCall],
    ) -> Result<()> {
        let mut failures = Vec::new();
        for (i, shop) in shops.iter().enumerate() {
            let prev = if i == 0 {
                previous_response_id.clone()
            } else {
                None
            };
            let wire = if i == 0 {
                wire.clone()
            } else {
                Wire {
                    prior: None,
                    tail_n: wire.tail_n,
                }
            };
            let (itx, mut irx) = tokio::sync::mpsc::unbounded_channel();
            let run = {
                let messages = messages.clone();
                let engine = engine.clone();
                async move {
                    let r = self
                        .attempt(shop, station, messages, prev, wire, engine, &itx, due)
                        .await;
                    drop(itx);
                    r
                }
            };
            let relay = async {
                let mut held = Vec::new();
                let mut open = false;
                while let Some(ev) = irx.recv().await {
                    if !open && is_answer(&ev) {
                        open = true;
                        let _ = tx.send(StreamEvent::ShopWorked {
                            model: station.model.clone(),
                            shop: shop.name.clone(),
                        });
                        for h in held.drain(..) {
                            let _ = tx.send(h);
                        }
                    }
                    if open {
                        let _ = tx.send(ev);
                    } else {
                        held.push(ev);
                    }
                }
                (open, held)
            };
            let (result, (open, held)) = futures_util::future::join(run, relay).await;
            if open {
                return result;
            }
            let said = held.iter().rev().find_map(|ev| match ev {
                StreamEvent::Error { message } => Some(message.clone()),
                _ => None,
            });
            match (result, said) {
                (Ok(()), None) => {
                    let _ = tx.send(StreamEvent::ShopWorked {
                        model: station.model.clone(),
                        shop: shop.name.clone(),
                    });
                    for h in held {
                        let _ = tx.send(h);
                    }
                    return Ok(());
                }
                (Err(e), _) => failures.push(format!("{}: {e:#}", shop.name)),
                (Ok(()), Some(m)) => failures.push(format!("{}: {m}", shop.name)),
            }
        }
        Err(anyhow::anyhow!(
            "no shop answered for {}. {}",
            station.model,
            failures.join("; ")
        ))
    }

    #[allow(clippy::too_many_arguments)]
    async fn attempt(
        &self,
        shop: &Shop,
        station: &Station,
        messages: Vec<ApiMessage>,
        previous_response_id: Option<String>,
        wire: Wire,
        engine: std::sync::Arc<std::sync::Mutex<crate::book::Engine>>,
        tx: &UnboundedSender<StreamEvent>,
        due: &[DueCall],
    ) -> Result<()> {
        match shop.protocol {
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
                crate::api_chat::stream(self, shop, station, messages, engine, tx, due).await
            }
            Protocol::Responses => {
                crate::api_responses::stream(
                    self,
                    shop,
                    station,
                    messages,
                    previous_response_id,
                    wire,
                    engine,
                    tx,
                    due,
                )
                .await
            }
        }
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
pub(crate) fn unreachable(shop: &str, url: &str, err: &reqwest::Error) -> anyhow::Error {
    if cfg!(target_arch = "wasm32") {
        anyhow::anyhow!(
            "couldn't reach {shop} ({url}): the browser blocked the request. likely CORS, so the server must allow this page's origin (Access-Control-Allow-Origin), or it's down / the URL is wrong"
        )
    } else {
        let mut cause: &(dyn std::error::Error + 'static) = err;
        while let Some(inner) = std::error::Error::source(cause) {
            cause = inner;
        }
        anyhow::anyhow!("couldn't reach {shop} ({url}): {cause}")
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
