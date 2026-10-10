use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

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
            headers: HashMap::new(),
        }
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
        models,
        headers: HashMap::new(),
    })
}

pub fn find_for_model<'a>(shops: &'a [Shop], model: &str) -> Option<&'a Shop> {
    shops.iter().find(|s| s.models.iter().any(|m| m == model))
}

pub async fn discover_all(shops: &mut [Shop]) {
    let http = match reqwest::Client::builder().build() {
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
    for shop in shops.iter_mut() {
        if shop.protocol == Protocol::Demo {
            continue;
        }
        if let Err(e) = discover_models(shop, &http).await {
            tracing::warn!(shop = %shop.name, err = %format!("{e:#}"), "model discovery failed");
        }
    }
}

async fn discover_models(shop: &mut Shop, http: &reqwest::Client) -> Result<()> {
    #[derive(Deserialize)]
    struct ModelsResponse {
        data: Vec<ModelEntry>,
    }

    let url = format!("{}/models", shop.url.trim_end_matches('/'));
    let mut req = http.get(&url);
    if !shop.key.is_empty() {
        req = req.bearer_auth(&shop.key);
    }
    let resp = req.send().await.with_context(|| format!("GET {}", url))?;
    if !resp.status().is_success() {
        return Err(anyhow!("upstream {}", resp.status()));
    }
    let parsed: ModelsResponse = resp.json().await.context("parsing /v1/models response")?;
    let configured = shop.models.clone();
    let (models, tools_by_model) = apply_discovery(parsed.data, &configured);
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

    #[test]
    fn find_for_model_picks_first_matching() {
        let shops = vec![
            Shop {
                name: "a".into(),
                url: "u1".into(),
                key: "".into(),
                protocol: Protocol::ChatCompletions,
                window: WindowMode::Full,
                models: vec!["m1".into(), "m2".into()],
                headers: HashMap::new(),
            },
            Shop {
                name: "b".into(),
                url: "u2".into(),
                key: "".into(),
                protocol: Protocol::Responses,
                window: WindowMode::Warm,
                models: vec!["m2".into(), "m3".into()],
                headers: HashMap::new(),
            },
        ];
        assert_eq!(find_for_model(&shops, "m1").unwrap().name, "a");
        assert_eq!(find_for_model(&shops, "m2").unwrap().name, "a");
        assert_eq!(find_for_model(&shops, "m3").unwrap().name, "b");
        assert!(find_for_model(&shops, "nope").is_none());
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
