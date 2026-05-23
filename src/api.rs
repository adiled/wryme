// Shared types and the protocol dispatcher for talking to an LLM endpoint.
//
// The actual wire-format work lives in two sibling files, one per protocol:
//   - api_chat.rs       OpenAI-compatible /chat/completions (the universal
//                       baseline; Ollama, Groq, LM Studio, vanilla OpenAI
//                       chat, etc.)
//   - api_responses.rs  OpenAI-compatible /responses (the typed-event newer
//                       protocol; OpenAI directly, kara, agentic backends)
//
// This file owns the Client, the streams' shared event vocabulary
// (StreamEvent), the message struct sent upstream (ApiMessage), and the
// SSE framing helpers both protocols use.
//
// StreamEvents emitted by either protocol:
//   - Delta { text }      content delta to append to the current reply
//   - Brain { text }      reasoning/thinking delta (DeepSeek/Qwen on chat,
//                         response.reasoning_summary_text.delta on responses)
//   - ToolCall { name }   the model is calling a tool. Drives the
//                         "tinkering" indicator. Name is Some on the first
//                         delta for that call, None on continuation chunks.
//   - ResponseId { id }   id from a Responses response.created event.
//                         Stashed by App and replayed as previous_response_id
//                         on the next turn so the server keeps us pinned to
//                         the same warm session.
//   - Done                clean end of stream
//   - Error { message }   anything we couldn't classify as success

use anyhow::{Context, Result};
use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;

use crate::station::{Protocol, Station};

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
    // pub(crate) so the protocol-specific sibling files can read these
    // without going through accessor methods.
    pub(crate) http: reqwest::Client,
    pub(crate) station: Station,
}

impl Client {
    pub fn new(station: Station) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("wryme/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("building http client")?;
        Ok(Self { http, station })
    }

    pub fn station(&self) -> &Station {
        &self.station
    }

    /// Open a streaming completion. Each delta is sent over `tx` as it arrives.
    /// Returns when the upstream stream closes or errors.
    ///
    /// `previous_response_id` is only honored by Responses-protocol stations;
    /// Chat Completions ignores it. Used for session continuity so the server
    /// can pin requests to the same warm session.
    pub async fn stream_completion(
        &self,
        messages: Vec<ApiMessage>,
        previous_response_id: Option<String>,
        tx: UnboundedSender<StreamEvent>,
    ) {
        if self.station.is_demo {
            let prompt = messages
                .iter()
                .rev()
                .find(|m| m.role == "user")
                .map(|m| m.content.as_str())
                .unwrap_or("");
            crate::demo::stream(prompt, tx.clone()).await;
            let _ = tx.send(StreamEvent::Done);
            return;
        }
        let result = match self.station.protocol {
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
    /// Length of the event body (excluding the trailing separator).
    pub body_len: usize,
    /// Total bytes to consume from the buffer.
    pub end: usize,
}

/// Find the next event boundary (`\n\n` or `\r\n\r\n`) in the buffer.
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

/// Truncate a string to `max` characters with an ellipsis. Used to keep
/// upstream error bodies from blowing up the status line.
pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut out = s.chars().take(max).collect::<String>();
        out.push_str("…");
        out
    }
}
