// Chat Completions wire protocol.
//
// POSTs to `<base>/chat/completions` with `stream: true`. The response is
// SSE; each event body is a JSON `ChatChunk` with `choices[].delta` carrying
// content text, reasoning_content (DeepSeek/Qwen extension), and/or
// tool_calls. We surface the relevant pieces as StreamEvents and let the
// caller assemble them into a Message.

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
    }

    let base = client.station.url.trim_end_matches('/');
    let url = format!("{}/chat/completions", base);
    let body = Req {
        model: &client.station.model,
        messages: &messages,
        stream: true,
    };

    let mut req = client.http.post(&url).json(&body);
    if !client.station.key.is_empty() {
        req = req.bearer_auth(&client.station.key);
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
