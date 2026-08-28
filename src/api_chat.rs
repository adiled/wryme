// Chat Completions wire protocol.
//
// POSTs to `<shop.url>/chat/completions` with `stream: true`. Body carries
// the model (from station), the message history, and any translatable
// dials. SSE response parsed for content / reasoning_content / tool_calls.
//
// Tools: we advertise `myshell_explore` (see explore.rs). When the model
// calls it, we run it locally and feed the result back as `tool` role
// messages on a follow-up request, looping until the model stops calling
// tools. This is the complete tool loop — not a stub.

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

use crate::api::{find_event_boundary, truncate, ApiMessage, Client, StreamEvent};
use crate::explore;
use crate::shop::Shop;
use crate::station::Station;

/// One tool call the model made, assembled from the streamed fragments.
struct ChatToolCall {
    id: String,
    name: String,
    arguments: String,
}

pub(crate) async fn stream(
    client: &Client,
    shop: &Shop,
    station: &Station,
    messages: Vec<ApiMessage>,
    tx: &UnboundedSender<StreamEvent>,
) -> Result<()> {
    // Local conversation we grow across follow-up requests. Starts as the
    // incoming history; tool calls and their results get appended here.
    let mut conv: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| json_msg(m.role.as_str(), m.content.as_str()))
        .collect();

    loop {
        let (calls, assistant_content) = stream_once(client, shop, station, &conv, tx).await?;
        if calls.is_empty() {
            return Ok(());
        }

        // The assistant message carrying the tool calls.
        let mut tcs = Vec::new();
        for c in &calls {
            tcs.push(serde_json::json!({
                "id": c.id,
                "type": "function",
                "function": { "name": c.name, "arguments": c.arguments },
            }));
        }
        conv.push(serde_json::json!({
            "role": "assistant",
            "content": assistant_content,
            "tool_calls": tcs,
        }));

        // Execute each tool call locally and append a `tool` result.
        for c in &calls {
            let output = match explore::execute(&c.name, &c.arguments).await {
                Some(o) => o,
                None => format!("unknown tool '{}'", c.name),
            };
            let _ = tx.send(StreamEvent::ToolResult {
                name: c.name.clone(),
                output: output.clone(),
            });
            conv.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": c.id,
                "content": output,
            }));
        }
        // Loop: re-request with the grown conversation.
    }
}

/// One request/response round. Streams content/brain/tool events to `tx`,
/// assembles any tool calls into `Vec<ChatToolCall>`, and returns them
/// plus the assistant text streamed this round.
async fn stream_once(
    client: &Client,
    shop: &Shop,
    station: &Station,
    conv: &[serde_json::Value],
    tx: &UnboundedSender<StreamEvent>,
) -> Result<(Vec<ChatToolCall>, String)> {
    #[derive(Serialize)]
    struct Req<'a> {
        model: &'a str,
        messages: &'a [serde_json::Value],
        stream: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        temperature: Option<f32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max_tokens: Option<u32>,
        tools: &'a [serde_json::Value],
    }

    let tools = [explore_tool_json()];
    let base = shop.url.trim_end_matches('/');
    let url = format!("{}/chat/completions", base);
    let body = Req {
        model: &station.model,
        messages: conv,
        stream: true,
        temperature: station.dials.boldness,
        max_tokens: station.dials.verbosity,
        tools: &tools,
    };

    let mut req = client.http.post(&url).json(&body);
    if !shop.key.is_empty() {
        req = req.bearer_auth(&shop.key);
    }
    let resp = req.send().await.context("posting chat/completions")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("upstream {}: {}", status, truncate(&body, 800)));
    }

    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::with_capacity(8 * 1024);
    let mut calls: Vec<ChatToolCall> = Vec::new();
    let mut assistant_content = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("reading sse chunk")?;
        buf.extend_from_slice(&chunk);
        loop {
            let Some(end) = find_event_boundary(&buf) else {
                break;
            };
            let event_bytes = buf.drain(..end.end).collect::<Vec<u8>>();
            let event = &event_bytes[..end.body_len];
            handle_event(event, tx, &mut calls, &mut assistant_content)?;
        }
    }
    if !buf.is_empty() {
        handle_event(&buf, tx, &mut calls, &mut assistant_content)?;
    }

    // Surface the tool name for the UI label once per call.
    for c in calls.iter().filter(|c| !c.name.is_empty()) {
        let _ = tx.send(StreamEvent::ToolCall {
            name: Some(c.name.clone()),
        });
    }

    Ok((calls, assistant_content))
}

/// Advertise myshell_explore in the Chat Completions `tools` array.
fn explore_tool_json() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "name": explore::TOOL_NAME,
        "description": explore::TOOL_DESCRIPTION,
        "parameters": explore::tool_parameters(),
    })
}

fn json_msg(role: &str, content: &str) -> serde_json::Value {
    serde_json::json!({ "role": role, "content": content })
}

fn handle_event(
    bytes: &[u8],
    tx: &UnboundedSender<StreamEvent>,
    calls: &mut Vec<ChatToolCall>,
    assistant_content: &mut String,
) -> Result<()> {
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
                                assistant_content.push_str(&content);
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
                                let idx = tc.index.unwrap_or(0) as usize;
                                while calls.len() <= idx {
                                    calls.push(ChatToolCall {
                                        id: String::new(),
                                        name: String::new(),
                                        arguments: String::new(),
                                    });
                                }
                                let c = &mut calls[idx];
                                if let Some(id) = tc.id {
                                    c.id = id;
                                }
                                if let Some(f) = tc.function {
                                    if let Some(n) = f.name {
                                        c.name.push_str(&n);
                                    }
                                    if let Some(a) = f.arguments {
                                        c.arguments.push_str(&a);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Err(_) => {
                // Vendor extensions or keepalive comments. Ignore.
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
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<DeltaToolCall>>,
}

#[derive(Deserialize)]
struct DeltaToolCall {
    #[serde(default)]
    index: Option<u32>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<DeltaFunction>,
}

#[derive(Deserialize)]
struct DeltaFunction {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}
