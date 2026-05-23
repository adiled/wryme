// OpenAI-compatible chat-completions streaming client.
//
// Talks to ANY endpoint that speaks the `/chat/completions` SSE protocol:
// OpenAI, Groq, Together, OpenRouter, Anyscale, Fireworks, vLLM, Ollama,
// LM Studio, and so on. This is the de-facto industry baseline.
//
// We emit a stream of `StreamEvent` values the UI can consume:
//   - Delta { text }            : content delta to append to the current reply
//   - Brain { text }            : reasoning/thinking delta (DeepSeek, Qwen, etc)
//   - ToolCall { name }         : the model is calling a tool. Name only on the
//                                 first delta for that call; subsequent argument
//                                 deltas arrive as ToolCall { name: None }. Used
//                                 by the UI as a "tinkering" indicator.
//   - Done                      : clean end of stream
//   - Error { message }         : anything we couldn't classify as success

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

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
    Done,
    Error { message: String },
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    station: Station,
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
    pub async fn stream_completion(
        &self,
        messages: Vec<ApiMessage>,
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
        if let Err(e) = self.stream_inner(messages, &tx).await {
            let _ = tx.send(StreamEvent::Error {
                message: format!("{:#}", e),
            });
        }
        let _ = tx.send(StreamEvent::Done);
    }

    async fn stream_inner(
        &self,
        messages: Vec<ApiMessage>,
        tx: &UnboundedSender<StreamEvent>,
    ) -> Result<()> {
        #[derive(Serialize)]
        struct Req<'a> {
            model: &'a str,
            messages: &'a [ApiMessage],
            stream: bool,
        }

        let base = self.station.url.trim_end_matches('/');
        let url = format!("{}/chat/completions", base);
        let body = Req {
            model: &self.station.model,
            messages: &messages,
            stream: true,
        };

        let mut req = self.http.post(&url).json(&body);
        if !self.station.key.is_empty() {
            req = req.bearer_auth(&self.station.key);
        }
        let resp = req.send().await.context("posting chat/completions")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("upstream {}: {}", status, truncate(&body, 800)));
        }

        let mut stream = resp.bytes_stream();
        // SSE framing: events are separated by \n\n. Within an event, lines
        // starting with `data: ` carry the payload. The terminator is the
        // sentinel payload `[DONE]`. We buffer across chunk boundaries.
        let mut buf: Vec<u8> = Vec::with_capacity(8 * 1024);

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("reading sse chunk")?;
            buf.extend_from_slice(&chunk);

            // Drain whole events from the front of the buffer.
            loop {
                let Some(end) = find_event_boundary(&buf) else {
                    break;
                };
                let event_bytes = buf.drain(..end.end).collect::<Vec<u8>>();
                let event = &event_bytes[..end.body_len];
                handle_event(event, tx)?;
            }
        }
        // Any trailing event without a final blank line.
        if !buf.is_empty() {
            handle_event(&buf, tx)?;
        }
        Ok(())
    }
}

struct Boundary {
    /// Length of the event body (excluding the trailing separator).
    body_len: usize,
    /// Total bytes to consume from the buffer.
    end: usize,
}

fn find_event_boundary(buf: &[u8]) -> Option<Boundary> {
    // Look for \n\n or \r\n\r\n.
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

fn handle_event(bytes: &[u8], tx: &UnboundedSender<StreamEvent>) -> Result<()> {
    let text = std::str::from_utf8(bytes).context("non-utf8 sse event")?;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        let Some(payload) = line.strip_prefix("data:") else {
            continue;
        };
        let payload = payload.trim_start();
        if payload == "[DONE]" {
            return Ok(());
        }
        if payload.is_empty() {
            continue;
        }
        match serde_json::from_str::<ChatChunk>(payload) {
            Ok(chunk) => {
                for choice in chunk.choices {
                    if let Some(delta) = choice.delta {
                        if let Some(content) = delta.content {
                            if !content.is_empty() {
                                let _ = tx.send(StreamEvent::Delta { text: content });
                            }
                        }
                        if let Some(reasoning) = delta.reasoning_content {
                            if !reasoning.is_empty() {
                                let _ = tx.send(StreamEvent::Brain { text: reasoning });
                            }
                        }
                        if let Some(tool_calls) = delta.tool_calls {
                            for tc in tool_calls {
                                let name = tc.function.and_then(|f| f.name);
                                let _ = tx.send(StreamEvent::ToolCall { name });
                            }
                        }
                    }
                }
            }
            Err(_) => {
                // Some providers emit non-JSON keepalive comments or vendor
                // events. Ignore. Only valid JSON matters.
            }
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct ChatChunk {
    #[serde(default)]
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    #[serde(default)]
    delta: Option<Delta>,
}

#[derive(Deserialize)]
struct Delta {
    #[serde(default)]
    content: Option<String>,
    /// Vendor extension shipped by DeepSeek, Qwen, OpenRouter passthroughs,
    /// and others that surface reasoning models' chain of thought separately
    /// from the visible answer. We render this as the "brain" block.
    #[serde(default)]
    reasoning_content: Option<String>,
    /// Tool-call deltas. We don't execute tools, just surface a "tinkering"
    /// indicator with the name so the user sees the model trying to do
    /// something instead of an empty bubble.
    #[serde(default)]
    tool_calls: Option<Vec<DeltaToolCall>>,
}

#[derive(Deserialize)]
struct DeltaToolCall {
    #[serde(default)]
    function: Option<DeltaFunction>,
}

#[derive(Deserialize)]
struct DeltaFunction {
    #[serde(default)]
    name: Option<String>,
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut out = s.chars().take(max).collect::<String>();
        out.push_str("…");
        out
    }
}
