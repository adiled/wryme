use anyhow::Result;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) const DEFAULT_SHOPS: &str = r#"# wryme shops — add your providers here. `canned` is local, no network. All shop knobs shown.
[[shop]]
name = "canned"
url = ""
models = ["canned replies"]
# protocol = "chat-completions" # or "responses"
# window = "full" # or "warm"
# key = ""
# key_env = "OPENAI_API_KEY"
# headers = { }
"#;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) const DEFAULT_STATIONS: &str = r#"# wryme stations — canned is local, no network. All dials shown with defaults.
[[station]]
name = "canned"
model = "canned replies"
# boldness = 0.7
patience = "steady"
# brainy = "murmur"
tinker_keep = "all"
tinker_clip = "all"
tinker_depth = "all"
# voice = "Tara"
"#;

pub const ACCENTS: &[(&str, ratatui::style::Color)] = &[
    ("cyan", ratatui::style::Color::Cyan),
    ("azure", ratatui::style::Color::Rgb(125, 185, 205)),
    ("green", ratatui::style::Color::Green),
    ("lightgreen", ratatui::style::Color::LightGreen),
    ("yellow", ratatui::style::Color::Yellow),
    ("magenta", ratatui::style::Color::Magenta),
    ("red", ratatui::style::Color::Red),
    ("blue", ratatui::style::Color::Blue),
    ("white", ratatui::style::Color::White),
    ("gray", ratatui::style::Color::Gray),
    ("lightcyan", ratatui::style::Color::LightCyan),
    ("lightyellow", ratatui::style::Color::LightYellow),
    ("lightmagenta", ratatui::style::Color::LightMagenta),
];

#[cfg(not(target_arch = "wasm32"))]
const DEFAULT_PREFS: &str = "{\n  \"accent\": \"cyan\"\n}\n";

pub fn accent_index(color: ratatui::style::Color) -> usize {
    ACCENTS
        .iter()
        .position(|(_, c)| *c == color)
        .unwrap_or(0)
}

pub fn accent_pref() -> ratatui::style::Color {
    parse_accent(prefs_text().as_deref())
}

pub fn parse_accent(text: Option<&str>) -> ratatui::style::Color {
    let Some(text) = text else {
        return ACCENTS[0].1;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return ACCENTS[0].1;
    };
    let Some(name) = v.get("accent").and_then(|s| s.as_str()) else {
        return ACCENTS[0].1;
    };
    ACCENTS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, c)| *c)
        .unwrap_or(ACCENTS[0].1)
}

