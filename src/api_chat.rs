// Chat Completions wire protocol.
//
// POSTs to `<shop.url>/chat/completions` with `stream: true`. Body carries
// the model (from station), the message history, and any translatable
// dials. SSE response parsed for content / reasoning_content / tool_calls.

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

use crate::api::{find_event_boundary, truncate, ApiMessage, Client, StreamEvent};

pub(crate) async fn stream(
    client: &Client,
    messages: Vec<ApiMessage>,
    tx: &UnboundedSender<StreamEvent>,
) -> Result<()> {
    #[derive(Serialize)]
    struct Req<'a> {
        model: &'a str,
        messages: &'a [ApiMessage],
        stream: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        temperature: Option<f32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max_tokens: Option<u32>,
        // reasoning_effort has no Chat Completions equivalent; silently dropped.
    }

    let base = client.shop.url.trim_end_matches('/');
    let url = format!("{}/chat/completions", base);
    let body = Req {
        model: &client.station.model,
        messages: &messages,
        stream: true,
        temperature: client.station.dials.boldness,
        max_tokens: client.station.dials.verbosity,
    };

    let mut req = client.http.post(&url).json(&body);
    if !client.shop.key.is_empty() {
        req = req.bearer_auth(&client.shop.key);
    }
    let resp = req.send().await.context("posting chat/completions")?;

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
    function: Option<DeltaFunction>,
}

#[derive(Deserialize)]
struct DeltaFunction {
    #[serde(default)]
    name: Option<String>,
}
