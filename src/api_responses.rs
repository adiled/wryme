use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};
use futures_util::StreamExt;
use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;

use crate::api::{ApiMessage, Client, StreamEvent, find_event_boundary, truncate};
use crate::book;
use crate::shop::{Shop, WindowMode};
use crate::station::Station;
use crate::tools;

struct FuncCall {
    call_id: String,
    item_id: String,
    name: String,
    rendered: bool,
    arguments: String,
}

#[derive(Default)]
struct Turn {
    calls: Vec<FuncCall>,
    carry: Vec<serde_json::Value>,
    new_id: Option<String>,
    saw_text: bool,
}

fn hosted_tool_name(item_type: &str) -> Option<&'static str> {
    match item_type {
        "file_search_call" => Some("file_search"),
        "web_search_call" => Some("web_search"),
        "code_interpreter_call" => Some("code_interpreter"),
        "image_generation_call" => Some("image_generation"),
        "computer_call" | "computer_use_call" => Some("computer_use"),
        "mcp_list_tools" => Some("mcp_list_tools"),
        "mcp_approval_request" => Some("mcp_approval_request"),
        "multi_agent_call" | "multi_agent_call_output" => Some("multi_agent"),
        "agent_message" => Some("agent_message"),
        _ => None,
    }
}

fn function_call(call_id: &str, name: &str, arguments: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "function_call",
        "call_id": call_id,
        "name": name,
        "arguments": arguments,
    })
}

fn function_call_output(call_id: &str, output: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "function_call_output",
        "call_id": call_id,
        "output": output,
    })
}

fn delivered(due: &[crate::api::DueCall]) -> impl Iterator<Item = serde_json::Value> + '_ {
    due.iter().flat_map(|(call_id, name, args, output)| {
        [
            function_call(call_id, name, args),
            function_call_output(call_id, output),
        ]
    })
}

fn system_note(text: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "message",
        "role": "system",
        "content": [{ "type": "input_text", "text": text }],
    })
}

fn said_before_a_call(item: &serde_json::Value) -> Option<serde_json::Value> {
    let words: String = item
        .get("content")?
        .as_array()?
        .iter()
        .filter_map(|part| part.get("text").or_else(|| part.get("refusal")))
        .filter_map(|t| t.as_str())
        .collect();
    (!words.is_empty())
        .then(|| serde_json::json!({ "type": "message", "role": "assistant", "content": words }))
}

fn trouble(error: &serde_json::Value) -> Option<String> {
    if let Some(text) = error.as_str() {
        return Some(text.to_string());
    }
    if let Some(message) = error.get("message").and_then(|m| m.as_str()) {
        return Some(message.to_string());
    }
    let said: String = error
        .get("content")
        .and_then(|c| c.as_array())
        .into_iter()
        .flatten()
        .filter_map(|part| part.get("text").and_then(|t| t.as_str()))
        .collect();
    if !said.is_empty() {
        return Some(said);
    }
    (!error.is_null()).then(|| error.to_string())
}

fn sealed(item: &serde_json::Value) -> bool {
    item.get("encrypted_content")
        .and_then(|c| c.as_str())
        .is_some_and(|c| !c.is_empty())
}

const MAX_TOOL_CALLS: u32 = 10;
const MULTI_AGENT_MIN: f32 = 5.6;

fn supports_multi_agent(model: &str) -> bool {
    let rest = match model.strip_prefix("gpt-") {
        Some(r) => r,
        None => return false,
    };
    let seg = rest.split('-').next().unwrap_or("");
    let mut num = String::new();
    let mut dot = false;
    for c in seg.chars() {
        if c.is_ascii_digit() {
            num.push(c);
        } else if c == '.' && !dot {
            dot = true;
            num.push(c);
        } else {
            break;
        }
    }
    num.parse::<f32>().is_ok_and(|v| v >= MULTI_AGENT_MIN)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn stream(
    client: &Client,
    shop: &Shop,
    station: &Station,
    messages: Vec<ApiMessage>,
    previous_response_id: Option<String>,
    engine: Arc<Mutex<book::Engine>>,
    tx: &UnboundedSender<StreamEvent>,
    due: &[crate::api::DueCall],
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
            due,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(e) if crate::api::is_tool_unsupported_msg(&format!("{e:#}")) => {
                if crate::api::is_toolless(&shop.name, &station.model) {
                    return Err(e);
                }
                crate::api::mark_toolless(&shop.name, &station.model);
                tracing::warn!(model=%station.model, "tools unsupported, retrying responses without tools");
                let _ = tx.send(StreamEvent::Error {
                    message: "tools unsupported by model, retrying without tools".into(),
                });
                return stream_warm(client, shop, station, messages, None, engine, tx, due).await;
            }
            Err(e) => {
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
                    return stream_full(client, shop, station, messages, engine, tx, due).await;
                }
                return Err(e);
            }
        }
    }
    match stream_full(client, shop, station, messages.clone(), engine.clone(), tx, due).await {
        Ok(()) => Ok(()),
        Err(e) if crate::api::is_tool_unsupported_msg(&format!("{e:#}")) => {
            if crate::api::is_toolless(&shop.name, &station.model) {
                return Err(e);
            }
            crate::api::mark_toolless(&shop.name, &station.model);
            tracing::warn!(model=%station.model, "tools unsupported, retrying responses without tools");
            let _ = tx.send(StreamEvent::Error {
                message: "tools unsupported by model, retrying without tools".into(),
            });
            stream_full(client, shop, station, messages, engine, tx, due).await
        }
        Err(e) => Err(e),
    }
}

