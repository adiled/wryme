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
//   - ResponseId { id }         : id from a Responses-protocol response.created
//                                 event. Stashed by App and replayed as
//                                 previous_response_id on the next request so
//                                 the server pins us to the same warm session.
//   - Done                      : clean end of stream
//   - Error { message }         : anything we couldn't classify as success

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
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
            Protocol::ChatCompletions => self.stream_chat(messages, &tx).await,
            Protocol::Responses => {
                self.stream_responses(messages, previous_response_id, &tx)
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

    async fn stream_chat(
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

    /// POST to /responses with typed SSE events. Each event is a JSON
    /// object with a "type" field. We pattern-match the types that
    /// correspond to our StreamEvent variants and drop the rest.
    async fn stream_responses(
        &self,
        messages: Vec<ApiMessage>,
        previous_response_id: Option<String>,
        tx: &UnboundedSender<StreamEvent>,
    ) -> Result<()> {
        #[derive(Serialize)]
        struct ResponsesReq<'a> {
            model: &'a str,
            input: Vec<ResponsesInput<'a>>,
            stream: bool,
            #[serde(skip_serializing_if = "Option::is_none")]
            instructions: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            previous_response_id: Option<&'a str>,
        }

        #[derive(Serialize)]
        struct ResponsesInput<'a> {
            role: &'a str,
            content: &'a str,
        }

        // The Responses API takes the system prompt as a top-level
        // `instructions` field instead of an in-band message.
        let instructions: Option<&str> = messages
            .iter()
            .find(|m| m.role == "system")
            .map(|m| m.content.as_str());

        // Conversation messages, system stripped (it lives in `instructions`).
        let conv_msgs: Vec<&ApiMessage> = messages
            .iter()
            .filter(|m| m.role != "system")
            .collect();

        // When `previous_response_id` is set, the server already has the
        // entire prior conversation pinned to that session. Replaying it
        // would duplicate every turn. Send only the latest user message
        // (the new turn). When no previous id, send the whole history
        // because the server has no other way to see it.
        let input: Vec<ResponsesInput> = if previous_response_id.is_some() {
            conv_msgs
                .last()
                .into_iter()
                .map(|m| ResponsesInput {
                    role: m.role.as_str(),
                    content: m.content.as_str(),
                })
                .collect()
        } else {
            conv_msgs
                .iter()
                .map(|m| ResponsesInput {
                    role: m.role.as_str(),
                    content: m.content.as_str(),
                })
                .collect()
        };

        let base = self.station.url.trim_end_matches('/');
        let url = format!("{}/responses", base);
        let body = ResponsesReq {
            model: &self.station.model,
            input,
            stream: true,
            instructions,
            previous_response_id: previous_response_id.as_deref(),
        };

        let mut req = self.http.post(&url).json(&body);
        if !self.station.key.is_empty() {
            req = req.bearer_auth(&self.station.key);
        }
        let resp = req.send().await.context("posting responses")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("upstream {}: {}", status, truncate(&body, 800)));
        }

        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::with_capacity(8 * 1024);
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("reading sse chunk")?;
            buf.extend_from_slice(&chunk);
            loop {
                let Some(end) = find_event_boundary(&buf) else {
                    break;
                };
                let event_bytes = buf.drain(..end.end).collect::<Vec<u8>>();
                let event = &event_bytes[..end.body_len];
                handle_responses_event(event, tx)?;
            }
        }
        if !buf.is_empty() {
            handle_responses_event(&buf, tx)?;
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

/// Parse one SSE event body from the /responses stream and emit the
/// matching StreamEvent. Events are JSON objects with a "type" field;
/// anything we don't recognize is silently ignored.
fn handle_responses_event(bytes: &[u8], tx: &UnboundedSender<StreamEvent>) -> Result<()> {
    let text = std::str::from_utf8(bytes).context("non-utf8 sse event")?;
    // An SSE event body can include `event:` and `data:` lines. We only
    // need the `data:` payload; the JSON inside carries its own `type`.
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        let Some(payload) = line.strip_prefix("data:") else {
            continue;
        };
        let payload = payload.trim_start();
        if payload == "[DONE]" || payload.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(payload) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let event_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match event_type {
            "response.created" => {
                // Capture the response id so we can replay it as
                // previous_response_id on the next turn and stay pinned
                // to the same server-side session.
                if let Some(id) = v
                    .get("response")
                    .and_then(|r| r.get("id"))
                    .and_then(|i| i.as_str())
                {
                    let _ = tx.send(StreamEvent::ResponseId {
                        id: id.to_string(),
                    });
                }
            }
            "response.output_text.delta" => {
                if let Some(d) = v.get("delta").and_then(|d| d.as_str()) {
                    if !d.is_empty() {
                        let _ = tx.send(StreamEvent::Delta { text: d.to_string() });
                    }
                }
            }
            "response.reasoning_summary_text.delta" => {
                if let Some(d) = v.get("delta").and_then(|d| d.as_str()) {
                    if !d.is_empty() {
                        let _ = tx.send(StreamEvent::Brain { text: d.to_string() });
                    }
                }
            }
            "response.output_item.added" => {
                // A new output item appeared. If it is a tool call, surface
                // the tool name. Built-in tools (file_search, web_search,
                // code_interpreter, etc.) get their type as the name.
                // Explicit function calls and MCP server calls carry a
                // "name" field on the item.
                let item = v.get("item");
                let item_type = item
                    .and_then(|i| i.get("type"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                let name: Option<String> = match item_type {
                    "function_call" | "mcp_call" => item
                        .and_then(|i| i.get("name"))
                        .and_then(|n| n.as_str())
                        .map(String::from),
                    "file_search_call" => Some("file_search".into()),
                    "web_search_call" => Some("web_search".into()),
                    "code_interpreter_call" => Some("code_interpreter".into()),
                    "image_generation_call" => Some("image_generation".into()),
                    "computer_use_call" => Some("computer_use".into()),
                    _ => None,
                };
                if name.is_some() {
                    let _ = tx.send(StreamEvent::ToolCall { name });
                }
            }
            // Built-in tool progress events. Each fires throughout the
            // tool's run; the indicator stays "tinkering" until the model
            // resumes writing.
            "response.file_search_call.in_progress"
            | "response.file_search_call.searching"
            | "response.web_search_call.in_progress"
            | "response.web_search_call.searching"
            | "response.code_interpreter_call.in_progress"
            | "response.code_interpreter_call.interpreting" => {
                let _ = tx.send(StreamEvent::ToolCall { name: None });
            }
            _ => { /* ignore */ }
        }
    }
    Ok(())
}
