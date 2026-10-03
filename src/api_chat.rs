use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

use crate::api::{ApiMessage, Client, StreamEvent, find_event_boundary, truncate};
use crate::book;
use crate::shop::Shop;
use crate::station::Station;
use crate::tools;

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
    let mut conv: Vec<serde_json::Value> = messages.iter().filter_map(json_msg).collect();

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
    let mut rounds: u32 = 0;
    let mut nudged = false;
    loop {
        if rounds >= crate::api::MAX_TOOL_ROUNDS && !nudged {
            nudged = true;
            tracing::warn!(rounds, "tool round cap reached, forcing final answer");
            conv.push(serde_json::json!({
                "role": "system",
                "content": crate::api::FINAL_ANSWER_NUDGE,
            }));
        }
        let (calls, assistant_content) = match stream_once(client, shop, station, &conv, nudged, tx)
            .await
        {
            Ok(v) => v,
            Err(e) if crate::api::is_tool_unsupported_msg(&format!("{e:#}")) => {
                if crate::api::is_toolless(&station.model) {
                    return Err(e);
                }
                crate::api::mark_toolless(&station.model);
                tracing::warn!(model=%station.model, "tools unsupported, retrying without tools and marking toolless");
                let _ = tx.send(StreamEvent::Error {
                    message: "tools unsupported by model, retrying without tools".into(),
                });
                match stream_once(client, shop, station, &conv, nudged, tx).await {
                    Ok(v) => v,
                    Err(e2) => return Err(e2),
                }
            }
            Err(e) => return Err(e),
        };
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
            let output = "error: unusable tool call — every call needs an id and a function name"
                .to_string();
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
            if broken.is_empty() {
                return Ok(());
            }
            bad_rounds += 1;
            if bad_rounds > 2 {
                return Ok(());
            }
            continue;
        }
        bad_rounds = 0;
        let calls = paired;
        if nudged {
            tracing::warn!("model kept requesting tools after the round cap, ending turn");
            return Ok(());
        }
        rounds += 1;

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
    }
}

fn tool_policy(model_toolless: bool, tools_off: bool) -> (bool, bool) {
    let offer_tools = !model_toolless && !tools_off;
    let filter_transcript = model_toolless;
    (offer_tools, filter_transcript)
}

async fn stream_once(
    client: &Client,
    shop: &Shop,
    station: &Station,
    conv: &[serde_json::Value],
    tools_off: bool,
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
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<&'a str>,
        tool_choice: &'a str,
        tools: &'a [serde_json::Value],
    }

    #[derive(Serialize)]
    struct StreamOptions {
        include_usage: bool,
    }

    let model_toolless = crate::api::is_toolless(&station.model);
    let (offer_tools, filter_transcript) = tool_policy(model_toolless, tools_off);
    let tools = if offer_tools {
        tools::tool_defs_chat()
    } else {
        Vec::new()
    };
    let tool_choice = if offer_tools { "auto" } else { "none" };
    let filtered_conv: Vec<serde_json::Value>;
    let conv: &[serde_json::Value] = if filter_transcript {
        filtered_conv = conv
            .iter()
            .filter(|v| v.get("tool_calls").is_none() && v.get("tool_call_id").is_none())
            .cloned()
            .collect();
        if filtered_conv.is_empty() {
            conv
        } else {
            &filtered_conv
        }
    } else {
        conv
    };
    let base = shop.url.trim_end_matches('/');
    let url = format!("{}/chat/completions", base);
    let body = Req {
        model: &station.model,
        messages: conv,
        stream: true,
        stream_options: StreamOptions {
            include_usage: true,
        },
        temperature: station.dials.boldness,
        reasoning_effort: station.dials.patience.map(|p| p.as_wire()),
        tool_choice,
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
        while let Some(end) = find_event_boundary(&buf) {
            let event_bytes = buf.drain(..end.end).collect::<Vec<u8>>();
            let event = &event_bytes[..end.body_len];
            handle_event(event, tx, &mut calls, &mut assistant_content)?;
        }
    }
    if !buf.is_empty() {
        handle_event(&buf, tx, &mut calls, &mut assistant_content)?;
    }

    for c in calls.iter().filter(|c| !c.name.is_empty()) {
        let _ = tx.send(StreamEvent::ToolCall {
            name: Some(c.name.clone()),
        });
    }

    Ok((calls, assistant_content))
}

