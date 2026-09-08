// Responses wire protocol.
//
// POSTs to `<shop.url>/responses` with `stream: true`. Body uses `input`
// instead of `messages`, lifts the system prompt to a top-level
// `instructions` field, carries the model (from station) and translatable
// dials. Two window modes (shop `window`): `Full` replays the whole
// transcript statelessly (`store: false`); `Warm` pins follow-ups to
// `previous_response_id` (`store: true`) for shops that retain windows.
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
use crate::shop::{Shop, WindowMode};
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

/// Server-side per-turn cap on stacked tool calls: bounds spend when a
/// model keeps calling instead of answering.
const MAX_TOOL_CALLS: u32 = 10;

pub(crate) async fn stream(
    client: &Client,
    shop: &Shop,
    station: &Station,
    messages: Vec<ApiMessage>,
    previous_response_id: Option<String>,
    engine: Arc<Mutex<book::Engine>>,
    tx: &UnboundedSender<StreamEvent>,
) -> Result<()> {
    if shop.window == WindowMode::Warm {
        match stream_warm(
            client,
            shop,
            station,
            messages.clone(),
            previous_response_id,
            engine.clone(),
            tx,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(e) if crate::api::is_tool_unsupported_msg(&format!("{e:#}")) => {
                if crate::api::is_toolless(&station.model) {
                    return Err(e);
                }
                crate::api::mark_toolless(&station.model);
                tracing::warn!(model=%station.model, "tools unsupported, retrying responses without tools");
                let _ = tx.send(StreamEvent::Error {
                    message: "tools unsupported by model, retrying without tools".into(),
                });
                return stream_warm(client, shop, station, messages, None, engine, tx).await;
            }
            Err(e) => {
                // A warm attempt can fail two ways: the shop names the
                // problem ("previous_response_id is not supported") or it
                // just 400s the whole body ("Invalid JSON request", as ds4
                // does). Either way the pinned id is unusable: tell the UI
                // to pin this shop to full windows (runtime-only, once),
                // then serve this turn with a full replay instead of
                // erroring. Anything else (5xx, network) propagates.
                let msg = format!("{e:#}");
                if msg.contains("previous_response_id")
                    || msg.contains("upstream 400")
                    || msg.contains("upstream 404")
                    || msg.contains("upstream 422")
                {
                    tracing::warn!(shop = %shop.name, err = %crate::api::truncate(&msg, 500), "warm unsupported, full replay");
                    let _ = tx.send(StreamEvent::WindowUnsupported {
                        shop: shop.name.clone(),
                    });
                    return stream_full(client, shop, station, messages, engine, tx).await;
                }
                return Err(e);
            }
        }
    }
    match stream_full(client, shop, station, messages.clone(), engine.clone(), tx).await {
        Ok(()) => Ok(()),
        Err(e) if crate::api::is_tool_unsupported_msg(&format!("{e:#}")) => {
            if crate::api::is_toolless(&station.model) {
                return Err(e);
            }
            crate::api::mark_toolless(&station.model);
            tracing::warn!(model=%station.model, "tools unsupported, retrying responses without tools");
            let _ = tx.send(StreamEvent::Error {
                message: "tools unsupported by model, retrying without tools".into(),
            });
            stream_full(client, shop, station, messages, engine, tx).await
        }
        Err(e) => Err(e),
    }
}

