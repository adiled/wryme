// Responses wire protocol.
//
// POSTs to `<shop.url>/responses` with `stream: true`. Body uses `input`
// instead of `messages`, lifts the system prompt to a top-level
// `instructions` field, carries the model (from station) and translatable
// dials. When previous_response_id is set, we ship only the latest user
// turn instead of replaying the full history; the server has the rest
// pinned to its warm session.
//
// Tools: we advertise the shell tool (named after the user's real shell,
// e.g. `zsh`), its discovery companion (`zsh_explore`), and the async-job
// checker (`zsh_check`). When the model calls one we run it locally and
// feed the result back as a function_call_output item on a follow-up
// request, looping until the model stops calling tools. Finished async
// jobs are planted back here as a function_call + function_call_output
// pair so the model sees the outcome and continues.

use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;

use crate::api::{find_event_boundary, truncate, ApiMessage, Client, StreamEvent};
use crate::book;
use crate::shop::Shop;
use crate::tools;
use crate::station::{Patience, Station};

/// A function call the model made during one response stream.
struct FuncCall {
    /// The call id the server pairs a function_call_output with.
    call_id: String,
    /// The output item id; matches function_call_arguments.delta events.
    item_id: String,
    name: String,
    arguments: String,
}

pub(crate) async fn stream(
    client: &Client,
    shop: &Shop,
    station: &Station,
    messages: Vec<ApiMessage>,
    previous_response_id: Option<String>,
    engine: Arc<Mutex<book::Engine>>,
    tx: &UnboundedSender<StreamEvent>,
) -> Result<()> {
    let instructions: Option<&str> = messages
        .iter()
        .find(|m| m.role == "system")
        .map(|m| m.content.as_str());

    let conv_msgs: Vec<&ApiMessage> = messages
        .iter()
        .filter(|m| m.role != "system")
        .collect();

    // First request: full conversation, or just the latest user turn when
    // the session is pinned via previous_response_id. Established
    // compartments' rendered bookmarks ride along as `system` input items.
    let mut prev_id = previous_response_id;
    let mut input: Vec<serde_json::Value> = if prev_id.is_some() {
        conv_msgs
            .last()
            .into_iter()
            .map(|m| json_msg(m.role.as_str(), m.content.as_str()))
            .collect()
    } else {
        conv_msgs
            .iter()
            .map(|m| json_msg(m.role.as_str(), m.content.as_str()))
            .collect()
    };
    prepend_preamble(&mut input, &engine);

    // Plant any finished async jobs into the input as a check-call +
    // result pair, so the model sees the outcome and continues.
    let due = crate::jobs::claim_due();
    if !due.is_empty() {
        let check = crate::tools::check_name();
        for (id, output) in due {
            let call_id = format!("check_{id}");
            input.push(serde_json::json!({
                "type": "function_call",
                "call_id": call_id,
                "name": check,
                "arguments": format!("{{\"id\":{id}}}"),
            }));
            input.push(serde_json::json!({
                "type": "function_call_output",
                "call_id": call_id,
                "output": output,
            }));
        }
    }

    loop {
        let (calls, new_id) =
            stream_once(client, shop, station, &input, prev_id.as_deref(), instructions, tx)
                .await?;
        if calls.is_empty() {
            return Ok(());
        }

        // Execute each tool call locally and build the follow-up input.
        let mut next_input = Vec::new();
        for call in calls {
            let output = match tools::execute(&engine, &call.name, &call.arguments, &messages).await {
                Some(o) => o,
                None => format!("unknown tool '{}'", call.name),
            };
            let _ = tx.send(StreamEvent::ToolResult {
                name: call.name.clone(),
                output: output.clone(),
            });
            next_input.push(serde_json::json!({
                "type": "function_call_output",
                "call_id": call.call_id,
                "output": output,
            }));
        }
        // Re-pin the preamble: the model may have promoted a compartment
        // to the preamble this round.
        prepend_preamble(&mut next_input, &engine);
        prev_id = Some(new_id);
        input = next_input;
    }
}

/// Prepend the established compartments' rendered bookmarks as `system`
/// input items, so the model always sees its memory at the top.
fn prepend_preamble(input: &mut Vec<serde_json::Value>, engine: &Arc<Mutex<book::Engine>>) {
    let preambles = if let Ok(e) = engine.lock() {
        e.preamble()
    } else {
        return;
    };
    let mut items: Vec<serde_json::Value> = preambles
        .into_iter()
        .map(|p| serde_json::json!({ "type": "system", "content": p }))
        .collect();
    items.append(input);
    *input = items;
}

