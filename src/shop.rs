// Shops: the kitchens. Where the model lives, how we reach it, and what
// authentication it wants. Stations declare a model name; shops declare
// which model names they serve. At startup we match them by string.
//
// Sources, in order:
//   1. Built-in demo shop. Always present, never speaks over the wire,
//      streams canned replies. The thing a brand-new user lands on when
//      they have no config.
//   2. WME_DEFAULT_SHOP_* env vars. Defines one shop inline. Convenient
//      for the "just install and point it somewhere" case.
//   3. ~/.config/wryme/shops.toml. Any number of named shops.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

/// How a Responses shop carries the window between turns.
///
/// `Full` (default): stateless. Every request carries the whole
/// transcript with `store: false`; works against any shop, but the
/// server re-prefills everything each tool round, so long windows get
/// slow. `Warm`: the server keeps the window warm; follow-ups send only
/// the new items against `previous_response_id` with `store: true`.
/// Fast, but only shops that actually retain windows (OpenAI, our ds4).
/// Set `window = "warm"` per shop to opt in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowMode {
    Full,
    Warm,
}
///
/// `Demo` is our local canned-replies generator. No network.
/// `Responses` is the default: the newer typed-event protocol at
/// `/v1/responses`. Cleaner for tool calls, reasoning, refusals, and
/// built-in tools. Stateless (`store: false`, full transcript replayed),
/// so it works against any shop that implements the endpoint — OpenAI,
/// our local ds4/glm servers, and Ollama's OpenAI-compat endpoint.
/// `ChatCompletions` is the opt-out baseline: `/v1/chat/completions`
/// with flat `choices[].delta` chunks. Set
/// `protocol = "chat-completions"` for servers with no `/responses`.
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
    /// Models this shop advertises. Convention: list newest-first. The
    /// first model is what wryme picks when synthesizing a default
    /// station for a fresh launch with no saved stations.
    pub models: Vec<String>,
    /// Custom headers sent with every request to this shop.
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
    /// Inline key. Use `key_env` instead if you don't want secrets in the
    /// config file.
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    key_env: Option<String>,
    /// "responses" (default) or "chat-completions".
    #[serde(default)]
    protocol: Option<String>,
    /// "full" (default) or "warm". Warm keeps the window server-side.
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

    if let Some(path) = config_path() {
        if !path.exists() {
            // First run — seed a file so canned shows as a persisted shop/radio and user sees the shape.
            let _ = ensure_default_file(&path);
        }
        if path.exists() {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let parsed: ShopsFile = toml::from_str(&text)
                .with_context(|| format!("parsing {}", path.display()))?;
            for def in parsed.shop {
                out.push(def.resolve());
            }
        }
    }
    Ok(out)
}

fn ensure_default_file(path: &PathBuf) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let body = r#"# wryme shops — add your providers here. `canned` is local, no network.
[[shop]]
name = "canned"
url = ""
models = ["canned replies"]
"#;
    std::fs::write(path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
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
        url: url.unwrap_or_else(|| "https://api.openai.com/v1".into()),
        key,
        protocol,
        window,
        models,
        headers: HashMap::new(),
    })
}

fn config_path() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(|h| {
        PathBuf::from(h)
            .join(".config")
            .join("wryme")
            .join("shops.toml")
    })
}

/// Find the first shop whose `models` list advertises this model name.
/// Returns None if no shop serves it. Callers should treat that as an
/// error at startup with a helpful message.
pub fn find_for_model<'a>(shops: &'a [Shop], model: &str) -> Option<&'a Shop> {
    shops.iter().find(|s| s.models.iter().any(|m| m == model))
}

/// Hit each shop's `/v1/models` endpoint to populate its `models` list.
/// Shops that already have a non-empty `models` (specified by the user
/// in shops.toml) are left alone. Demo is skipped. Returns the list of
/// (shop_name, error) pairs for shops where discovery failed.
pub async fn discover_all(shops: &mut [Shop]) -> Vec<(String, String)> {
    let http = match reqwest::Client::builder().build() {
        Ok(c) => c,
        Err(e) => {
            return shops
                .iter()
                .map(|s| (s.name.clone(), format!("http client: {}", e)))
                .collect();
        }
    };
    let mut errors = Vec::new();
    for shop in shops.iter_mut() {
        if shop.protocol == Protocol::Demo || !shop.models.is_empty() {
            continue;
        }
        if let Err(e) = discover_models(shop, &http).await {
            errors.push((shop.name.clone(), format!("{:#}", e)));
        }
    }
    errors
}

async fn discover_models(shop: &mut Shop, http: &reqwest::Client) -> Result<()> {
    #[derive(Deserialize)]
    struct ModelsResponse {
        data: Vec<ModelEntry>,
    }
    #[derive(Deserialize)]
    struct ModelEntry {
        id: String,
    }

    let url = format!("{}/models", shop.url.trim_end_matches('/'));
    let mut req = http.get(&url);
    if !shop.key.is_empty() {
        req = req.bearer_auth(&shop.key);
    }
    let resp = req
        .send()
        .await
        .with_context(|| format!("GET {}", url))?;
    if !resp.status().is_success() {
        return Err(anyhow!("upstream {}", resp.status()));
    }
    let parsed: ModelsResponse = resp.json().await.context("parsing /v1/models response")?;
    shop.models = parsed.data.into_iter().map(|m| m.id).collect();
    Ok(())
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
        assert_eq!(
            def(Some("warm".into())).resolve().window,
            WindowMode::Warm
        );
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
}
