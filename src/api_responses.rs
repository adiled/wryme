// Responses wire protocol.
//
// POSTs to `<base>/responses` with `stream: true`. The response is SSE
// with typed events; each `data:` line carries a JSON object with a
// "type" field that tells us what kind of update it is. We pattern-match
// the relevant types and surface them as StreamEvents; everything else
// is silently ignored so future server-side additions don't break us.
//
// Two things this protocol does that Chat Completions cannot:
//
//   1. Session continuity. The first event (response.created) carries an
//      id. We emit it as StreamEvent::ResponseId so App can stash it and
//      hand it back as `previous_response_id` next turn. With that set,
//      we only ship the new user message in `input`; the server already
//      has the prior turns pinned to the warm session.
//
//   2. First-class tool indications. `response.output_item.added` events
//      announce a new output item, including function calls, MCP server
//      calls, and built-in tools (file_search, web_search, etc.). We
//      surface the tool name as a StreamEvent::ToolCall so the UI can
//      switch the header indicator to "tinkering" with the name on the
//      right.

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;

use crate::api::{find_event_boundary, truncate, ApiMessage, Client, StreamEvent};

pub(crate) async fn stream(
    client: &Client,
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

    let base = client.station.url.trim_end_matches('/');
    let url = format!("{}/responses", base);
    let body = ResponsesReq {
        model: &client.station.model,
        input,
        stream: true,
        instructions,
        previous_response_id: previous_response_id.as_deref(),
    };

    let mut req = client.http.post(&url).json(&body);
    if !client.station.key.is_empty() {
        req = req.bearer_auth(&client.station.key);
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
