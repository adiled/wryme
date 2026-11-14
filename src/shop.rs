use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

use crate::shop_memory::Memory;

#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub name: String,
    pub def: Value,
}

const DEFAULT_OPENAI_URL: &str = "https://api.openai.com/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowMode {
    Full,
    Warm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Demo,
    ChatCompletions,
    Responses,
}

#[derive(Debug, Clone)]
pub struct Shop {
    pub name: String,
    pub url: String,
    pub key: String,
    pub protocol: Protocol,
    pub window: WindowMode,
    pub models: Vec<String>,
    pub limit: Vec<String>,
    pub headers: HashMap<String, String>,
}

impl Shop {
    pub fn demo() -> Self {
        Self {
            name: "demo".into(),
            url: String::new(),
            key: String::new(),
            protocol: Protocol::Demo,
            window: WindowMode::Full,
            models: vec!["canned replies".into()],
            limit: vec!["canned replies".into()],
            headers: HashMap::new(),
        }
    }

    pub fn lists(&self, model: &str) -> bool {
        self.models.iter().any(|m| m == model)
    }

    pub fn may_take(&self, model: &str) -> bool {
        self.protocol != Protocol::Demo
            && (self.limit.is_empty() || self.limit.iter().any(|m| m == model))
    }
}

#[derive(Debug, Deserialize)]
struct ShopsFile {
    #[serde(default)]
    shop: Vec<ShopDef>,
}

#[derive(Debug, Deserialize)]
struct ShopDef {
    name: String,
    url: String,
    key: Option<String>,
    #[serde(default)]
    key_env: Option<String>,
    #[serde(default)]
    protocol: Option<String>,
    #[serde(default)]
    window: Option<String>,
    #[serde(default)]
    models: Vec<String>,
    #[serde(default)]
    headers: HashMap<String, String>,
}

impl ShopDef {
    fn resolve(self) -> Shop {
        let key = match (self.key, self.key_env) {
            (Some(k), _) => k,
            (None, Some(env_name)) => std::env::var(&env_name).unwrap_or_default(),
            (None, None) => String::new(),
        };
        let protocol = match self.protocol.as_deref() {
            Some("chat-completions") => Protocol::ChatCompletions,
            _ => Protocol::Responses,
        };
        let window = match self.window.as_deref() {
            Some("warm") => WindowMode::Warm,
            _ => WindowMode::Full,
        };
        Shop {
            name: self.name,
            url: self.url,
            key,
            protocol,
            window,
            limit: self.models.clone(),
            models: self.models,
            headers: self.headers,
        }
    }
}

pub fn load_all() -> Result<Vec<Shop>> {
    let mut out = vec![Shop::demo()];

    if let Some(env_shop) = from_env() {
        out.push(env_shop);
    }

    if let Some(text) = crate::config::shops_text() {
        let parsed: ShopsFile = toml::from_str(&text).context("parsing shops.toml")?;
        for def in parsed.shop {
            out.push(def.resolve());
        }
    }
    if crate::sync::AWAY
        && let Some(host) = crate::config::host_pref()
    {
        for shop in &mut out {
            if let Some(url) = crate::sync::reach(&shop.url, &host) {
                shop.url = url;
            }
        }
    }
    Ok(out)
}

fn from_env() -> Option<Shop> {
    let name = std::env::var("WME_DEFAULT_SHOP_NAME").ok();
    let url = std::env::var("WME_DEFAULT_SHOP_URL").ok();
    let key = std::env::var("WME_DEFAULT_SHOP_KEY")
        .ok()
        .or_else(|| std::env::var("OPENAI_API_KEY").ok())
        .unwrap_or_default();
    let protocol = std::env::var("WME_DEFAULT_SHOP_PROTOCOL").ok();
    let window = std::env::var("WME_DEFAULT_SHOP_WINDOW").ok();
    let models = std::env::var("WME_DEFAULT_SHOP_MODELS").ok();

    if name.is_none()
        && url.is_none()
        && key.is_empty()
        && protocol.is_none()
        && window.is_none()
        && models.is_none()
    {
        return None;
    }

    let protocol = match protocol.as_deref() {
        Some("chat-completions") => Protocol::ChatCompletions,
        _ => Protocol::Responses,
    };
    let window = match window.as_deref() {
        Some("warm") => WindowMode::Warm,
        _ => WindowMode::Full,
    };
    let models: Vec<String> = models
        .map(|s| s.split(',').map(|m| m.trim().to_string()).collect())
        .unwrap_or_default();

    Some(Shop {
        name: name.unwrap_or_else(|| "default".into()),
        url: url.unwrap_or_else(|| DEFAULT_OPENAI_URL.into()),
        key,
        protocol,
        window,
        limit: models.clone(),
        models,
        headers: HashMap::new(),
    })
}

