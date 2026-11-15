use anyhow::{Context, Result, anyhow};
use toml_edit::{Array, ArrayOfTables, DocumentMut, Item, Table, value};

use crate::shop::{Protocol, WindowMode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    None,
    Plain(String),
    Env(String),
}

impl Key {
    fn parse(text: &str) -> Self {
        match text.trim() {
            "" => Key::None,
            t => match t.strip_prefix('$') {
                Some(name) => Key::Env(name.to_string()),
                None => Key::Plain(t.to_string()),
            },
        }
    }

    fn shown(&self) -> String {
        match self {
            Key::None => "(none)".into(),
            Key::Env(name) => format!("${name}"),
            Key::Plain(k) => {
                let tail: String = k.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
                format!("••••{tail}")
            }
        }
    }

    fn text(&self) -> String {
        match self {
            Key::None => String::new(),
            Key::Env(name) => format!("${name}"),
            Key::Plain(k) => k.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    pub name: String,
    pub url: String,
    pub key: Key,
    pub protocol: Protocol,
    pub window: WindowMode,
    pub models: Vec<String>,
}

impl Draft {
    pub fn blank() -> Self {
        Draft {
            name: String::new(),
            url: String::new(),
            key: Key::None,
            protocol: Protocol::Responses,
            window: WindowMode::Full,
            models: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Knob {
    Url,
    Key,
    Protocol,
    Window,
    Models,
}

impl Knob {
    pub const ALL: [Knob; 5] = [Knob::Url, Knob::Key, Knob::Protocol, Knob::Window, Knob::Models];

    pub fn name(self) -> &'static str {
        match self {
            Knob::Url => "url",
            Knob::Key => "key",
            Knob::Protocol => "protocol",
            Knob::Window => "window",
            Knob::Models => "models",
        }
    }

    pub fn typed(self) -> bool {
        matches!(self, Knob::Url | Knob::Key | Knob::Models)
    }

    pub fn shown(self, d: &Draft) -> String {
        match self {
            Knob::Url if d.url.is_empty() => "local (no network)".into(),
            Knob::Key => d.key.shown(),
            knob => knob.text(d),
        }
    }

    pub fn text(self, d: &Draft) -> String {
        match self {
            Knob::Url => d.url.clone(),
            Knob::Key => d.key.text(),
            Knob::Protocol => protocol_name(d.protocol).into(),
            Knob::Window => window_name(d.window).into(),
            Knob::Models => d.models.join(", "),
        }
    }

    pub fn set(self, d: &mut Draft, text: &str) {
        match self {
            Knob::Url => d.url = text.trim().to_string(),
            Knob::Key => d.key = Key::parse(text),
            Knob::Models => {
                d.models = text.split(',').map(str::trim).filter(|m| !m.is_empty()).map(String::from).collect()
            }
            Knob::Protocol | Knob::Window => self.cycle(d),
        }
    }

    pub fn cycle(self, d: &mut Draft) {
        match self {
            Knob::Protocol => {
                d.protocol = match d.protocol {
                    Protocol::ChatCompletions => Protocol::Responses,
                    _ => Protocol::ChatCompletions,
                }
            }
            Knob::Window => {
                d.window = match d.window {
                    WindowMode::Full => WindowMode::Warm,
                    WindowMode::Warm => WindowMode::Full,
                }
            }
            Knob::Url | Knob::Key | Knob::Models => {}
        }
    }
}

pub fn protocol_name(p: Protocol) -> &'static str {
    match p {
        Protocol::Demo => "demo",
        Protocol::ChatCompletions => "chat-completions",
        Protocol::Responses => "responses",
    }
}

pub fn window_name(w: WindowMode) -> &'static str {
    match w {
        WindowMode::Full => "full",
        WindowMode::Warm => "warm",
    }
}

pub fn saved(text: &str) -> Result<Vec<Draft>> {
    let doc: DocumentMut = text.parse().context("parsing shops.toml")?;
    let Some(shops) = doc.get("shop").and_then(Item::as_array_of_tables) else {
        return Ok(Vec::new());
    };
    Ok(shops.iter().map(draft).collect())
}

pub fn update(text: &str, origin: &str, d: &Draft) -> Result<String> {
    let mut doc: DocumentMut = text.parse().context("parsing shops.toml")?;
    let table = doc
        .get_mut("shop")
        .and_then(Item::as_array_of_tables_mut)
        .and_then(|shops| shops.iter_mut().find(|t| named(t) == Some(origin)))
        .ok_or_else(|| anyhow!("shop '{origin}' is not in shops.toml"))?;
    write(table, d);
    Ok(doc.to_string())
}

pub fn add(text: &str, d: &Draft) -> Result<String> {
    let mut doc: DocumentMut = text.parse().context("parsing shops.toml")?;
    let shops = doc
        .entry("shop")
        .or_insert(Item::ArrayOfTables(ArrayOfTables::new()))
        .as_array_of_tables_mut()
        .context("shop in shops.toml is not a list of [[shop]] tables")?;
    if shops.iter().any(|t| named(t) == Some(d.name.as_str())) {
        return Err(anyhow!("shop '{}' already exists", d.name));
    }
    let mut table = Table::new();
    write(&mut table, d);
    shops.push(table);
    Ok(doc.to_string())
}

fn named(t: &Table) -> Option<&str> {
    t.get("name").and_then(Item::as_str)
}

fn draft(t: &Table) -> Draft {
    let text = |k: &str| t.get(k).and_then(Item::as_str).map(String::from);
    let key = match (text("key"), text("key_env")) {
        (Some(k), _) if !k.is_empty() => Key::Plain(k),
        (_, Some(name)) => Key::Env(name),
        _ => Key::None,
    };
    Draft {
        name: text("name").unwrap_or_default(),
        url: text("url").unwrap_or_default(),
        key,
        protocol: match text("protocol").as_deref() {
            Some("chat-completions") => Protocol::ChatCompletions,
            _ => Protocol::Responses,
        },
        window: match text("window").as_deref() {
            Some("warm") => WindowMode::Warm,
            _ => WindowMode::Full,
        },
        models: t
            .get("models")
            .and_then(Item::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default(),
    }
}

fn write(t: &mut Table, d: &Draft) {
    t["name"] = value(&d.name);
    t["url"] = value(&d.url);
    match &d.key {
        Key::None => {
            t.remove("key");
            t.remove("key_env");
        }
        Key::Plain(k) => {
            t["key"] = value(k);
            t.remove("key_env");
        }
        Key::Env(name) => {
            t["key_env"] = value(name);
            t.remove("key");
        }
    }
    choice(t, "protocol", protocol_name(d.protocol), "responses");
    choice(t, "window", window_name(d.window), "full");
    t["models"] = value(Array::from_iter(d.models.iter().map(String::as_str)));
}

fn choice(t: &mut Table, key: &str, chosen: &str, default: &str) {
    if chosen != default || t.contains_key(key) {
        t[key] = value(chosen);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "\
# my shops
[[shop]]
name = \"ollama\"
url = \"http://localhost:11434/v1\"
protocol = \"chat-completions\"
models = [\"qwen3\"]
headers = { x-team = \"wryme\" } # keep me

[[shop]]
name = \"cloud\"
url = \"https://api.example.com/v1\"
key_env = \"CLOUD_KEY\"
models = []
";

    #[test]
    fn reads_every_saved_shop_with_its_key_source() {
        let shops = saved(FILE).unwrap();
        assert_eq!(shops.len(), 2);
        assert_eq!(shops[0].protocol, Protocol::ChatCompletions);
        assert_eq!(shops[0].models, ["qwen3"]);
        assert_eq!(shops[1].key, Key::Env("CLOUD_KEY".into()));
        assert_eq!(Knob::Key.shown(&shops[1]), "$CLOUD_KEY");
    }

    #[test]
    fn updating_changes_only_what_moved_and_keeps_the_rest() {
        let mut d = saved(FILE).unwrap().remove(0);
        Knob::Models.set(&mut d, "qwen3, llama3 ,");
        Knob::Key.set(&mut d, "sk-secret-1234");
        let out = update(FILE, "ollama", &d).unwrap();
        assert!(out.starts_with("# my shops"));
        assert!(out.contains("headers = { x-team = \"wryme\" } # keep me"));
        assert!(out.contains("models = [\"qwen3\", \"llama3\"]"));
        assert!(out.contains("key = \"sk-secret-1234\""));
        assert!(out.contains("key_env = \"CLOUD_KEY\""));
        assert_eq!(saved(&out).unwrap()[0], d);
        assert_eq!(Knob::Key.shown(&d), "••••1234");
    }

    #[test]
    fn an_env_key_replaces_a_plain_one_and_defaults_stay_unwritten() {
        let mut d = saved(FILE).unwrap().remove(1);
        Knob::Key.set(&mut d, "  ");
        let out = update(FILE, "cloud", &d).unwrap();
        let cloud = &out[out.find("name = \"cloud\"").unwrap()..];
        assert!(!cloud.contains("key"));
        assert!(!cloud.contains("protocol"));
        assert!(!cloud.contains("window"));
    }

    #[test]
    fn saving_as_new_appends_a_shop_and_refuses_a_taken_name() {
        let mut d = saved(FILE).unwrap().remove(1);
        d.name = "cloud two".into();
        Knob::Window.cycle(&mut d);
        let out = add(FILE, &d).unwrap();
        let shops = saved(&out).unwrap();
        assert_eq!(shops.len(), 3);
        assert_eq!(shops[2].window, WindowMode::Warm);
        assert!(add(&out, &d).is_err());
        assert!(update(FILE, "nobody", &d).is_err());
        assert_eq!(saved(&add("", &d).unwrap()).unwrap(), [d]);
    }
}