#[allow(clippy::too_many_arguments)]
async fn stream_warm(
    client: &Client,
    shop: &Shop,
    station: &Station,
    messages: Vec<ApiMessage>,
    previous_response_id: Option<String>,
    engine: Arc<Mutex<book::Engine>>,
    tx: &UnboundedSender<StreamEvent>,
    due: &[crate::api::DueCall],
) -> Result<()> {
    let system = instructions_of(&messages);
    let instructions = system.as_deref();

    let conv_msgs: Vec<&ApiMessage> = messages.iter().filter(|m| m.role != "system").collect();

    let mut prev_id = previous_response_id;
    let mut input: Vec<serde_json::Value> = if prev_id.is_some() {
        conv_msgs
            .last()
            .filter(|m| m.role == "user")
            .into_iter()
            .flat_map(|m| json_msg(m))
            .collect()
    } else {
        conv_msgs.iter().flat_map(|m| json_msg(m)).collect()
    };

    input.extend(delivered(due));

    let mut bad_rounds: u32 = 0;
    let mut depth: u32 = 0;
    let mut nudged = false;
    let cap = station.dials.tinker_depth.depth_n();
    loop {
        if cap.is_some_and(|c| depth as usize >= c) && !nudged {
            nudged = true;
            tracing::warn!(depth, "tinker depth reached, forcing final answer");
            input.push(system_note(crate::api::FINAL_ANSWER_NUDGE));
        }
        let (calls, new_id, _) = stream_once(
            client,
            shop,
            station,
            &input,
            prev_id.as_deref(),
            true,
            instructions,
            tx,
            nudged,
        )
        .await?;
        let (paired, broken): (Vec<_>, Vec<_>) = calls
            .into_iter()
            .partition(|c| !c.call_id.is_empty() && !c.name.is_empty());
        let mut next_input = Vec::new();
        for c in &broken {
            let output = "error: unusable tool call — every call needs an id and a function name"
                .to_string();
            let _ = tx.send(StreamEvent::ToolResult {
                call_id: c.call_id.clone(),
                name: c.name.clone(),
                arguments: c.arguments.clone(),
                output: output.clone(),
            });
            next_input.push(system_note(&output));
        }
        if paired.is_empty() {
            if broken.is_empty() {
                return Ok(());
            }
            bad_rounds += 1;
            if bad_rounds > 2 {
                return Ok(());
            }
            prev_id = Some(new_id);
            input = next_input;
            continue;
        }
        bad_rounds = 0;
        let calls = paired;
        if nudged {
            tracing::warn!("model kept requesting tools past tinker depth, ending turn");
            return Ok(());
        }
        depth += 1;
        if calls
            .iter()
            .any(|c| !c.rendered && !crate::api::client_can_run(&c.name))
        {
            tracing::info!(shop = %shop.name, "provider runs tools server-side, client does not execute");
            return Ok(());
        }
        for mut call in calls {
            if call.rendered {
                continue;
            }
            if crate::api::owns_tool(&shop.name, &station.model, &call.name) {
                tracing::info!(shop = %shop.name, tool = %call.name, "shop executes tool server-side, client does not run it");
                return Ok(());
            }
            let output = match tools::execute(&engine, &call.name, &call.arguments).await {
                Some(o) => o,
                None => format!("unknown tool '{}'", call.name),
            };
            call.rendered = true;
            let _ = tx.send(StreamEvent::ToolResult {
                call_id: call.call_id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
                output: output.clone(),
            });
            next_input.push(function_call_output(&call.call_id, &output));
        }
        prev_id = Some(new_id);
        input = next_input;
    }
}

async fn stream_full(
    client: &Client,
    shop: &Shop,
    station: &Station,
    messages: Vec<ApiMessage>,
    engine: Arc<Mutex<book::Engine>>,
    tx: &UnboundedSender<StreamEvent>,
    due: &[crate::api::DueCall],
) -> Result<()> {
    let system = instructions_of(&messages);
    let instructions = system.as_deref();

    let conv_msgs: Vec<&ApiMessage> = messages.iter().filter(|m| m.role != "system").collect();

    let mut input: Vec<serde_json::Value> = conv_msgs.iter().flat_map(|m| json_msg(m)).collect();

    input.extend(delivered(due));

    let mut bad_rounds: u32 = 0;
    let mut depth: u32 = 0;
    let mut nudged = false;
    let cap = station.dials.tinker_depth.depth_n();
    loop {
        if cap.is_some_and(|c| depth as usize >= c) && !nudged {
            nudged = true;
            tracing::warn!(depth, "tinker depth reached, forcing final answer");
            input.push(system_note(crate::api::FINAL_ANSWER_NUDGE));
        }
        let (calls, _new_id, carry) = stream_once(
            client,
            shop,
            station,
            &input,
            None,
            false,
            instructions,
            tx,
            nudged,
        )
        .await?;
        let (paired, broken): (Vec<_>, Vec<_>) = calls
            .into_iter()
            .partition(|c| !c.call_id.is_empty() && !c.name.is_empty());
        let mut follow = Vec::new();
        follow.extend(carry);
        for c in &broken {
            let output = "error: unusable tool call — every call needs an id and a function name"
                .to_string();
            let _ = tx.send(StreamEvent::ToolResult {
                call_id: c.call_id.clone(),
                name: c.name.clone(),
                arguments: c.arguments.clone(),
                output: output.clone(),
            });
            follow.push(system_note(&output));
        }
        if paired.is_empty() {
            if broken.is_empty() {
                return Ok(());
            }
            bad_rounds += 1;
            if bad_rounds > 2 {
                return Ok(());
            }
            input.extend(follow);
            continue;
        }
        bad_rounds = 0;
        let calls = paired;
        if nudged {
            tracing::warn!("model kept requesting tools past tinker depth, ending turn");
            return Ok(());
        }
        depth += 1;
        if calls
            .iter()
            .any(|c| !c.rendered && !crate::api::client_can_run(&c.name))
        {
            tracing::info!(shop = %shop.name, "provider runs tools server-side, client does not execute");
            return Ok(());
        }
        for mut call in calls {
            if call.rendered {
                continue;
            }
            if crate::api::owns_tool(&shop.name, &station.model, &call.name) {
                tracing::info!(shop = %shop.name, tool = %call.name, "shop executes tool server-side, client does not run it");
                return Ok(());
            }
            let output = match tools::execute(&engine, &call.name, &call.arguments).await {
                Some(o) => o,
                None => format!("unknown tool '{}'", call.name),
            };
            call.rendered = true;
            let _ = tx.send(StreamEvent::ToolResult {
                call_id: call.call_id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
                output: output.clone(),
            });
            follow.push(function_call(&call.call_id, &call.name, &call.arguments));
            follow.push(function_call_output(&call.call_id, &output));
        }
        input.extend(follow);
    }
}

fn instructions_of(messages: &[ApiMessage]) -> Option<String> {
    let said: Vec<&str> = messages
        .iter()
        .filter(|m| m.role == "system" && !m.content.is_empty())
        .map(|m| m.content.as_str())
        .collect();
    (!said.is_empty()).then(|| said.join("\n\n"))
}