/// One request/response round. Streams content/brain/tool events to `tx`,
/// collects any function calls the model made, and returns them plus the
/// new response id (for pinning the follow-up request).
async fn stream_once(
    client: &Client,
    shop: &Shop,
    station: &Station,
    input: &[serde_json::Value],
    previous_response_id: Option<&str>,
    instructions: Option<&str>,
    tx: &UnboundedSender<StreamEvent>,
) -> Result<(Vec<FuncCall>, String)> {
    #[derive(Serialize)]
    struct ResponsesReq<'a> {
        model: &'a str,
        input: &'a [serde_json::Value],
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
        tools: &'a [serde_json::Value],
    }

    #[derive(Serialize)]
    struct Reasoning {
        effort: &'static str,
    }

    let reasoning = station.dials.patience.map(|p: Patience| Reasoning {
        effort: p.as_wire(),
    });
    let tools = tools::tool_defs();

    let base = shop.url.trim_end_matches('/');
    let url = format!("{}/responses", base);
    let body = ResponsesReq {
        model: &station.model,
        input,
        stream: true,
        instructions,
        previous_response_id,
        temperature: station.dials.boldness,
        max_output_tokens: station.dials.verbosity,
        reasoning,
        tools: &tools,
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
    let mut calls: Vec<FuncCall> = Vec::new();
    let mut new_id: Option<String> = None;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("reading sse chunk")?;
        buf.extend_from_slice(&chunk);
        loop {
            let Some(end) = find_event_boundary(&buf) else {
                break;
            };
            let event_bytes = buf.drain(..end.end).collect::<Vec<u8>>();
            let event = &event_bytes[..end.body_len];
            handle_event(event, tx, &mut calls, &mut new_id)?;
        }
    }
    if !buf.is_empty() {
        handle_event(&buf, tx, &mut calls, &mut new_id)?;
    }

    let new_id = new_id.context("no response.created seen")?;
    Ok((calls, new_id))
}

fn json_msg(role: &str, content: &str) -> serde_json::Value {
    serde_json::json!({ "role": role, "content": content })
}

/// Parse one SSE event body and emit matching StreamEvents. Function calls
/// are accumulated into `calls`; the newest response id lands in `new_id`.
fn handle_event(
    bytes: &[u8],
    tx: &UnboundedSender<StreamEvent>,
    calls: &mut Vec<FuncCall>,
    new_id: &mut Option<String>,
) -> Result<()> {
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
                    *new_id = Some(id.to_string());
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
                if item_type == "function_call" {
                    let item_id = item
                        .and_then(|i| i.get("id"))
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();
                    let call_id = item
                        .and_then(|i| i.get("call_id"))
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = item
                        .and_then(|i| i.get("name"))
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    let arguments = item
                        .and_then(|i| i.get("arguments"))
                        .and_then(|a| a.as_str())
                        .unwrap_or("")
                        .to_string();
                    if !name.is_empty() {
                        let _ = tx.send(StreamEvent::ToolCall {
                            name: Some(name.clone()),
                        });
                    }
                    calls.push(FuncCall {
                        call_id,
                        item_id,
                        name,
                        arguments,
                    });
                } else {
                    // Built-in tool calls (file/web/code search etc.) we
                    // don't run locally: still surface a name label.
                    let name: Option<String> = match item_type {
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
            }
            "response.function_call_arguments.delta" => {
                let item_id = v
                    .get("output_item_id")
                    .and_then(|i| i.as_str())
                    .unwrap_or("");
                if let Some(d) = v.get("delta").and_then(|d| d.as_str()) {
                    if let Some(c) = calls.iter_mut().find(|c| c.item_id == item_id) {
                        c.arguments.push_str(d);
                    }
                }
            }
            "response.output_item.done" => {
                let item = v.get("output_item");
                let item_type = item
                    .and_then(|i| i.get("type"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                if item_type == "function_call" {
                    let item_id = item
                        .and_then(|i| i.get("id"))
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();
                    if let Some(arguments) = item
                        .and_then(|i| i.get("arguments"))
                        .and_then(|a| a.as_str())
                    {
                        if let Some(c) = calls.iter_mut().find(|c| c.item_id == item_id) {
                            c.arguments = arguments.to_string();
                        }
                    }
                }
            }
            "response.file_search_call.in_progress"
            | "response.web_search_call.in_progress"
            | "response.code_interpreter_call.in_progress" => {
                let _ = tx.send(StreamEvent::ToolCall { name: None });
            }
            _ => { /* ignore */ }
        }
    }
    Ok(())
}
