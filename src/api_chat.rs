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

use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

use crate::api::{find_event_boundary, truncate, ApiMessage, Client, StreamEvent};
use crate::book;
use crate::shop::Shop;
use crate::tools;
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
    engine: Arc<Mutex<book::Engine>>,
    tx: &UnboundedSender<StreamEvent>,
) -> Result<()> {
    // Local conversation we grow across follow-up requests. Starts as the
    // incoming history (which already carries the preamble system
    // messages); tool calls and their results get appended here.
    let mut conv: Vec<serde_json::Value> = messages
        .iter()
        .filter_map(|m| json_msg(m))
        .collect();

    // Plant any finished async jobs back into the conversation as a
    // check-call + result pair, so the model sees the outcome naturally.
    let due = crate::jobs::claim_due();
    if !due.is_empty() {
        let check = crate::tools::check_name();
        for (id, output) in due {
            let call_id = format!("check_{id}");
            let args = format!("{{\"id\":{id}}}");
            conv.push(serde_json::json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": call_id,
                    "type": "function",
                    "function": { "name": check, "arguments": args },
                }],
            }));
            conv.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": output,
            }));
        }
    }

    let mut bad_rounds: u32 = 0;
    loop {
        let (calls, assistant_content) = stream_once(client, shop, station, &conv, tx).await?;
        // Split pairable calls from broken ones. Broken ones never reach
        // the wire (their shape would 400 this and every replayed turn),
        // but each still answers with a clear error the model can read.
        let (paired, broken): (Vec<_>, Vec<_>) = calls
            .into_iter()
            .partition(|c| !c.id.is_empty() && !c.name.is_empty());
        if paired.is_empty() && !broken.is_empty() {
            conv.push(serde_json::json!({
                "role": "assistant",
                "content": assistant_content,
            }));
        }
        for c in &broken {
            let output =
                "error: unusable tool call — every call needs an id and a function name".to_string();
            let _ = tx.send(StreamEvent::ToolResult {
                call_id: c.id.clone(),
                name: c.name.clone(),
                arguments: c.arguments.clone(),
                output: output.clone(),
            });
            conv.push(serde_json::json!({
                "role": "system",
                "content": output,
            }));
        }
        if paired.is_empty() {
            bad_rounds += 1;
            if bad_rounds > 2 {
                return Ok(());
            }
            continue;
        }
        bad_rounds = 0;
        let calls = paired;

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

        // Execute each tool call locally, persist the pair via a ToolResult
        // event, and append a `tool` result to the follow-up conversation.
        for c in &calls {
            let output = match tools::execute(&engine, &c.name, &c.arguments).await {
                Some(o) => o,
                None => format!("unknown tool '{}'", c.name),
            };
            let _ = tx.send(StreamEvent::ToolResult {
                call_id: c.id.clone(),
                name: c.name.clone(),
                arguments: c.arguments.clone(),
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
        stream_options: StreamOptions,
        #[serde(skip_serializing_if = "Option::is_none")]
        temperature: Option<f32>,
        // Token ceiling. `max_completion_tokens` covers visible + reasoning
        // tokens and is the only cap o-series models accept (`max_tokens`
        // is deprecated and rejected there).
        #[serde(skip_serializing_if = "Option::is_none")]
        max_completion_tokens: Option<u32>,
        // Patience dial. Chat's top-level `reasoning_effort`
        // (none/minimal/low/medium/high/...); omitted when unset.
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<&'a str>,
        tool_choice: &'a str,
        tools: &'a [serde_json::Value],
    }

    #[derive(Serialize)]
    struct StreamOptions {
        include_usage: bool,
    }

    let tools = tools::tool_defs_chat();
    let base = shop.url.trim_end_matches('/');
    let url = format!("{}/chat/completions", base);
    let body = Req {
        model: &station.model,
        messages: conv,
        stream: true,
        stream_options: StreamOptions { include_usage: true },
        temperature: station.dials.boldness,
        max_completion_tokens: station.dials.verbosity,
        reasoning_effort: station.dials.patience.map(|p| p.as_wire()),
        tool_choice: "auto",
        tools: &tools,
    };

    let mut req = client.http.post(&url).json(&body);
    if !shop.key.is_empty() {
        req = req.bearer_auth(&shop.key);
    }
    for (k, v) in &shop.headers {
        req = req.header(k, v);
    }
    tracing::debug!(
        url = %url,
        model = %station.model,
        body = %serde_json::to_string(&body).unwrap_or_default(),
        "chat request"
    );
    let resp = req.send().await.context("posting chat/completions")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        tracing::warn!(url = %url, %status, body = %truncate(&body, 2000), "chat upstream error");
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

fn json_msg(m: &ApiMessage) -> Option<serde_json::Value> {
    // A tool-role message: emit a `tool` message with the call_id + result.
    // Unpaired results (empty call_id) never go on the wire: strict
    // servers 400 them and the poison persists in history.
    if m.role == "tool" {
        if m.tool_call_id.is_empty() {
            return None;
        }
        return Some(serde_json::json!({
            "role": "tool",
            "tool_call_id": m.tool_call_id,
            "content": m.tool_result,
        }));
    }
    // An assistant message that made tool calls: attach the tool_calls array.
    let mut base = if m.images.is_empty() {
        serde_json::json!({ "role": m.role, "content": m.content })
    } else {
        // User message with image attachments: content becomes an array of
        // text + image_url parts, each image base64'd into a data URL.
        let mut parts: Vec<serde_json::Value> = Vec::new();
        if !m.content.is_empty() {
            parts.push(serde_json::json!({
                "type": "text",
                "text": m.content,
            }));
        }
        for path in &m.images {
            if let Some((mime, b64)) = crate::api::image_data_url(path) {
                parts.push(serde_json::json!({
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:{mime};base64,{b64}"),
                        "detail": "auto",
                    },
                }));
            }
        }
        serde_json::json!({
            "role": m.role,
            "content": parts,
        })
    };
    if !m.tool_calls.is_empty() {
        let tcs: Vec<serde_json::Value> = m.tool_calls.iter().filter(|c| {
            // Never replay unpaired calls: empty ids poison future turns.
            !c.id.is_empty() && !c.name.is_empty()
        }).map(|c| {
            serde_json::json!({
                "id": c.id,
                "type": "function",
                "function": { "name": c.name, "arguments": c.arguments },
            })
        }).collect();
        if tcs.is_empty() {
            base.as_object_mut().map(|o| o.remove("tool_calls"));
        } else {
            base["tool_calls"] = serde_json::json!(tcs);
        }
    }
    Some(base)
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
                // Token usage rides the final chunk (empty choices) when
                // `stream_options.include_usage` is set.
                if let Some(u) = chunk.usage.as_ref() {
                    let input = u.get("prompt_tokens").and_then(|n| n.as_u64()).unwrap_or(0);
                    let output = u
                        .get("completion_tokens")
                        .and_then(|n| n.as_u64())
                        .unwrap_or(0);
                    let total = u.get("total_tokens").and_then(|n| n.as_u64());
                    if input + output > 0 {
                        let _ = tx.send(StreamEvent::Usage { input, output });
                    } else if let Some(t) = total.filter(|t| *t > 0) {
                        let _ = tx.send(StreamEvent::Usage { input: t, output: 0 });
                    }
                }
                for choice in chunk.choices {
                    // Terminal reason for this choice. Surfaces truncation
                    // and content-filter cutoffs that are otherwise silent
                    // over SSE (no HTTP error, just a stopped stream).
                    match choice.finish_reason.as_deref() {
                        Some("length") => {
                            let _ = tx.send(StreamEvent::Error {
                                message: "stopped: token limit reached (bump verbosity)".into(),
                            });
                        }
                        Some("content_filter") => {
                            let _ = tx.send(StreamEvent::Error {
                                message: "stopped: content filter".into(),
                            });
                        }
                        _ => {}
                    }
                    if let Some(delta) = choice.delta {
                        if let Some(content) = delta.content {
                            if !content.is_empty() {
                                assistant_content.push_str(&content);
                                let _ = tx.send(StreamEvent::Delta { text: content });
                            }
                        }
                        // Refusal text is model output too; show it instead
                        // of dropping it.
                        if let Some(refusal) = delta.refusal {
                            if !refusal.is_empty() {
                                assistant_content.push_str(&refusal);
                                let _ = tx.send(StreamEvent::Delta { text: refusal });
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
                                        c.arguments.push_str(&arg_string(&a));
                                    }
                                }
                            }
                        }
                        // Legacy single-call shape some compat servers still
                        // emit. Folds into call 0 like a normal delta.
                        if let Some(f) = delta.function_call {
                            while calls.is_empty() {
                                calls.push(ChatToolCall {
                                    id: String::new(),
                                    name: String::new(),
                                    arguments: String::new(),
                                });
                            }
                            let c = &mut calls[0];
                            if let Some(n) = f.name {
                                c.name.push_str(&n);
                            }
                            if let Some(a) = f.arguments {
                                c.arguments.push_str(&arg_string(&a));
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
    // Present (with empty choices) on the final usage chunk when
    // `stream_options.include_usage` is set. Parsed into a Usage event.
    #[serde(default)]
    usage: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct Choice {
    #[serde(default)]
    finish_reason: Option<String>,
    #[serde(default)]
    delta: Option<Delta>,
}

#[derive(Deserialize)]
struct Delta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    refusal: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<DeltaToolCall>>,
    // Deprecated single-call shape; some compat servers still emit it.
    #[serde(default)]
    function_call: Option<DeltaFunction>,
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
    // Spec says string, but compat servers sometimes emit an object for
    // one-shot calls. Kept as Value so one odd field can't sink the
    // whole chunk (serde would drop the entire delta otherwise).
    #[serde(default)]
    arguments: Option<serde_json::Value>,
}

/// Arguments to string: verbatim when already a string, serialized when
/// a server sent an object instead.
fn arg_string(v: &serde_json::Value) -> String {
    if let Some(s) = v.as_str() {
        return s.to_string();
    }
    serde_json::to_string(v).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel() -> (
        UnboundedSender<StreamEvent>,
        tokio::sync::mpsc::UnboundedReceiver<StreamEvent>,
    ) {
        tokio::sync::mpsc::unbounded_channel()
    }

    #[test]
    fn refusal_delta_surfaces_as_text() {
        let (tx, mut rx) = channel();
        let mut calls = Vec::new();
        let mut content = String::new();
        handle_event(
            b"data: {\"choices\":[{\"delta\":{\"refusal\":\"sorry\"}}]}\n\n",
            &tx,
            &mut calls,
            &mut content,
        )
        .unwrap();
        assert!(content.contains("sorry"));
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, StreamEvent::Delta { text } if text == "sorry"));
    }

    #[test]
    fn finish_reason_length_and_filter_become_errors() {
        let (tx, mut rx) = channel();
        let mut calls = Vec::new();
        let mut content = String::new();
        handle_event(
            b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
            &tx,
            &mut calls,
            &mut content,
        )
        .unwrap();
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, StreamEvent::Error { message } if message.contains("token limit")));

        handle_event(
            b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"content_filter\"}]}\n\n",
            &tx,
            &mut calls,
            &mut content,
        )
        .unwrap();
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, StreamEvent::Error { message } if message.contains("content filter")));
    }

    #[test]
    fn legacy_function_call_delta_accumulates() {
        let (tx, _rx) = channel();
        let mut calls = Vec::new();
        let mut content = String::new();
        handle_event(
            b"data: {\"choices\":[{\"delta\":{\"function_call\":{\"name\":\"zsh\",\"arguments\":\"{\\\"command\\\":\\\"ls\\\"}\"}}}]}\n\n",
            &tx,
            &mut calls,
            &mut content,
        )
        .unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "zsh");
        assert_eq!(calls[0].arguments, "{\"command\":\"ls\"}");
    }

    #[test]
    fn unpaired_tool_history_never_reaches_wire() {
        use crate::api::{ApiMessage, ApiToolCall};
        let base = || ApiMessage {
            role: "assistant".into(),
            content: "hi".into(),
            images: vec![],
            tool_calls: vec![],
            tool_call_id: String::new(),
            tool_result: String::new(),
        };
        // Empty id: dropped, and with no valid calls the key vanishes.
        let mut m = base();
        m.tool_calls.push(ApiToolCall {
            id: "".into(),
            name: "zsh".into(),
            arguments: "{}".into(),
        });
        let v = json_msg(&m).unwrap();
        assert!(v.get("tool_calls").is_none());
        // Empty name: same.
        let mut m = base();
        m.tool_calls.push(ApiToolCall {
            id: "c1".into(),
            name: "".into(),
            arguments: "{}".into(),
        });
        let v = json_msg(&m).unwrap();
        assert!(v.get("tool_calls").is_none());
        // Valid call survives.
        let mut m = base();
        m.tool_calls.push(ApiToolCall {
            id: "c1".into(),
            name: "zsh".into(),
            arguments: "{}".into(),
        });
        let v = json_msg(&m).unwrap();
        assert_eq!(v["tool_calls"].as_array().unwrap().len(), 1);
        // Unpaired tool result is dropped entirely.
        let tool = ApiMessage {
            role: "tool".into(),
            content: "out".into(),
            images: vec![],
            tool_calls: vec![],
            tool_call_id: "".into(),
            tool_result: "out".into(),
        };
        assert!(json_msg(&tool).is_none());
    }

    #[test]
    fn usage_chunk_emits_usage_event() {        let (tx, mut rx) = channel();
        let mut calls = Vec::new();
        let mut content = String::new();
        handle_event(
            b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1200,\"completion_tokens\":300,\"total_tokens\":1500}}\n\n",
            &tx,
            &mut calls,
            &mut content,
        )
        .unwrap();
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, StreamEvent::Usage { input: 1200, output: 300 }));
    }

    #[test]
    fn usage_chunk_parses_cleanly() {        let (tx, _rx) = channel();
        let mut calls = Vec::new();
        let mut content = String::new();
        handle_event(
            b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1}}\n\ndata: [DONE]\n\n",
            &tx,
            &mut calls,
            &mut content,
        )
        .unwrap();
        assert!(calls.is_empty());
    }
}