/// Warm window: the server retains the conversation (`store: true`).
/// First request carries the full transcript; follow-ups send only the
/// new tool outputs against `previous_response_id`. Fast on long
/// windows; only for shops that actually keep windows.
async fn stream_warm(
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

    let mut prev_id = previous_response_id;
    let mut input: Vec<serde_json::Value> = if prev_id.is_some() {
        conv_msgs
            .last()
            .into_iter()
            .flat_map(|m| json_msg(m))
            .collect()
    } else {
        conv_msgs
            .iter()
            .flat_map(|m| json_msg(m))
            .collect()
    };
    prepend_preamble_counted(&mut input, &engine, 0);

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

    let mut bad_rounds: u32 = 0;
    loop {
        let (calls, new_id, _) = stream_once(
            client,
            shop,
            station,
            &input,
            prev_id.as_deref(),
            true,
            instructions,
            tx,
        )
        .await?;
        let (paired, broken): (Vec<_>, Vec<_>) = calls
            .into_iter()
            .partition(|c| !c.call_id.is_empty() && !c.name.is_empty());
        let mut next_input = Vec::new();
        for c in &broken {
            let output =
                "error: unusable tool call — every call needs an id and a function name".to_string();
            let _ = tx.send(StreamEvent::ToolResult {
                call_id: c.call_id.clone(),
                name: c.name.clone(),
                arguments: c.arguments.clone(),
                output: output.clone(),
            });
            next_input.push(serde_json::json!({
                "type": "message",
                "role": "system",
                "content": [{ "type": "input_text", "text": output }],
            }));
        }
        if paired.is_empty() {
            if broken.is_empty() {
                return Ok(());
            }
            bad_rounds += 1;
            if bad_rounds > 2 {
                return Ok(());
            }
            // preamble is top-of-conversation only — follow-up tool rounds reuse the warm
            // server context (store:true) so no preamble re-injection.
            prev_id = Some(new_id);
            input = next_input;
            continue;
        }
        bad_rounds = 0;
        let calls = paired;
        for call in calls {
            let output = match tools::execute(&engine, &call.name, &call.arguments).await {
                Some(o) => o,
                None => format!("unknown tool '{}'", call.name),
            };
            let _ = tx.send(StreamEvent::ToolResult {
                call_id: call.call_id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
                output: output.clone(),
            });
            next_input.push(serde_json::json!({
                "type": "function_call_output",
                "call_id": call.call_id,
                "output": output,
            }));
        }
        prev_id = Some(new_id);
        input = next_input;
    }
}

/// Full window: stateless (`store: false`). The whole transcript,
/// including tool history, rides every request, so any shop works.
/// Costs a full prefill per tool round on long windows.
async fn stream_full(
    client: &Client,
    shop: &Shop,
    station: &Station,
    messages: Vec<ApiMessage>,
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

    let mut input: Vec<serde_json::Value> = conv_msgs
        .iter()
        .flat_map(|m| json_msg(m))
        .collect();
    let _preamble_len = prepend_preamble_counted(&mut input, &engine, 0);

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

    let mut bad_rounds: u32 = 0;
    loop {
        let (calls, _new_id, reasoning_items) =
            stream_once(client, shop, station, &input, None, false, instructions, tx).await?;
        let (paired, broken): (Vec<_>, Vec<_>) = calls
            .into_iter()
            .partition(|c| !c.call_id.is_empty() && !c.name.is_empty());
        let mut follow = Vec::new();
        follow.extend(reasoning_items);
        for c in &broken {
            let output =
                "error: unusable tool call — every call needs an id and a function name".to_string();
            let _ = tx.send(StreamEvent::ToolResult {
                call_id: c.call_id.clone(),
                name: c.name.clone(),
                arguments: c.arguments.clone(),
                output: output.clone(),
            });
            follow.push(serde_json::json!({
                "type": "message",
                "role": "system",
                "content": [{ "type": "input_text", "text": output }],
            }));
        }
        if paired.is_empty() {
            if broken.is_empty() {
                return Ok(());
            }
            // Nothing to execute: re-ask with the error notes so the model
            // can correct itself, but stop feeding a model that won't.
            bad_rounds += 1;
            if bad_rounds > 2 {
                return Ok(());
            }
            input.extend(follow);
            continue;
        }
        bad_rounds = 0;
        let calls = paired;
        for call in calls {
            let output = match tools::execute(&engine, &call.name, &call.arguments).await {
                Some(o) => o,
                None => format!("unknown tool '{}'", call.name),
            };
            let _ = tx.send(StreamEvent::ToolResult {
                call_id: call.call_id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
                output: output.clone(),
            });
            follow.push(serde_json::json!({
                "type": "function_call",
                "call_id": call.call_id.clone(),
                "name": call.name.clone(),
                "arguments": call.arguments.clone(),
            }));
            follow.push(serde_json::json!({
                "type": "function_call_output",
                "call_id": call.call_id,
                "output": output,
            }));
        }
        input.extend(follow);
        // preamble is top-of-conversation only — not re-pinned on every tool
        // follow-up (would be insane on long stateless replays).
    }
}

