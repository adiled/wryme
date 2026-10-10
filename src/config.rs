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
        static CONNECTED: RefCell<bool> = const { RefCell::new(false) };
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

pub fn can_write() -> bool {
    imp::can_write()
}

#[cfg(target_arch = "wasm32")]
pub fn set_text(shops: Option<String>, stations: Option<String>, connected: bool) {
    imp::set_text(shops, stations, connected);
}