fn json_msg(m: &ApiMessage) -> Option<serde_json::Value> {
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
    let mut base = if m.images.is_empty() {
        serde_json::json!({ "role": m.role, "content": m.content })
    } else {
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
        let tcs: Vec<serde_json::Value> = m
            .tool_calls
            .iter()
            .filter(|c| !c.id.is_empty() && !c.name.is_empty())
            .map(|c| {
                serde_json::json!({
                    "id": c.id,
                    "type": "function",
                    "function": { "name": c.name, "arguments": c.arguments },
                })
            })
            .collect();
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
        if let Ok(chunk) = serde_json::from_str::<ChatChunk>(payload) {
            if let Some(id) = chunk.id.as_deref() {
                tracing::Span::current().record("response_id", id);
            }
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
                    let _ = tx.send(StreamEvent::Usage {
                        input: t,
                        output: 0,
                    });
                }
            }
            for choice in chunk.choices {
                match choice.finish_reason.as_deref() {
                    Some("length") => {
                        let _ = tx.send(StreamEvent::Error {
                            message: "stopped: token limit reached".into(),
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
                    if let Some(content) = delta.content
                        && !content.is_empty()
                    {
                        assistant_content.push_str(&content);
                        let _ = tx.send(StreamEvent::Delta { text: content });
                    }
                    if let Some(refusal) = delta.refusal
                        && !refusal.is_empty()
                    {
                        assistant_content.push_str(&refusal);
                        let _ = tx.send(StreamEvent::Delta { text: refusal });
                    }
                    if let Some(reasoning) = delta.reasoning_content
                        && !reasoning.is_empty()
                    {
                        let _ = tx.send(StreamEvent::Heart { text: reasoning });
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
    }
    Ok(())
}

#[derive(Deserialize)]
struct ChatChunk {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    choices: Vec<Choice>,
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
    #[serde(default)]
    arguments: Option<serde_json::Value>,
}

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
        let mut m = base();
        m.tool_calls.push(ApiToolCall {
            id: "".into(),
            name: "zsh".into(),
            arguments: "{}".into(),
        });
        let v = json_msg(&m).unwrap();
        assert!(v.get("tool_calls").is_none());
        let mut m = base();
        m.tool_calls.push(ApiToolCall {
            id: "c1".into(),
            name: "".into(),
            arguments: "{}".into(),
        });
        let v = json_msg(&m).unwrap();
        assert!(v.get("tool_calls").is_none());
        let mut m = base();
        m.tool_calls.push(ApiToolCall {
            id: "c1".into(),
            name: "zsh".into(),
            arguments: "{}".into(),
        });
        let v = json_msg(&m).unwrap();
        assert_eq!(v["tool_calls"].as_array().unwrap().len(), 1);
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
    fn usage_chunk_emits_usage_event() {
        let (tx, mut rx) = channel();
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
        assert!(matches!(
            ev,
            StreamEvent::Usage {
                input: 1200,
                output: 300
            }
        ));
    }

    #[test]
    fn usage_chunk_parses_cleanly() {
        let (tx, _rx) = channel();
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

    #[test]
    fn tools_off_withholds_tools_but_keeps_transcript() {
        let (offer_tools, filter_transcript) = tool_policy(false, true);
        assert!(!offer_tools, "final round must not offer tools");
        assert!(
            !filter_transcript,
            "final round must keep the tool transcript so the model can still read what it gathered"
        );
    }

    #[test]
    fn toolless_model_still_filters_transcript() {
        let (offer_tools, filter_transcript) = tool_policy(true, false);
        assert!(!offer_tools);
        assert!(filter_transcript);
    }

    #[test]
    fn normal_round_offers_tools_and_keeps_transcript() {
        let (offer_tools, filter_transcript) = tool_policy(false, false);
        assert!(offer_tools);
        assert!(!filter_transcript);
    }

    #[test]
    fn nudge_tells_the_model_to_stop_calling_tools() {
        let n = crate::api::FINAL_ANSWER_NUDGE;
        assert!(n.to_lowercase().contains("do not call"));
    }
}
