// Shared types and the protocol dispatcher for talking to an LLM endpoint.
//
// Each Client carries a (Shop, Station) pair. The shop owns the wire
// concerns (url, key, protocol). The station owns the model identity and
// the dials. The protocol-specific work lives in two sibling files:
//
//   api_chat.rs       /chat/completions (universal baseline)
//   api_responses.rs  /responses (typed events, kara, OpenAI Responses)
//
// StreamEvents emitted by either protocol:
//   Delta { text }      content delta to append to the current reply
//   Brain { text }      reasoning / thinking delta
//   ToolCall { name }   the model is calling a tool; drives "tinkering"
//   ResponseId { id }   captured from Responses response.created; replayed
//                       as previous_response_id next turn for session pinning
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
    pub(crate) shop: Shop,
    pub(crate) station: Station,
}

impl Client {
    pub fn new(shop: Shop, station: Station) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("wryme/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("building http client")?;
        Ok(Self { http, shop, station })
    }

    pub fn shop(&self) -> &Shop {
        &self.shop
    }

    pub fn station(&self) -> &Station {
        &self.station
    }

    /// Open a streaming completion. Each delta is sent over `tx` as it arrives.
    /// Returns when the upstream stream closes or errors.
    pub async fn stream_completion(
        &self,
        messages: Vec<ApiMessage>,
        previous_response_id: Option<String>,
        tx: UnboundedSender<StreamEvent>,
    ) {
        let result = match self.shop.protocol {
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
            Protocol::ChatCompletions => crate::api_chat::stream(self, messages, &tx).await,
            Protocol::Responses => {
                crate::api_responses::stream(self, messages, previous_response_id, &tx).await
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
