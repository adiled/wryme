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
use std::path::PathBuf;

/// Which wire protocol this shop speaks.
///
/// `Demo` is our local canned-replies generator. No network.
/// `ChatCompletions` is the universal baseline: `/v1/chat/completions`
/// with flat `choices[].delta` chunks. Almost every server.
/// `Responses` is the newer typed-event protocol at `/v1/responses`.
/// Cleaner for tool calls, reasoning, refusals, and built-in tools.
/// OpenAI directly and agentic backends (like our local kara) support it.
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
    /// Models this shop advertises. Convention: list newest-first. The
    /// first model is what wryme picks when synthesizing a default
    /// station for a fresh launch with no saved stations.
    pub models: Vec<String>,
}

impl Shop {
    pub fn demo() -> Self {
        Self {
            name: "demo".into(),
            url: String::new(),
            key: String::new(),
            protocol: Protocol::Demo,
            models: vec!["canned replies".into()],
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
    /// "chat-completions" (default) or "responses".
    #[serde(default)]
    protocol: Option<String>,
    #[serde(default)]
    models: Vec<String>,
}

impl ShopDef {
    fn resolve(self) -> Shop {
        let key = match (self.key, self.key_env) {
            (Some(k), _) => k,
            (None, Some(env_name)) => std::env::var(&env_name).unwrap_or_default(),
            (None, None) => String::new(),
        };
        let protocol = match self.protocol.as_deref() {
            Some("responses") => Protocol::Responses,
            _ => Protocol::ChatCompletions,
        };
        Shop {
            name: self.name,
            url: self.url,
            key,
            protocol,
            models: self.models,
        }
    }
}

pub fn load_all() -> Result<Vec<Shop>> {
    let mut out = vec![Shop::demo()];

    if let Some(env_shop) = from_env() {
        out.push(env_shop);
    }

    if let Some(path) = config_path() {
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

fn from_env() -> Option<Shop> {
    let name = std::env::var("WME_DEFAULT_SHOP_NAME").ok();
    let url = std::env::var("WME_DEFAULT_SHOP_URL").ok();
    let key = std::env::var("WME_DEFAULT_SHOP_KEY")
        .ok()
        .or_else(|| std::env::var("OPENAI_API_KEY").ok())
        .unwrap_or_default();
    let protocol = std::env::var("WME_DEFAULT_SHOP_PROTOCOL").ok();
    let models = std::env::var("WME_DEFAULT_SHOP_MODELS").ok();

    if name.is_none() && url.is_none() && key.is_empty() && protocol.is_none() && models.is_none()
    {
        return None;
    }

    let protocol = match protocol.as_deref() {
        Some("responses") => Protocol::Responses,
        _ => Protocol::ChatCompletions,
    };
    let models: Vec<String> = models
        .map(|s| s.split(',').map(|m| m.trim().to_string()).collect())
        .unwrap_or_default();

    Some(Shop {
        name: name.unwrap_or_else(|| "default".into()),
        url: url.unwrap_or_else(|| "https://api.openai.com/v1".into()),
        key,
        protocol,
        models,
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
    fn find_for_model_picks_first_matching() {
        let shops = vec![
            Shop {
                name: "a".into(),
                url: "u1".into(),
                key: "".into(),
                protocol: Protocol::ChatCompletions,
                models: vec!["m1".into(), "m2".into()],
            },
            Shop {
                name: "b".into(),
                url: "u2".into(),
                key: "".into(),
                protocol: Protocol::Responses,
                models: vec!["m2".into(), "m3".into()],
            },
        ];
        assert_eq!(find_for_model(&shops, "m1").unwrap().name, "a");
        assert_eq!(find_for_model(&shops, "m2").unwrap().name, "a");
        assert_eq!(find_for_model(&shops, "m3").unwrap().name, "b");
        assert!(find_for_model(&shops, "nope").is_none());
    }
}