/// Prepend the established compartments' rendered bookmarks and any
/// book-writing prod as `system` input items, so the model always sees
/// its memory at the top. (The Responses protocol drops system messages
/// other than the first instructions, so the engine's preamble must be
/// injected as real input items each round.)
///
/// Counted variant: swaps out the `old_len` previously prepended items so
/// a growing stateless input never accumulates duplicate preambles.
/// Returns the new prepended count.
fn prepend_preamble_counted(
    input: &mut Vec<serde_json::Value>,
    engine: &Arc<Mutex<book::Engine>>,
    old_len: usize,
) -> usize {
    let drain = old_len.min(input.len());
    input.drain(..drain);
    let (preambles, prod) = if let Ok(mut e) = engine.lock() {
        (e.preamble(), e.take_prod())
    } else {
        return 0;
    };
    let mut items: Vec<serde_json::Value> = preambles
        .into_iter()
        .map(|p| {
            serde_json::json!({
                "type": "message",
                "role": "system",
                "content": [{ "type": "input_text", "text": p }],
            })
        })
        .collect();
    if let Some(prod) = prod {
        items.push(serde_json::json!({
            "type": "message",
            "role": "system",
            "content": [{ "type": "input_text", "text": prod }],
        }));
    }
    let n = items.len();
    items.append(input);
    *input = items;
    n
}

/// One request/response round. Streams content/brain/tool events to `tx`,
/// collects any function calls the model made plus completed reasoning
/// items (for stateless resume), and returns them with the new response
/// id (captured for the UI; never replayed — `store: false` keeps no
/// server state).
async fn stream_once(
    client: &Client,
    shop: &Shop,
    station: &Station,
    input: &[serde_json::Value],
    previous_response_id: Option<&str>,
    store: bool,
    instructions: Option<&str>,
    tx: &UnboundedSender<StreamEvent>,
) -> Result<(Vec<FuncCall>, String, Vec<serde_json::Value>)> {
    #[derive(Serialize)]
    struct ResponsesReq<'a> {
        model: &'a str,
        input: &'a [serde_json::Value],
        stream: bool,
        // Warm windows retain server-side; full windows carry everything
        // and retain nothing. Never sends a previous id it wasn't given.
        store: bool,
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
        // Only when reasoning runs: lets the server return
        // encrypted_content so follow-ups keep reasoning context with
        // store:false.
        #[serde(skip_serializing_if = "Option::is_none")]
        include: Option<Vec<&'a str>>,
        // Runaway guard: most tool calls the model may stack up in one
        // turn before the server cuts it off.
        max_tool_calls: u32,
        // Oversize input trims from the conversation head instead of
        // 400ing. Matters on long stateless replays.
        truncation: &'a str,
        tools: &'a [serde_json::Value],
    }

    #[derive(Serialize)]
    struct Reasoning {
        // Depth knob, only when the station sets patience. Omitted
        // otherwise so trivial turns stay fast and the model's native
        // thinking depth is untouched.
        #[serde(skip_serializing_if = "Option::is_none")]
        effort: Option<&'static str>,
        // Asked whenever we ask for thinking at all: without a summary
        // most servers emit no reasoning stream, and the Brain display
        // goes dark.
        summary: &'static str,
    }

    // Thinking is opt-in via the patience dial, never forced: requesting
    // reasoning makes even trivial turns think out loud (slow). The Brain
    // display shows whatever the server volunteers regardless.
    let reasoning = station.dials.patience.map(|p: Patience| Reasoning {
        effort: Some(p.as_wire()),
        summary: "auto",
    });
    let include = reasoning
        .as_ref()
        .map(|_| vec!["reasoning.encrypted_content"]);
    let toolless = crate::api::is_toolless(&station.model);
    let tools = if toolless {
        Vec::new()
    } else {
        tools::tool_defs()
    };
    // For toolless models strip any prior tool history from input so the server
    // doesn't reject poisoned function_call items.
    let filtered_input: Vec<serde_json::Value>;
    let input: &[serde_json::Value] = if toolless {
        filtered_input = input
            .iter()
            .filter(|v| {
                let t = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
                t != "function_call" && t != "function_call_output"
            })
            .cloned()
            .collect();
        if filtered_input.is_empty() { input } else { &filtered_input }
    } else {
        input
    };

    let base = shop.url.trim_end_matches('/');
    let url = format!("{}/responses", base);
    let body = ResponsesReq {
        model: &station.model,
        input,
        stream: true,
        store,
        instructions,
        previous_response_id,
        temperature: station.dials.boldness,
        max_output_tokens: station.dials.verbosity,
        reasoning,
        include,
        max_tool_calls: MAX_TOOL_CALLS,
        truncation: "auto",
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
        store = body.store,
        body = %serde_json::to_string(&body).unwrap_or_default(),
        "responses request"
    );
    let resp = req.send().await.context("posting responses")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        tracing::warn!(url = %url, %status, body = %truncate(&body, 2000), "responses upstream error");
        return Err(anyhow!("upstream {}: {}", status, truncate(&body, 800)));
    }

    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::with_capacity(8 * 1024);
    let mut calls: Vec<FuncCall> = Vec::new();
    let mut reasoning_items: Vec<serde_json::Value> = Vec::new();
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
            handle_event(event, tx, &mut calls, &mut reasoning_items, &mut new_id)?;
        }
    }
    if !buf.is_empty() {
        handle_event(&buf, tx, &mut calls, &mut reasoning_items, &mut new_id)?;
    }

    let new_id = new_id.context("no response.created seen")?;
    Ok((calls, new_id, reasoning_items))
}

