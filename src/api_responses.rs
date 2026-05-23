// Responses wire protocol.
//
// POSTs to `<shop.url>/responses` with `stream: true`. Body uses `input`
// instead of `messages`, lifts the system prompt to a top-level
// `instructions` field, carries the model (from station) and translatable
// dials. When previous_response_id is set, we ship only the latest user
// turn instead of replaying the full history; the server has the rest
// pinned to its warm session.

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;

use crate::api::{find_event_boundary, truncate, ApiMessage, Client, StreamEvent};
use crate::shop::Shop;
use crate::station::{Patience, Station};

pub(crate) async fn stream(
    client: &Client,
    shop: &Shop,
    station: &Station,
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
        #[serde(skip_serializing_if = "Option::is_none")]
        temperature: Option<f32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max_output_tokens: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning: Option<Reasoning>,
    }

    #[derive(Serialize)]
    struct ResponsesInput<'a> {
        role: &'a str,
        content: &'a str,
    }

    #[derive(Serialize)]
    struct Reasoning {
        effort: &'static str,
    }

    let instructions: Option<&str> = messages
        .iter()
        .find(|m| m.role == "system")
        .map(|m| m.content.as_str());

    let conv_msgs: Vec<&ApiMessage> = messages
        .iter()
        .filter(|m| m.role != "system")
        .collect();

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

    let reasoning = station.dials.patience.map(|p: Patience| Reasoning {
        effort: p.as_wire(),
    });

    let base = shop.url.trim_end_matches('/');
    let url = format!("{}/responses", base);
    let body = ResponsesReq {
        model: &station.model,
        input,
        stream: true,
        instructions,
        previous_response_id: previous_response_id.as_deref(),
        temperature: station.dials.boldness,
        max_output_tokens: station.dials.verbosity,
        reasoning,
    };

    let mut req = client.http.post(&url).json(&body);
    if !shop.key.is_empty() {
        req = req.bearer_auth(&shop.key);
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
            handle_event(event, tx)?;
        }
    }
    if !buf.is_empty() {
        handle_event(&buf, tx)?;
    }
    Ok(())
}

/// Parse one SSE event body from the /responses stream and emit the
/// matching StreamEvent. Events are JSON objects with a "type" field;
/// anything we don't recognize is silently ignored.
fn handle_event(bytes: &[u8], tx: &UnboundedSender<StreamEvent>) -> Result<()> {
    let text = std::str::from_utf8(bytes).context("non-utf8 sse event")?;
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