pub fn write_accent(color: ratatui::style::Color) -> Result<()> {
    let name = ACCENTS[accent_index(color)].0;
    write_prefs(&format!("{{\n  \"accent\": \"{name}\"\n}}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_round_trip() {
        for (name, color) in ACCENTS {
            let json = format!("{{\"accent\": \"{name}\"}}");
            assert_eq!(parse_accent(Some(&json)), *color);
        }
    }

    #[test]
    fn accent_falls_back_to_cyan() {
        assert_eq!(parse_accent(None), ratatui::style::Color::Cyan);
        assert_eq!(
            parse_accent(Some("{\"accent\": \"neon-pink\"}")),
            ratatui::style::Color::Cyan
        );
        assert_eq!(parse_accent(Some("not json")), ratatui::style::Color::Cyan);
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use anyhow::{Context, Result};
    use std::path::PathBuf;

    fn dir() -> Option<PathBuf> {
        std::env::var("HOME").ok().map(|h| {
            PathBuf::from(h)
                .join(".config")
                .join("wryme")
        })
    }

    fn read_or_seed(name: &str, seed: &str) -> Option<String> {
        let path = dir()?.join(name);
        if !path.exists() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&path, seed);
        }
        std::fs::read_to_string(&path).ok()
    }

    pub fn shops_text() -> Option<String> {
        read_or_seed("shops.toml", super::DEFAULT_SHOPS)
    }

    pub fn stations_text() -> Option<String> {
        read_or_seed("stations.toml", super::DEFAULT_STATIONS)
    }

    pub fn write_stations(text: &str) -> Result<()> {
        let path = dir().context("no $HOME")?.join("stations.toml");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
    }

    pub fn prefs_text() -> Option<String> {
        read_or_seed("prefs.json", super::DEFAULT_PREFS)
    }

    pub fn write_prefs(text: &str) -> Result<()> {
        let path = dir().context("no $HOME")?.join("prefs.json");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
    }

    pub fn can_write() -> bool {
        dir().is_some()
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use anyhow::Result;
    use std::cell::RefCell;
    use wasm_bindgen::{JsCast, JsValue};

    thread_local! {
        static SHOPS: RefCell<Option<String>> = const { RefCell::new(None) };
        static STATIONS: RefCell<Option<String>> = const { RefCell::new(None) };
        static PREFS: RefCell<Option<String>> = const { RefCell::new(None) };
        static CONNECTED: RefCell<bool> = const { RefCell::new(false) };
        static ROSTER: RefCell<Option<String>> = const { RefCell::new(None) };
    }

    fn call_global(name: &str, args: &[JsValue]) {
        let global = js_sys::global();
        let Ok(f) = js_sys::Reflect::get(&global, &JsValue::from_str(name)) else {
            return;
        };
        let Some(f) = f.dyn_ref::<js_sys::Function>() else {
            return;
        };
        let args = js_sys::Array::from_iter(args.iter().cloned());
        let _ = f.apply(&global, &args);
    }

    pub fn set_text(shops: Option<String>, stations: Option<String>, connected: bool) {
        SHOPS.with(|c| *c.borrow_mut() = shops);
        STATIONS.with(|c| *c.borrow_mut() = stations);
        CONNECTED.with(|c| *c.borrow_mut() = connected);
    }

    pub fn set_prefs_text(text: Option<String>) {
        PREFS.with(|c| *c.borrow_mut() = text);
    }

    pub fn prefs_text() -> Option<String> {
        PREFS.with(|c| c.borrow().clone())
    }

    pub fn set_roster_text(text: Option<String>) {
        ROSTER.with(|c| *c.borrow_mut() = text);
    }

    pub fn roster_text() -> Option<String> {
        ROSTER.with(|c| c.borrow().clone())
    }

    pub fn write_prefs(text: &str) -> Result<()> {
        PREFS.with(|c| *c.borrow_mut() = Some(text.to_string()));
        call_global(
            "wrymeWriteFile",
            &[JsValue::from_str("prefs.json"), JsValue::from_str(text)],
        );
        Ok(())
    }

    pub fn shops_text() -> Option<String> {
        SHOPS.with(|c| c.borrow().clone())
    }

    pub fn stations_text() -> Option<String> {
        STATIONS.with(|c| c.borrow().clone())
    }

    pub fn write_stations(text: &str) -> Result<()> {
        STATIONS.with(|c| *c.borrow_mut() = Some(text.to_string()));
        call_global(
            "wrymeWriteFile",
            &[JsValue::from_str("stations.toml"), JsValue::from_str(text)],
        );
        Ok(())
    }

    pub fn can_write() -> bool {
        CONNECTED.with(|c| *c.borrow())
    }
}

pub fn shops_text() -> Option<String> {
    imp::shops_text()
}

pub fn stations_text() -> Option<String> {
    imp::stations_text()
}

pub fn write_stations(text: &str) -> Result<()> {
    imp::write_stations(text)
}

pub fn prefs_text() -> Option<String> {
    imp::prefs_text()
}

pub fn write_prefs(text: &str) -> Result<()> {
    imp::write_prefs(text)
}

#[cfg(target_arch = "wasm32")]
pub fn set_prefs_text(text: Option<String>) {
    imp::set_prefs_text(text)
}

#[cfg(target_arch = "wasm32")]
pub fn set_roster_text(text: Option<String>) {
    imp::set_roster_text(text)
}

#[cfg(target_arch = "wasm32")]
pub fn roster_text() -> Option<String> {
    imp::roster_text()
}

pub fn can_write() -> bool {
    imp::can_write()
}

#[cfg(target_arch = "wasm32")]
pub fn set_text(shops: Option<String>, stations: Option<String>, connected: bool) {
    imp::set_text(shops, stations, connected);
}
