// Shared types and the protocol dispatcher.
//
// Client wraps just the reqwest http handle. Active shop and station live
// on App and are passed in per request so the popup can mutate them
// without going through Client.
//
// Per-protocol work lives in two sibling files:
//   api_chat.rs       /chat/completions
//   api_responses.rs  /responses
//
// StreamEvents both protocols can emit:
//   Delta { text }      content delta
//   Brain { text }      reasoning / thinking delta
//   ToolCall { name }   model is calling a tool; drives "tinkering"
//   ResponseId { id }   captured from response.created, replayed as
//                       previous_response_id next turn for session pinning
//   Done                clean end of stream
//   Error { message }   anything we couldn't classify as success

use anyhow::{Context, Result};
use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;

use crate::shop::{Protocol, Shop};
use crate::station::Station;

#[derive(Debug, Clone, Serialize)]
pub struct ApiMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug)]
pub enum StreamEvent {
    Delta { text: String },
    Brain { text: String },
    ToolCall { name: Option<String> },
    ResponseId { id: String },
    Done,
    Error { message: String },
}

#[derive(Clone)]
pub struct Client {
    pub(crate) http: reqwest::Client,
}

impl Client {
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("wryme/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("building http client")?;
        Ok(Self { http })
    }

    /// Open a streaming completion against the given shop with the given
    /// station's model + dials. Each StreamEvent is sent on `tx` as it
    /// arrives; returns when the upstream stream closes or errors.
    pub async fn stream_completion(
        &self,
        shop: Shop,
        station: Station,
        messages: Vec<ApiMessage>,
        previous_response_id: Option<String>,
        tx: UnboundedSender<StreamEvent>,
    ) {
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
                crate::api_chat::stream(self, &shop, &station, messages, &tx).await
            }
            Protocol::Responses => {
                crate::api_responses::stream(
                    self,
                    &shop,
                    &station,
                    messages,
                    previous_response_id,
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

// ---- SSE framing helpers used by both protocol files ----

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
        out.push_str("…");
        out
    }
}