pub fn find_for_model<'a>(shops: &'a [Shop], model: &str) -> Option<&'a Shop> {
    shops.iter().find(|s| s.lists(model))
}

pub fn listing<'a>(shops: &'a [Shop], model: &str) -> Vec<&'a Shop> {
    shops.iter().filter(|s| s.lists(model)).collect()
}

pub fn remembered<'a>(shops: &'a [Shop], memory: &Memory, model: &str) -> Option<&'a Shop> {
    let name = memory.get(model)?;
    shops.iter().find(|s| &s.name == name && s.may_take(model))
}

pub fn known<'a>(shops: &'a [Shop], memory: &Memory, model: &str) -> Option<&'a Shop> {
    find_for_model(shops, model).or_else(|| remembered(shops, memory, model))
}

pub fn candidates(shops: &[Shop], model: &str, first: &str) -> Vec<Shop> {
    let mut out: Vec<Shop> = shops.iter().filter(|s| s.may_take(model)).cloned().collect();
    if let Some(i) = out.iter().position(|s| s.name == first) {
        let s = out.remove(i);
        out.insert(0, s);
    }
    out
}

pub fn choose(shops: &[Shop], memory: &Memory, model: &str, current: &str) -> Option<Shop> {
    known(shops, memory, model)
        .cloned()
        .or_else(|| candidates(shops, model, current).into_iter().next())
}

pub fn turn_shops(shops: &[Shop], active: &Shop, model: &str) -> Vec<Shop> {
    if active.protocol == Protocol::Demo || active.lists(model) {
        return vec![active.clone()];
    }
    let mut out = candidates(shops, model, &active.name);
    if let Some(s) = out.iter_mut().find(|s| s.name == active.name) {
        *s = active.clone();
    }
    if out.is_empty() {
        out.push(active.clone());
    }
    out
}

const DISCOVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

pub async fn discover_all(shops: &mut [Shop]) {
    #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
    let mut http_builder = reqwest::Client::builder();
    #[cfg(not(target_arch = "wasm32"))]
    for var in &["NODE_EXTRA_CA_CERTS", "SSL_CERT_FILE"] {
        if let Ok(path) = std::env::var(var) {
            if let Ok(pem) = std::fs::read(&path) {
                if let Ok(cert) = reqwest::Certificate::from_pem(&pem) {
                    http_builder = http_builder.add_root_certificate(cert);
                }
            }
        }
    }
    let http = match http_builder.build() {
        Ok(c) => c,
        Err(e) => {
            for shop in shops.iter() {
                if shop.protocol != Protocol::Demo {
                    tracing::warn!(shop = %shop.name, err = %e, "model discovery unavailable");
                }
            }
            return;
        }
    };
    let http = &http;
    futures_util::future::join_all(
        shops
            .iter_mut()
            .filter(|s| s.protocol != Protocol::Demo)
            .map(|shop| async move {
                match crate::platform::timeout(DISCOVERY_TIMEOUT, discover_models(shop, http)).await {
                    Some(Ok(())) => {}
                    Some(Err(e)) => {
                        tracing::warn!(shop = %shop.name, err = %format!("{e:#}"), "model discovery failed")
                    }
                    None => tracing::warn!(shop = %shop.name, "model discovery timed out"),
                }
            }),
    )
    .await;
}

pub(crate) fn should_send_auth(key: &str) -> bool {
    !key.is_empty() && !key.eq_ignore_ascii_case("public")
}

async fn discover_models(shop: &mut Shop, http: &reqwest::Client) -> Result<()> {
    #[derive(Deserialize)]
    struct ModelsResponse {
        data: Vec<ModelEntry>,
    }

    let url = format!("{}/models", shop.url.trim_end_matches('/'));
    let mut req = http.get(&url);
    if should_send_auth(&shop.key) {
        req = req.bearer_auth(&shop.key);
    }
    for (k, v) in &shop.headers {
        req = req.header(k, v);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| crate::api::unreachable(&shop.name, &url, &e))?;
    if !resp.status().is_success() {
        return Err(anyhow!("upstream {}", resp.status()));
    }
    let parsed: ModelsResponse = resp.json().await.context("parsing /v1/models response")?;
    let (models, tools_by_model) = apply_discovery(parsed.data, &shop.limit);
    shop.models = models;
    for (model, tools) in tools_by_model {
        crate::api::record_tools(&shop.name, &model, tools);
    }
    Ok(())
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
    #[serde(default)]
    tools: Vec<Value>,
}