#[derive(Serialize)]
struct ResponsesReq<'a> {
    model: &'a str,
    input: &'a [serde_json::Value],
    stream: bool,
    store: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_response_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<Reasoning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    include: Option<Vec<&'a str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tool_calls: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    multi_agent: Option<MultiAgent>,
    truncation: &'a str,
    tools: &'a [serde_json::Value],
}

#[derive(Serialize)]
struct Reasoning {
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<&'static str>,
}

#[derive(Serialize)]
struct MultiAgent {
    enabled: bool,
    max_concurrent_subagents: u8,
}

#[allow(clippy::too_many_arguments)]
fn request<'a>(
    station: &'a Station,
    input: &'a [serde_json::Value],
    previous_response_id: Option<&'a str>,
    store: bool,
    instructions: Option<&'a str>,
    tools: &'a [serde_json::Value],
    multi: bool,
    tools_off: bool,
) -> ResponsesReq<'a> {
    let reasoning = if multi {
        station.dials.patience.map(|p| Reasoning {
            effort: Some(p.as_wire()),
            summary: None,
        })
    } else {
        (station.dials.patience.is_some() || station.dials.brainy.is_some()).then(|| Reasoning {
            effort: station.dials.patience.map(|p| p.as_wire()),
            summary: station.dials.brainy.and_then(|b| b.as_wire()),
        })
    };
    let include = reasoning
        .as_ref()
        .map(|_| vec!["reasoning.encrypted_content"]);
    ResponsesReq {
        model: &station.model,
        input,
        stream: true,
        store,
        instructions,
        previous_response_id,
        temperature: station.dials.boldness,
        reasoning,
        include,
        max_tool_calls: (!multi && !tools_off).then_some(MAX_TOOL_CALLS),
        tool_choice: tools_off.then_some("none"),
        multi_agent: multi.then_some(MultiAgent {
            enabled: true,
            max_concurrent_subagents: 3,
        }),
        truncation: "auto",
        tools,
    }
}

#[allow(clippy::too_many_arguments)]
async fn stream_once(
    client: &Client,
    shop: &Shop,
    station: &Station,
    input: &[serde_json::Value],
    previous_response_id: Option<&str>,
    store: bool,
    instructions: Option<&str>,
    tx: &UnboundedSender<StreamEvent>,
    tools_off: bool,
) -> Result<(Vec<FuncCall>, String, Vec<serde_json::Value>)> {
    let toolless = crate::api::is_toolless(&shop.name, &station.model);
    let tools = if toolless {
        Vec::new()
    } else {
        let mut tools: Vec<serde_json::Value> =
            crate::api::tools_for(&shop.name, &station.model)
                .iter()
                .map(|t| t.def.clone())
                .collect();
        if crate::api::client_runs_tools() {
            tools.extend(
                tools::tool_defs()
                    .into_iter()
                    .filter(|d| !crate::api::owns_tool(&shop.name, &station.model, d["name"].as_str().unwrap_or_default())),
            );
        } else {
            tools.extend(tools::window_tool_defs().into_iter().filter(|d| {
                !crate::api::owns_tool(
                    &shop.name,
                    &station.model,
                    d["name"].as_str().unwrap_or_default(),
                )
            }));
        }
        tools
    };
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
        if filtered_input.is_empty() {
            input
        } else {
            &filtered_input
        }
    } else {
        input
    };

    let base = shop.url.trim_end_matches('/');
    let url = format!("{}/responses", base);

    let mut attempt_multi =
        supports_multi_agent(&station.model) && !crate::api::is_nomulti(&shop.name, &station.model);
    let resp = loop {
        let body = request(
            station,
            input,
            previous_response_id,
            store,
            instructions,
            &tools,
            attempt_multi,
            tools_off,
        );

        let mut req = client.http.post(&url).json(&body);
        if attempt_multi {
            req = req.header("OpenAI-Beta", "responses_multi_agent=v1");
        }
        if crate::shop::should_send_auth(&shop.key) {
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
        let resp = req
            .send()
            .await
            .map_err(|e| crate::api::unreachable(&shop.name, &url, &e))?;

        if resp.status().is_success() {
            break resp;
        }
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        tracing::warn!(url = %url, %status, body = %truncate(&body, 2000), "responses upstream error");
        let msg = format!("upstream {}: {}", status, truncate(&body, 800));
        if attempt_multi && msg.to_lowercase().contains("multi_agent") {
            tracing::warn!(shop = %shop.name, "multi_agent unsupported, retrying without it");
            crate::api::mark_nomulti(&shop.name, &station.model);
            attempt_multi = false;
            continue;
        }
        return Err(anyhow!(msg));
    };

    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::with_capacity(8 * 1024);
    let mut turn = Turn::default();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("reading sse chunk")?;
        buf.extend_from_slice(&chunk);
        while let Some(end) = find_event_boundary(&buf) {
            let event_bytes = buf.drain(..end.end).collect::<Vec<u8>>();
            let event = &event_bytes[..end.body_len];
            handle_event(event, tx, &mut turn)?;
        }
    }
    if !buf.is_empty() {
        handle_event(&buf, tx, &mut turn)?;
    }

    let new_id = turn.new_id.context("no response.created seen")?;
    Ok((turn.calls, new_id, turn.carry))
}