fn json_msg(m: &ApiMessage) -> Vec<serde_json::Value> {
    // A tool result: the Responses item is `function_call_output`, keyed
    // by the call id it answers. Unpaired results never go on the wire.
    if m.role == "tool" {
        if m.tool_call_id.is_empty() {
            return vec![];
        }
        return vec![serde_json::json!({
            "type": "function_call_output",
            "call_id": m.tool_call_id,
            "output": m.tool_result,
        })];
    }
    // An assistant turn that called tools: replay each call as a
    // `function_call` item (plus the text as a message item when any),
    // so a stateless shop sees the full transcript. Unpaired calls are
    // dropped: replaying them poisons every future turn.
    if !m.tool_calls.is_empty() {
        let mut items: Vec<serde_json::Value> = Vec::new();
        if !m.content.is_empty() {
            items.push(serde_json::json!({ "role": m.role, "content": m.content }));
        }
        for c in m.tool_calls.iter().filter(|c| !c.id.is_empty() && !c.name.is_empty()) {
            items.push(serde_json::json!({
                "type": "function_call",
                "call_id": c.id,
                "name": c.name,
                "arguments": c.arguments,
            }));
        }
        return items;
    }
    if m.images.is_empty() {
        return vec![serde_json::json!({ "role": m.role, "content": m.content })];
    }
    // User message with images: the Responses protocol takes `input` items,
    // so each image becomes its own `input_image` item (per OpenAI's
    // /responses input_image schema) with a base64 data-URL `image_url`, and
    // the text rides as a normal `message` item.
    let mut items: Vec<serde_json::Value> = Vec::new();
    if !m.content.is_empty() {
        items.push(serde_json::json!({
            "type": "message",
            "role": m.role,
            "content": [{ "type": "input_text", "text": m.content }],
        }));
    }
    for path in &m.images {
        if let Some((mime, b64)) = crate::api::image_data_url(path) {
            items.push(serde_json::json!({
                "type": "input_image",
                "image_url": format!("data:{mime};base64,{b64}"),
                "detail": "auto",
            }));
        }
    }
    items
}
/// Parse one SSE event body and emit matching StreamEvents. Function calls
/// are accumulated into `calls`, completed reasoning items (with
/// encrypted_content) into `reasoning` for stateless resume; the newest
/// response id lands in `new_id`.
fn handle_event(
    bytes: &[u8],
    tx: &UnboundedSender<StreamEvent>,
    calls: &mut Vec<FuncCall>,
    reasoning: &mut Vec<serde_json::Value>,
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
            // Terminal states. Usage feeds the status-bar meter; failures
            // and cutoffs surface as errors instead of a stream that just
            // stops.
            "response.completed" => {
                let usage = v.get("response").and_then(|r| r.get("usage"));
                let input = usage
                    .and_then(|u| u.get("input_tokens"))
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0);
                let output = usage
                    .and_then(|u| u.get("output_tokens"))
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0);
                let total = usage.and_then(|u| u.get("total_tokens")).and_then(|n| n.as_u64());
                if input + output > 0 {
                    let _ = tx.send(StreamEvent::Usage { input, output });
                } else if let Some(t) = total.filter(|t| *t > 0) {
                    let _ = tx.send(StreamEvent::Usage { input: t, output: 0 });
                }
            }
            "response.failed" => {
                let msg = v
                    .get("response")
                    .and_then(|r| r.get("error"))
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("response failed");
                let _ = tx.send(StreamEvent::Error {
                    message: msg.to_string(),
                });
            }
            "response.incomplete" => {
                let reason = v
                    .get("response")
                    .and_then(|r| r.get("incomplete_details"))
                    .and_then(|d| d.get("reason"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("incomplete");
                let _ = tx.send(StreamEvent::Error {
                    message: format!("stopped: {reason}"),
                });
            }
            "response.created" => {
                if let Some(id) = v
                    .get("response")
                    .and_then(|r| r.get("id"))
                    .and_then(|i| i.as_str())
                {
                    *new_id = Some(id.to_string());
                    tracing::Span::current().record("response_id", id);
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
            // Authoritative full arguments. Some servers send few or no
            // deltas and put everything here — without this arm those
            // calls execute with empty arguments.
            "response.function_call_arguments.done" => {
                let item_id = v
                    .get("item_id")
                    .and_then(|i| i.as_str())
                    .or_else(|| v.get("output_item_id").and_then(|i| i.as_str()))
                    .unwrap_or("");
                if let Some(args) = v.get("arguments").and_then(arg_string) {
                    if let Some(c) = calls.iter_mut().find(|c| c.item_id == item_id) {
                        if c.arguments.is_empty() || !args.is_empty() {
                            c.arguments = args;
                        }
                    }
                }
            }
            "response.output_item.done" => {
                let item = v.get("output_item");
                let item_type = item
                    .and_then(|i| i.get("type"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                if item_type == "reasoning" {
                    // Completed reasoning item (summary + encrypted_content):
                    // replayed into the next stateless input so reasoning
                    // context survives with store:false.
                    if let Some(item) = item {
                        reasoning.push(item.clone());
                    }
                } else if item_type == "function_call" {
                    let item_id = item
                        .and_then(|i| i.get("id"))
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();
                    if let Some(arguments) =
                        item.and_then(|i| i.get("arguments")).and_then(arg_string)
                    {
                        if let Some(c) = calls.iter_mut().find(|c| c.item_id == item_id) {
                            if c.arguments.is_empty() || !arguments.is_empty() {
                                c.arguments = arguments;
                            }
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

/// Function-call arguments as a string. Spec says string, but compat
/// servers sometimes emit a JSON object — serialize it rather than
/// dropping the call's arguments on the floor.
fn arg_string(v: &serde_json::Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    if v.is_object() || v.is_array() {
        return serde_json::to_string(v).ok();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ApiToolCall;

    fn channel() -> (
        UnboundedSender<StreamEvent>,
        tokio::sync::mpsc::UnboundedReceiver<StreamEvent>,
    ) {
        tokio::sync::mpsc::unbounded_channel()
    }

    fn api_msg(role: &str) -> ApiMessage {
        ApiMessage {
            role: role.into(),
            content: String::new(),
            images: Vec::new(),
            tool_calls: Vec::new(),
            tool_call_id: String::new(),
            tool_result: String::new(),
        }
    }

    #[test]
    fn tool_history_replays_as_function_items() {
        let mut asst = api_msg("assistant");
        asst.tool_calls.push(ApiToolCall {
            id: "c1".into(),
            name: "zsh".into(),
            arguments: "{\"command\":\"ls\"}".into(),
        });
        let items = json_msg(&asst);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["type"], "function_call");
        assert_eq!(items[0]["call_id"], "c1");

        let mut tool = api_msg("tool");
        tool.tool_call_id = "c1".into();
        tool.tool_result = "out".into();
        let items = json_msg(&tool);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["type"], "function_call_output");
        assert_eq!(items[0]["call_id"], "c1");
        assert_eq!(items[0]["output"], "out");
    }

    #[test]
    fn unpaired_tool_history_never_reaches_wire() {
        use crate::api::ApiToolCall;
        let mut asst = api_msg("assistant");
        asst.tool_calls.push(ApiToolCall {
            id: "".into(),
            name: "zsh".into(),
            arguments: "{}".into(),
        });
        let items = json_msg(&asst);
        assert!(items.iter().all(|i| i.get("type").and_then(|t| t.as_str()) != Some("function_call")));

        let mut tool = api_msg("tool");
        tool.tool_call_id = "".into();
        tool.tool_result = "out".into();
        assert!(json_msg(&tool).is_empty());
    }

    #[test]
    fn completed_event_emits_usage() {
        let (tx, mut rx) = channel();
        let mut calls = Vec::new();
        let mut reasoning = Vec::new();
        let mut id = None;
        handle_event(
            b"data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":345,\"output_tokens\":69,\"total_tokens\":414}}}\n\n",
            &tx,
            &mut calls,
            &mut reasoning,
            &mut id,
        )
        .unwrap();
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, StreamEvent::Usage { input: 345, output: 69 }));
    }

    #[test]
    fn failed_and_incomplete_become_errors() {
        let (tx, mut rx) = channel();
        let mut calls = Vec::new();
        let mut reasoning = Vec::new();
        let mut id = None;
        handle_event(
            b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"boom\"}}}\n\n",
            &tx,
            &mut calls,
            &mut reasoning,
            &mut id,
        )
        .unwrap();
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, StreamEvent::Error { message } if message == "boom"));

        handle_event(
            b"data: {\"type\":\"response.incomplete\",\"response\":{\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n",
            &tx,
            &mut calls,
            &mut reasoning,
            &mut id,
        )
        .unwrap();
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, StreamEvent::Error { message } if message.contains("max_output_tokens")));
    }

    #[test]
    fn arguments_done_event_fills_empty_args() {
        // Servers that send no deltas put everything in
        // function_call_arguments.done — must not execute empty.
        let (tx, _rx) = channel();
        let mut calls = Vec::new();
        let mut reasoning = Vec::new();
        let mut id = None;
        handle_event(
            b"data: {\"type\":\"response.output_item.added\",\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"c1\",\"name\":\"zsh\",\"arguments\":\"\"}}\n\n",
            &tx, &mut calls, &mut reasoning, &mut id,
        )
        .unwrap();
        handle_event(
            b"data: {\"type\":\"response.function_call_arguments.done\",\"item_id\":\"fc_1\",\"arguments\":\"{\\\"command\\\":\\\"ls\\\"}\"}\n\n",
            &tx, &mut calls, &mut reasoning, &mut id,
        )
        .unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments, "{\"command\":\"ls\"}");
    }

    #[test]
    fn object_arguments_serialize_instead_of_dropping() {
        assert_eq!(
            arg_string(&serde_json::json!({"command": "ls"})).as_deref(),
            Some("{\"command\":\"ls\"}")
        );
    }

    #[test]
    fn reasoning_done_item_is_captured() {        let (tx, _rx) = channel();
        let mut calls = Vec::new();
        let mut reasoning = Vec::new();
        let mut id = None;
        handle_event(
            b"data: {\"type\":\"response.output_item.done\",\"output_item\":{\"type\":\"reasoning\",\"id\":\"rs_1\"}}\n\n",
            &tx,
            &mut calls,
            &mut reasoning,
            &mut id,
        )
        .unwrap();
        assert_eq!(reasoning.len(), 1);
        assert_eq!(reasoning[0]["id"], "rs_1");
    }
}