fn apply_discovery(
    entries: Vec<ModelEntry>,
    configured: &[String],
) -> (Vec<String>, Vec<(String, Vec<Tool>)>) {
    let mut models = Vec::new();
    let mut tools_by_model = Vec::new();
    for entry in entries {
        if !configured.is_empty() && !configured.contains(&entry.id) {
            continue;
        }
        let mut tools = Vec::new();
        for def in entry.tools {
            let Some(name) = tool_name(&def) else {
                continue;
            };
            if !tools.iter().any(|t: &Tool| t.name == name) {
                tools.push(Tool { name, def });
            }
        }
        models.push(entry.id.clone());
        tools_by_model.push((entry.id, tools));
    }
    if models.is_empty() && !configured.is_empty() {
        models = configured.to_vec();
    }
    (models, tools_by_model)
}

fn tool_name(def: &Value) -> Option<String> {
    def.get("function")
        .and_then(|f| f.get("name"))
        .and_then(|n| n.as_str())
        .or_else(|| def.get("name").and_then(|n| n.as_str()))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_or_missing_key_is_anonymous() {
        assert!(!should_send_auth(""));
        assert!(!should_send_auth("public"));
        assert!(!should_send_auth("PUBLIC"));
        assert!(should_send_auth("sk-abc"));
    }

    #[test]
    fn demo_is_always_there() {
        let demo = Shop::demo();
        assert_eq!(demo.name, "demo");
        assert_eq!(demo.protocol, Protocol::Demo);
        assert_eq!(demo.models, vec!["canned replies"]);
    }

    #[test]
    fn protocol_defaults_to_responses() {
        let def = |protocol: Option<String>| ShopDef {
            name: "x".into(),
            url: "u".into(),
            key: None,
            key_env: None,
            protocol,
            window: None,
            models: vec![],
            headers: HashMap::new(),
        };
        assert_eq!(def(None).resolve().protocol, Protocol::Responses);
        assert_eq!(
            def(Some("chat-completions".into())).resolve().protocol,
            Protocol::ChatCompletions
        );
    }

    #[test]
    fn window_defaults_to_full_and_opts_into_warm() {
        let def = |window: Option<String>| ShopDef {
            name: "x".into(),
            url: "u".into(),
            key: None,
            key_env: None,
            protocol: None,
            window,
            models: vec![],
            headers: std::collections::HashMap::new(),
        };
        assert_eq!(def(None).resolve().window, WindowMode::Full);
        assert_eq!(def(Some("warm".into())).resolve().window, WindowMode::Warm);
    }

    fn shop(name: &str, models: &[&str], limit: &[&str]) -> Shop {
        Shop {
            name: name.into(),
            url: format!("http://{name}"),
            key: "".into(),
            protocol: Protocol::ChatCompletions,
            window: WindowMode::Full,
            models: models.iter().map(|m| m.to_string()).collect(),
            limit: limit.iter().map(|m| m.to_string()).collect(),
            headers: HashMap::new(),
        }
    }

    fn names(shops: &[Shop]) -> Vec<&str> {
        shops.iter().map(|s| s.name.as_str()).collect()
    }

    #[test]
    fn find_for_model_picks_first_matching() {
        let shops = vec![shop("a", &["m1", "m2"], &[]), shop("b", &["m2", "m3"], &[])];
        assert_eq!(find_for_model(&shops, "m1").unwrap().name, "a");
        assert_eq!(find_for_model(&shops, "m2").unwrap().name, "a");
        assert_eq!(find_for_model(&shops, "m3").unwrap().name, "b");
        assert!(find_for_model(&shops, "nope").is_none());
        assert_eq!(listing(&shops, "m2").len(), 2);
    }

    #[test]
    fn only_a_models_line_limits_a_shop() {
        assert!(shop("open", &[], &[]).may_take("anything"));
        assert!(shop("open", &["m1"], &[]).may_take("anything"));
        assert!(shop("held", &["m1"], &["m1"]).may_take("m1"));
        assert!(!shop("held", &["m1"], &["m1"]).may_take("m2"));
        assert!(!Shop::demo().may_take("anything"));
    }

    #[test]
    fn undiscovered_model_tries_the_active_shop_first_then_the_rest() {
        let shops = vec![
            Shop::demo(),
            shop("a", &["m1"], &[]),
            shop("held", &["m1"], &["m1"]),
            shop("hum", &[], &[]),
        ];
        let hum = shops[3].clone();
        assert_eq!(names(&turn_shops(&shops, &hum, "x")), vec!["hum", "a"]);
        assert_eq!(names(&turn_shops(&shops, &shops[1], "m1")), vec!["a"]);
        assert_eq!(names(&turn_shops(&shops, &shops[2], "x")), vec!["a", "hum"]);
    }

    #[test]
    fn choose_prefers_a_listing_shop_then_memory_then_current() {
        let shops = vec![shop("a", &["m1"], &[]), shop("hum", &[], &[])];
        let mut memory = Memory::new();
        assert_eq!(choose(&shops, &memory, "m1", "hum").unwrap().name, "a");
        assert_eq!(choose(&shops, &memory, "x", "hum").unwrap().name, "hum");
        memory.insert("x".into(), "a".into());
        assert_eq!(choose(&shops, &memory, "x", "hum").unwrap().name, "a");
        memory.insert("x".into(), "gone".into());
        assert_eq!(choose(&shops, &memory, "x", "a").unwrap().name, "a");
        let held = vec![shop("held", &["m1"], &["m1"])];
        assert!(choose(&held, &Memory::new(), "x", "held").is_none());
    }

    fn entry(id: &str, defs: Vec<serde_json::Value>) -> ModelEntry {
        ModelEntry {
            id: id.into(),
            tools: defs,
        }
    }

    fn fn_def(shape: &str, name: &str) -> serde_json::Value {
        if shape == "chat" {
            serde_json::json!({ "type": "function", "function": { "name": name } })
        } else {
            serde_json::json!({ "type": "function", "name": name })
        }
    }

    #[test]
    fn discovery_keeps_tools_per_model() {
        let entries = vec![
            entry("m1", vec![fn_def("chat", "sh"), fn_def("chat", "fs")]),
            entry("m2", vec![fn_def("responses", "sh"), fn_def("responses", "curl")]),
        ];
        let (models, tools_by_model) = apply_discovery(entries, &[]);
        assert_eq!(models, vec!["m1".to_string(), "m2".to_string()]);
        assert_eq!(tools_by_model.len(), 2);
        let (m1, m1_tools) = &tools_by_model[0];
        assert_eq!((m1.as_str(), m1_tools.len()), ("m1", 2));
        assert_eq!(m1_tools[0].name, "sh");
        assert_eq!(m1_tools[1].name, "fs");
        let (m2, m2_tools) = &tools_by_model[1];
        assert_eq!((m2.as_str(), m2_tools.len()), ("m2", 2));
        assert_eq!(m2_tools[0].name, "sh");
        assert_eq!(m2_tools[1].name, "curl");
    }

    #[test]
    fn discovery_config_filters_models_and_their_tools() {
        let entries = vec![
            entry("m1", vec![fn_def("chat", "sh")]),
            entry("m2", vec![fn_def("chat", "curl")]),
        ];
        let configured = vec!["m2".to_string()];
        let (models, tools_by_model) = apply_discovery(entries, &configured);
        assert_eq!(models, vec!["m2".to_string()]);
        assert_eq!(tools_by_model.len(), 1);
        assert_eq!(tools_by_model[0].0, "m2");
        assert_eq!(tools_by_model[0].1.len(), 1);
        assert_eq!(tools_by_model[0].1[0].name, "curl");
    }

    #[test]
    fn discovery_falls_back_to_config_when_nothing_matches() {
        let entries = vec![entry("mx", vec![fn_def("chat", "sh")])];
        let configured = vec!["m9".to_string()];
        let (models, tools_by_model) = apply_discovery(entries, &configured);
        assert_eq!(models, vec!["m9".to_string()]);
        assert!(tools_by_model.is_empty());
    }

    #[test]
    fn discovery_skips_defs_without_a_name() {
        let entries = vec![entry(
            "m1",
            vec![
                fn_def("chat", "sh"),
                serde_json::json!({ "type": "function" }),
            ],
        )];
        let (models, tools_by_model) = apply_discovery(entries, &[]);
        assert_eq!(models, vec!["m1".to_string()]);
        assert_eq!(tools_by_model[0].1.len(), 1);
        assert_eq!(tools_by_model[0].1[0].name, "sh");
    }
}