fn json_msg(m: &ApiMessage) -> Vec<serde_json::Value> {
    if m.role == "tool" {
        if m.tool_call_id.is_empty() {
            return vec![];
        }
        return vec![function_call_output(&m.tool_call_id, &m.tool_result)];
    }
    if !m.tool_calls.is_empty() {
        let mut items: Vec<serde_json::Value> = Vec::new();
        if !m.content.is_empty() {
            items.push(
                serde_json::json!({ "type": "message", "role": m.role, "content": m.content }),
            );
        }
        for c in m
            .tool_calls
            .iter()
            .filter(|c| !c.id.is_empty() && !c.name.is_empty())
        {
            items.push(function_call(&c.id, &c.name, &c.arguments));
        }
        return items;
    }
    if m.images.is_empty() {
        return vec![
            serde_json::json!({ "type": "message", "role": m.role, "content": m.content }),
        ];
    }
    let mut parts: Vec<serde_json::Value> = Vec::new();
    if !m.content.is_empty() {
        parts.push(serde_json::json!({
            "type": "input_text",
            "text": m.content,
        }));
    }
    for path in &m.images {
        if let Some((mime, b64)) = crate::api::image_data_url(path) {
            parts.push(serde_json::json!({
                "type": "input_image",
                "image_url": format!("data:{mime};base64,{b64}"),
                "detail": "auto",
            }));
        }
    }
    if parts.is_empty() {
        return Vec::new();
    }
    vec![serde_json::json!({
        "type": "message",
        "role": m.role,
        "content": parts,
    })]
}
fn handle_event(
    bytes: &[u8],
    tx: &UnboundedSender<StreamEvent>,
    turn: &mut Turn,
) -> Result<()> {
    let Turn {
        calls,
        carry,
        new_id,
        saw_text,
    } = turn;
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
                let total = usage
                    .and_then(|u| u.get("total_tokens"))
                    .and_then(|n| n.as_u64());
                if input + output > 0 {
                    let _ = tx.send(StreamEvent::Usage { input, output });
                } else if let Some(t) = total.filter(|t| *t > 0) {
                    let _ = tx.send(StreamEvent::Usage {
                        input: t,
                        output: 0,
                    });
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
            "error" => {
                let msg = v
                    .get("message")
                    .and_then(|m| m.as_str())
                    .or_else(|| {
                        v.get("error")
                            .and_then(|e| e.get("message"))
                            .and_then(|m| m.as_str())
                    })
                    .unwrap_or("stream error");
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
                    let _ = tx.send(StreamEvent::ResponseId { id: id.to_string() });
                }
            }
            "response.output_text.delta" => {
                *saw_text = true;
                if let Some(d) = v.get("delta").and_then(|d| d.as_str())
                    && !d.is_empty()
                {
                    let _ = tx.send(StreamEvent::Delta {
                        text: d.to_string(),
                    });
                }
            }
            "response.output_text.done" => {
                if !*saw_text
                    && let Some(t) = v.get("text").and_then(|t| t.as_str())
                    && !t.is_empty()
                {
                    let _ = tx.send(StreamEvent::Delta {
                        text: t.to_string(),
                    });
                }
            }
            "response.refusal.delta" => {
                if let Some(d) = v.get("delta").and_then(|d| d.as_str())
                    && !d.is_empty()
                {
                    let _ = tx.send(StreamEvent::Delta {
                        text: d.to_string(),
                    });
                }
            }
            "response.reasoning_summary_text.delta" => {
                if let Some(d) = v.get("delta").and_then(|d| d.as_str())
                    && !d.is_empty()
                {
                    let _ = tx.send(StreamEvent::Brain {
                        text: d.to_string(),
                    });
                }
            }
            "response.reasoning_text.delta" | "response.reasoning.delta" => {
                if let Some(d) = v.get("delta").and_then(|d| d.as_str())
                    && !d.is_empty()
                {
                    let _ = tx.send(StreamEvent::Heart {
                        text: d.to_string(),
                    });
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
                        rendered: false,
                    });
                } else {
                    let name: Option<String> = if item_type == "mcp_call" {
                        item.and_then(|i| i.get("name"))
                            .and_then(|n| n.as_str())
                            .map(|n| n.to_string())
                    } else {
                        hosted_tool_name(item_type).map(|n| n.to_string())
                    };
                    if name.is_some() {
                        let _ = tx.send(StreamEvent::ToolCall { name });
                    }
                }
            }
            "response.function_call_arguments.delta" => {
                let item_id = v
                    .get("item_id")
                    .and_then(|i| i.as_str())
                    .or_else(|| v.get("output_item_id").and_then(|i| i.as_str()))
                    .unwrap_or("");
                if let Some(d) = v.get("delta").and_then(|d| d.as_str())
                    && let Some(c) = calls.iter_mut().find(|c| c.item_id == item_id)
                {
                    c.arguments.push_str(d);
                }
            }
            "response.function_call_arguments.done" => {
                let item_id = v
                    .get("item_id")
                    .and_then(|i| i.as_str())
                    .or_else(|| v.get("output_item_id").and_then(|i| i.as_str()))
                    .unwrap_or("");
                if let Some(args) = v.get("arguments").and_then(arg_string)
                    && let Some(c) = calls.iter_mut().find(|c| c.item_id == item_id)
                    && (c.arguments.is_empty() || !args.is_empty())
                {
                    c.arguments = args;
                }
            }
            "response.output_item.done" => {
                let item = v.get("item");
                let item_type = item
                    .and_then(|i| i.get("type"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                match item_type {
                    "reasoning" => {
                        if let Some(item) = item.filter(|i| sealed(i)) {
                            carry.push(item.clone());
                        }
                    }
                    "message" => {
                        if let Some(words) = item.and_then(said_before_a_call) {
                            carry.push(words);
                        }
                    }
                    "function_call" => {
                        let item_id = item
                            .and_then(|i| i.get("id"))
                            .and_then(|i| i.as_str())
                            .unwrap_or("")
                            .to_string();
                        if let Some(arguments) =
                            item.and_then(|i| i.get("arguments")).and_then(arg_string)
                            && let Some(c) = calls.iter_mut().find(|c| c.item_id == item_id)
                            && (c.arguments.is_empty() || !arguments.is_empty())
                        {
                            c.arguments = arguments;
                        }
                        if let Some(item) = item
                            && let Some(output) = item.get("output").and_then(arg_string)
                            && !output.is_empty()
                            && !calls
                                .iter()
                                .any(|c| c.item_id == item_id && c.rendered)
                        {
                            let name = item
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("tool")
                                .to_string();
                            let call_id = item
                                .get("call_id")
                                .or_else(|| item.get("id"))
                                .and_then(|i| i.as_str())
                                .unwrap_or("")
                                .to_string();
                            let arguments = item
                                .get("arguments")
                                .and_then(arg_string)
                                .unwrap_or_default();
                            if let Some(c) = calls.iter_mut().find(|c| c.item_id == item_id) {
                                c.rendered = true;
                            }
                            carry.push(item.clone());
                            let _ = tx.send(StreamEvent::ToolResult {
                                call_id,
                                name,
                                arguments,
                                output,
                            });
                        }
                    }
                    "mcp_call" => {
                        if let Some(item) = item {
                            carry.push(item.clone());
                            let name = item
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("mcp")
                                .to_string();
                            let call_id = item
                                .get("id")
                                .and_then(|i| i.as_str())
                                .unwrap_or("")
                                .to_string();
                            let arguments = item
                                .get("arguments")
                                .and_then(arg_string)
                                .unwrap_or_default();
                            if let Some(output) = item
                                .get("output")
                                .and_then(arg_string)
                                .filter(|o| !o.is_empty())
                                .or_else(|| item.get("error").and_then(trouble))
                                .filter(|o| !o.is_empty())
                            {
                                let _ = tx.send(StreamEvent::ToolResult {
                                    call_id,
                                    name,
                                    arguments,
                                    output,
                                });
                            }
                        }
                    }
                    _ => {
                        if hosted_tool_name(item_type).is_some()
                            && let Some(item) = item
                        {
                            carry.push(item.clone());
                        }
                    }
                }
            }
            "response.file_search_call.in_progress"
            | "response.file_search_call.searching"
            | "response.web_search_call.in_progress"
            | "response.web_search_call.searching"
            | "response.code_interpreter_call.in_progress"
            | "response.code_interpreter_call.interpreting"
            | "response.code_interpreter_call_code.delta"
            | "response.image_generation_call.generating"
            | "response.image_generation_call.partial_image"
            | "response.mcp_list_tools.in_progress"
            | "response.mcp_call.in_progress"
            | "response.mcp_call_arguments.delta"
            | "response.mcp_call_arguments.done" => {
                let _ = tx.send(StreamEvent::ToolCall { name: None });
            }
            "response.multi_agent_call.in_progress" | "response.agent_message.in_progress" => {
                let _ = tx.send(StreamEvent::ToolCall {
                    name: Some("multi_agent".to_string()),
                });
            }
            "response.mcp_call.failed" => {
                let _ = tx.send(StreamEvent::Error {
                    message: "mcp call failed".to_string(),
                });
            }
            _ => {}
        }
    }
    Ok(())
}

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
    fn every_system_message_is_said_once_as_instructions() {
        let msg = |role: &str, content: &str| ApiMessage {
            content: content.into(),
            ..api_msg(role)
        };
        assert_eq!(instructions_of(&[msg("user", "hi")]), None);
        let page = "# garden\nLast inked: 2026-10-28 20:31\n";
        assert_eq!(
            instructions_of(&[msg("system", page), msg("user", "hi")]).as_deref(),
            Some(page),
            "a page with no station prompt is not doubled"
        );
        assert_eq!(
            instructions_of(&[
                msg("system", "be brief"),
                msg("system", page),
                msg("user", "hi")
            ])
            .as_deref(),
            Some(format!("be brief\n\n{page}").as_str())
        );
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
        assert!(
            items
                .iter()
                .all(|i| i.get("type").and_then(|t| t.as_str()) != Some("function_call"))
        );

        let mut tool = api_msg("tool");
        tool.tool_call_id = "".into();
        tool.tool_result = "out".into();
        assert!(json_msg(&tool).is_empty());
    }

    fn tmp_file(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("wryme-test-{}-{}", std::process::id(), name));
        std::fs::write(&p, b"\x89PNG\r\n\x1a\n").unwrap();
        p
    }

    #[test]
    fn image_parts_nest_inside_the_message_content() {
        let img = tmp_file("nest.png");
        let mut m = api_msg("user");
        m.content = "what is this?".into();
        m.images = vec![img.to_string_lossy().into_owned()];

        let items = json_msg(&m);
        let _ = std::fs::remove_file(&img);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["type"], "message");
        assert_eq!(items[0]["role"], "user");

        let parts = items[0]["content"].as_array().expect("content is an array");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["type"], "input_text");
        assert_eq!(parts[0]["text"], "what is this?");
        assert_eq!(parts[1]["type"], "input_image");
        assert!(
            parts[1]["image_url"]
                .as_str()
                .expect("image_url is a bare string")
                .starts_with("data:image/png;base64,")
        );

        assert!(
            items
                .iter()
                .all(|i| i.get("type").and_then(|t| t.as_str()) != Some("input_image"))
        );
    }

    #[test]
    fn image_only_message_emits_the_image_part_alone() {
        let img = tmp_file("only.png");
        let mut m = api_msg("user");
        m.content = String::new();
        m.images = vec![img.to_string_lossy().into_owned()];

        let items = json_msg(&m);
        let _ = std::fs::remove_file(&img);

        assert_eq!(items.len(), 1);
        let parts = items[0]["content"].as_array().expect("content is an array");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["type"], "input_image");
    }

    #[test]
    fn unreadable_image_never_yields_an_empty_content_array() {
        let mut m = api_msg("user");
        m.content = "hi".into();
        m.images = vec!["/nonexistent/definitely-not-here.png".into()];

        let items = json_msg(&m);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["type"], "message");
        let parts = items[0]["content"].as_array().expect("content is an array");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["type"], "input_text");
        assert_eq!(parts[0]["text"], "hi");
    }

    #[test]
    fn object_arguments_serialize_instead_of_dropping() {
        assert_eq!(
            arg_string(&serde_json::json!({"command": "ls"})).as_deref(),
            Some("{\"command\":\"ls\"}")
        );
    }

    #[test]
    fn multi_agent_gates_on_gpt_version() {
        assert!(supports_multi_agent("gpt-5.6-mini"));
        assert!(supports_multi_agent("gpt-6.1-sol"));
        assert!(supports_multi_agent("gpt-6-astra"));
        assert!(!supports_multi_agent("gpt-5.4"));
        assert!(!supports_multi_agent("gpt-4o-mini"));
        assert!(!supports_multi_agent("gpt-4o"));
        assert!(!supports_multi_agent("llama-3.3-70b"));
        assert!(!supports_multi_agent("o1-mini"));
    }

    use crate::spec::{self, BOTH, OPEN_RESPONSES, OPENAI, Spec};
    use serde_json::{Value, json};

    fn event(ty: &str, fields: Value) -> Value {
        let mut ev = json!({ "type": ty, "sequence_number": 1 });
        ev.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        ev
    }

    fn response(fields: Value) -> Value {
        let mut r = json!({
            "id": "resp_1",
            "object": "response",
            "created_at": 1,
            "completed_at": null,
            "status": "completed",
            "incomplete_details": null,
            "model": "m",
            "previous_response_id": null,
            "instructions": null,
            "output": [],
            "error": null,
            "tools": [],
            "tool_choice": "auto",
            "truncation": "disabled",
            "parallel_tool_calls": true,
            "text": { "format": { "type": "text" } },
            "top_p": 1,
            "presence_penalty": 0,
            "frequency_penalty": 0,
            "top_logprobs": 0,
            "temperature": 1,
            "reasoning": null,
            "usage": null,
            "max_output_tokens": null,
            "max_tool_calls": null,
            "store": false,
            "background": false,
            "service_tier": "default",
            "metadata": {},
            "safety_identifier": null,
            "prompt_cache_key": null,
            "access_programs": null,
        });
        r.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        r
    }

    fn usage(input: u64, output: u64) -> Value {
        json!({
            "input_tokens": input,
            "input_tokens_details": { "cached_tokens": 0, "cache_write_tokens": 0 },
            "output_tokens": output,
            "output_tokens_details": { "reasoning_tokens": 0 },
            "total_tokens": input + output,
        })
    }

    fn function_call_item(status: &str, arguments: &str) -> Value {
        json!({ "type": "function_call", "id": "fc_1", "call_id": "c1", "name": "zsh", "arguments": arguments, "status": status })
    }

    fn message_item(text: &str) -> Value {
        json!({
            "type": "message",
            "id": "msg_1",
            "status": "completed",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": text, "annotations": [], "logprobs": [] }],
        })
    }

    fn reasoning_item(sealed: Option<&str>) -> Value {
        let mut item = json!({ "type": "reasoning", "id": "rs_1", "summary": [] });
        if let Some(sealed) = sealed {
            item["encrypted_content"] = json!(sealed);
        }
        item
    }

    fn mcp_call_item(status: &str, output: Option<&str>, error: Option<Value>) -> Value {
        json!({
            "type": "mcp_call",
            "id": "mcp_1",
            "server_label": "hum",
            "name": "zsh",
            "arguments": "{\"command\":\"ls\"}",
            "status": status,
            "output": output,
            "error": error,
        })
    }

    fn done(item: Value) -> Value {
        event(
            "response.output_item.done",
            json!({ "output_index": 0, "item": item }),
        )
    }

    fn added(item: Value) -> Value {
        event(
            "response.output_item.added",
            json!({ "output_index": 0, "item": item }),
        )
    }

    struct Heard {
        tx: UnboundedSender<StreamEvent>,
        rx: tokio::sync::mpsc::UnboundedReceiver<StreamEvent>,
        turn: Turn,
    }

    impl Heard {
        fn new() -> Self {
            let (tx, rx) = channel();
            Self {
                tx,
                rx,
                turn: Turn::default(),
            }
        }

        fn hear(&mut self, specs: &[Spec], ev: Value) -> &mut Self {
            spec::streamed(specs, &ev);
            self.beyond_spec(ev)
        }

        fn beyond_spec(&mut self, ev: Value) -> &mut Self {
            handle_event(
                format!("data: {ev}\n\n").as_bytes(),
                &self.tx,
                &mut self.turn,
            )
            .unwrap();
            self
        }

        fn next(&mut self) -> Option<StreamEvent> {
            self.rx.try_recv().ok()
        }
    }

    #[test]
    fn completed_event_emits_usage() {
        let mut h = Heard::new();
        h.hear(
            BOTH,
            event(
                "response.completed",
                json!({ "response": response(json!({ "usage": usage(345, 69) })) }),
            ),
        );
        assert!(matches!(
            h.next(),
            Some(StreamEvent::Usage {
                input: 345,
                output: 69
            })
        ));
    }

    #[test]
    fn failed_and_incomplete_become_errors() {
        let mut h = Heard::new();
        h.hear(
            BOTH,
            event(
                "response.failed",
                json!({ "response": response(json!({ "status": "failed", "error": { "code": "server_error", "message": "boom" } })) }),
            ),
        );
        assert!(matches!(h.next(), Some(StreamEvent::Error { message }) if message == "boom"));
        h.hear(
            BOTH,
            event(
                "response.incomplete",
                json!({ "response": response(json!({ "status": "incomplete", "incomplete_details": { "reason": "max_output_tokens" } })) }),
            ),
        );
        assert!(
            matches!(h.next(), Some(StreamEvent::Error { message }) if message.contains("max_output_tokens"))
        );
    }

    #[test]
    fn an_error_event_is_read_in_either_specs_shape() {
        let mut h = Heard::new();
        h.hear(
            OPENAI,
            event(
                "error",
                json!({ "code": "server_error", "message": "boom", "param": null }),
            ),
        );
        assert!(matches!(h.next(), Some(StreamEvent::Error { message }) if message == "boom"));
        h.hear(OPEN_RESPONSES, event("error", json!({ "error": { "type": "server_error", "code": "server_error", "message": "boom too", "param": null } })));
        assert!(matches!(h.next(), Some(StreamEvent::Error { message }) if message == "boom too"));
    }

    #[test]
    fn a_created_response_names_the_turn() {
        let mut h = Heard::new();
        h.hear(
            BOTH,
            event(
                "response.created",
                json!({ "response": response(json!({ "status": "in_progress" })) }),
            ),
        );
        assert_eq!(h.turn.new_id.as_deref(), Some("resp_1"));
    }

    #[test]
    fn arguments_done_event_fills_empty_args() {
        let mut h = Heard::new();
        h.hear(BOTH, added(function_call_item("in_progress", "")));
        h.hear(
            BOTH,
            event(
                "response.function_call_arguments.done",
                json!({ "item_id": "fc_1", "output_index": 0, "name": "zsh", "arguments": "{\"command\":\"ls\"}" }),
            ),
        );
        assert_eq!(h.turn.calls.len(), 1);
        assert_eq!(h.turn.calls[0].arguments, "{\"command\":\"ls\"}");
    }

    #[test]
    fn arguments_stream_in_by_their_item_id() {
        let mut h = Heard::new();
        h.hear(BOTH, added(function_call_item("in_progress", "")));
        for delta in ["{\"cmd\"", ":1}"] {
            h.hear(
                BOTH,
                event(
                    "response.function_call_arguments.delta",
                    json!({ "item_id": "fc_1", "output_index": 0, "delta": delta }),
                ),
            );
        }
        assert_eq!(h.turn.calls[0].arguments, "{\"cmd\":1}");
    }

    #[test]
    fn the_words_said_before_a_call_are_carried_into_the_next_round() {
        let mut h = Heard::new();
        h.hear(BOTH, done(message_item("I'll look.")));
        assert_eq!(
            h.turn.carry,
            vec![json!({ "type": "message", "role": "assistant", "content": "I'll look." })]
        );
        spec::conforms(OPENAI, "InputItem", &h.turn.carry[0]);
        spec::conforms(OPEN_RESPONSES, "ItemParam", &h.turn.carry[0]);
    }

    #[test]
    fn reasoning_is_carried_only_sealed_since_the_full_window_stores_nothing() {
        let mut h = Heard::new();
        h.hear(BOTH, done(reasoning_item(None)));
        assert!(h.turn.carry.is_empty());
        h.hear(BOTH, done(reasoning_item(Some("gAAAA"))));
        assert_eq!(h.turn.carry.len(), 1);
        spec::conforms(OPENAI, "InputItem", &h.turn.carry[0]);
        spec::conforms(OPEN_RESPONSES, "ItemParam", &h.turn.carry[0]);
    }

    #[test]
    fn summary_deltas_are_brain_and_raw_reasoning_is_heart_in_either_specs_name() {
        let mut h = Heard::new();
        h.hear(
            BOTH,
            event(
                "response.reasoning_summary_text.delta",
                json!({ "item_id": "rs_1", "output_index": 0, "summary_index": 0, "delta": "weighing it" }),
            ),
        );
        h.hear(
            OPENAI,
            event(
                "response.reasoning_text.delta",
                json!({ "item_id": "rs_1", "output_index": 0, "content_index": 0, "delta": "so step one" }),
            ),
        );
        h.hear(
            OPEN_RESPONSES,
            event(
                "response.reasoning.delta",
                json!({ "item_id": "rs_1", "output_index": 0, "content_index": 0, "delta": "and two" }),
            ),
        );
        assert!(matches!(h.next(), Some(StreamEvent::Brain { text }) if text == "weighing it"));
        assert!(matches!(h.next(), Some(StreamEvent::Heart { text }) if text == "so step one"));
        assert!(matches!(h.next(), Some(StreamEvent::Heart { text }) if text == "and two"));
        assert!(h.next().is_none());
    }

    #[test]
    fn mcp_call_surfaces_tool_and_result_and_carries() {
        let mut h = Heard::new();
        h.hear(OPENAI, added(mcp_call_item("in_progress", None, None)));
        assert!(
            matches!(h.next(), Some(StreamEvent::ToolCall { name }) if name.as_deref() == Some("zsh"))
        );
        assert!(
            h.turn.calls.is_empty(),
            "mcp_call is not a local function call"
        );
        h.hear(
            OPENAI,
            event(
                "response.mcp_call_arguments.delta",
                json!({ "item_id": "mcp_1", "output_index": 0, "delta": "{" }),
            ),
        );
        assert!(matches!(
            h.next(),
            Some(StreamEvent::ToolCall { name: None })
        ));
        h.hear(
            OPENAI,
            done(mcp_call_item("completed", Some("README.md"), None)),
        );
        assert!(
            matches!(h.next(), Some(StreamEvent::ToolResult { call_id, name, arguments, output }) if call_id == "mcp_1" && name == "zsh" && arguments == "{\"command\":\"ls\"}" && output == "README.md")
        );
        assert_eq!(
            h.turn.carry.len(),
            1,
            "done mcp_call item is carried for store:false continuity"
        );
        spec::conforms(OPENAI, "InputItem", &h.turn.carry[0]);
    }

    #[test]
    fn a_failed_mcp_call_shows_its_error() {
        let mut h = Heard::new();
        h.hear(
            OPENAI,
            event(
                "response.mcp_call.failed",
                json!({ "item_id": "mcp_1", "output_index": 0 }),
            ),
        );
        assert!(
            matches!(h.next(), Some(StreamEvent::Error { message }) if message == "mcp call failed")
        );
        let protocol =
            json!({ "type": "mcp_protocol_error", "code": -32000, "message": "no such command" });
        h.hear(OPENAI, done(mcp_call_item("failed", None, Some(protocol))));
        assert!(
            matches!(h.next(), Some(StreamEvent::ToolResult { output, .. }) if output == "no such command")
        );
        let ran = json!({ "type": "mcp_tool_execution_error", "content": [{ "type": "text", "text": "exit 127" }] });
        h.hear(OPENAI, done(mcp_call_item("failed", None, Some(ran))));
        assert!(
            matches!(h.next(), Some(StreamEvent::ToolResult { output, .. }) if output == "exit 127")
        );
    }

    #[test]
    fn a_provider_that_runs_a_function_call_itself_shows_its_result_once() {
        let mut h = Heard::new();
        h.hear(
            BOTH,
            added(function_call_item("in_progress", "{\"command\":\"ls\"}")),
        );
        assert!(
            matches!(h.next(), Some(StreamEvent::ToolCall { name }) if name.as_deref() == Some("zsh"))
        );
        let mut ran = function_call_item("completed", "{\"command\":\"ls\"}");
        ran["output"] = json!("README.md");
        h.beyond_spec(done(ran.clone()));
        assert!(
            matches!(h.next(), Some(StreamEvent::ToolResult { call_id, name, arguments, output }) if call_id == "c1" && name == "zsh" && arguments == "{\"command\":\"ls\"}" && output == "README.md")
        );
        assert_eq!(h.turn.carry.len(), 1);
        assert!(
            h.turn.calls[0].rendered,
            "a call the provider ran is not run again here"
        );
        h.beyond_spec(done(ran));
        assert!(h.next().is_none(), "its result shows once");
    }

    #[test]
    fn hosted_call_items_carry_forward_but_never_become_local_calls() {
        let mut h = Heard::new();
        h.hear(
            OPENAI,
            done(json!({ "type": "web_search_call", "id": "ws_1", "status": "completed", "action": { "type": "search", "query": "bees" } })),
        );
        h.hear(OPENAI, done(json!({ "type": "mcp_list_tools", "id": "ml_1", "server_label": "hum", "tools": [] })));
        assert_eq!(h.turn.carry.len(), 2);
        assert!(h.turn.calls.is_empty());
        for carried in &h.turn.carry {
            spec::conforms(OPENAI, "InputItem", carried);
        }
    }

    #[test]
    fn multi_agent_items_beyond_the_spec_signal_communing_and_carry_silently() {
        let mut h = Heard::new();
        h.beyond_spec(added(json!({ "id": "mac_1", "type": "multi_agent_call", "call_id": "c1", "action": "spawn_agent" })));
        assert!(
            matches!(h.next(), Some(StreamEvent::ToolCall { name }) if name.as_deref() == Some("multi_agent"))
        );
        assert!(h.turn.calls.is_empty());
        h.beyond_spec(done(json!({ "id": "mac_1", "type": "multi_agent_call", "call_id": "c1", "action": "spawn_agent" })));
        h.beyond_spec(done(json!({ "id": "amsg_1", "type": "agent_message", "author": "/root/a", "recipient": "/root" })));
        assert_eq!(h.turn.carry.len(), 2);
        assert!(h.turn.calls.is_empty());
    }

    #[test]
    fn refusal_and_done_without_deltas_surface_as_text() {
        let mut h = Heard::new();
        h.hear(
            BOTH,
            event("response.refusal.delta", json!({ "item_id": "msg_1", "output_index": 0, "content_index": 0, "delta": "i decline" })),
        );
        assert!(matches!(h.next(), Some(StreamEvent::Delta { text }) if text == "i decline"));
        h.hear(
            BOTH,
            event(
                "response.output_text.done",
                json!({ "item_id": "t_1", "output_index": 0, "content_index": 0, "text": "fallback text", "logprobs": [] }),
            ),
        );
        assert!(matches!(h.next(), Some(StreamEvent::Delta { text }) if text == "fallback text"));
        h.hear(
            BOTH,
            event(
                "response.output_text.delta",
                json!({ "item_id": "t_2", "output_index": 0, "content_index": 0, "delta": "streamed", "logprobs": [] }),
            ),
        );
        h.hear(
            BOTH,
            event(
                "response.output_text.done",
                json!({ "item_id": "t_2", "output_index": 0, "content_index": 0, "text": "streamed", "logprobs": [] }),
            ),
        );
        assert!(matches!(h.next(), Some(StreamEvent::Delta { text }) if text == "streamed"));
        assert!(
            h.next().is_none(),
            "done must not double-emit text after deltas"
        );
    }

    fn station(dials: crate::station::Dials) -> Station {
        Station {
            name: "s".into(),
            model: "gpt-5.4".into(),
            dials,
            voice: None,
        }
    }

    fn dialed() -> crate::station::Dials {
        crate::station::Dials {
            boldness: Some(0.7),
            patience: Some(crate::station::Patience::Steady),
            brainy: Some(crate::station::Brainy::Gabby),
            ..Default::default()
        }
    }

    fn history() -> Vec<ApiMessage> {
        let mut user = api_msg("user");
        user.content = "read the readme".into();
        let mut asked = api_msg("assistant");
        asked.content = "I'll look.".into();
        asked.tool_calls.push(ApiToolCall {
            id: "c1".into(),
            name: "zsh".into(),
            arguments: "{\"command\":\"cat README.md\"}".into(),
        });
        let mut answer = api_msg("tool");
        answer.tool_call_id = "c1".into();
        answer.tool_result = "# wryme".into();
        let mut said = api_msg("assistant");
        said.content = "It is wryme's readme.".into();
        let mut again = api_msg("user");
        again.content = "and now?".into();
        vec![user, asked, answer, said, again]
    }

    fn due() -> Vec<crate::api::DueCall> {
        vec![(
            "check_17".into(),
            "zsh_check".into(),
            "{\"id\":17}".into(),
            "job 17 finished".into(),
        )]
    }

    fn sent(body: &ResponsesReq) -> Value {
        let body = serde_json::to_value(body).unwrap();
        spec::conforms(OPENAI, "CreateResponse", &body);
        spec::conforms(OPEN_RESPONSES, "CreateResponseBody", &body);
        body
    }

    #[test]
    fn a_full_window_round_asks_as_the_spec_says() {
        let s = station(dialed());
        let tools = tools::tool_defs();
        let mut input: Vec<Value> = history().iter().flat_map(json_msg).collect();
        input.extend(delivered(&due()));
        input.extend(said_before_a_call(&message_item("Checking.")));
        input.push(reasoning_item(Some("gAAAA")));
        input.push(function_call("c2", "zsh", "{\"command\":\"ls\"}"));
        input.push(function_call_output("c2", "README.md"));
        input.push(system_note(crate::api::FINAL_ANSWER_NUDGE));
        let body = sent(&request(
            &s,
            &input,
            None,
            false,
            Some("be brief"),
            &tools,
            false,
            true,
        ));
        assert_eq!(body["store"], false);
        assert_eq!(
            body["tool_choice"], "none",
            "a nudged round forbids calls the way the spec does"
        );
        assert!(body.get("max_tool_calls").is_none());
        assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    }

    #[test]
    fn a_warm_window_round_asks_as_the_spec_says() {
        let s = station(crate::station::Dials::default());
        let tools = tools::window_tool_defs();
        let mut input: Vec<Value> = history().last().into_iter().flat_map(json_msg).collect();
        input.extend(delivered(&due()));
        input.push(function_call_output("c2", "README.md"));
        let body = sent(&request(
            &s,
            &input,
            Some("resp_1"),
            true,
            None,
            &tools,
            false,
            false,
        ));
        assert_eq!(body["previous_response_id"], "resp_1");
        assert!(body.get("reasoning").is_none());
    }

    #[test]
    fn a_hushed_station_asks_for_no_summary() {
        let dials = crate::station::Dials {
            brainy: Some(crate::station::Brainy::Hush),
            ..Default::default()
        };
        let s = station(dials);
        let body = sent(&request(&s, &[], None, true, None, &[], false, false));
        assert!(body["reasoning"].get("summary").is_none());
    }

    #[test]
    fn every_patience_is_an_effort_openai_takes() {
        use crate::station::Patience::*;
        for patience in [Bare, Swift, Quick, Steady, Slow, Deep, Max] {
            let s = station(crate::station::Dials {
                patience: Some(patience),
                ..Default::default()
            });
            let body = serde_json::to_value(request(&s, &[], None, true, None, &[], false, false))
                .unwrap();
            spec::conforms(OPENAI, "CreateResponse", &body);
        }
    }
}
