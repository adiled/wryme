use std::collections::BTreeMap;

pub type Memory = BTreeMap<String, String>;

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use super::Memory;
    use std::ffi::OsString;
    use std::path::PathBuf;

    pub(super) fn path_from(xdg_state: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
        let base = xdg_state
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home.map(|h| PathBuf::from(h).join(".local").join("state")))?;
        Some(base.join("wryme").join("shop-memory.json"))
    }

    fn path() -> Option<PathBuf> {
        path_from(std::env::var_os("XDG_STATE_HOME"), std::env::var_os("HOME"))
    }

    pub fn load() -> Memory {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn remember(model: &str, shop: &str) {
        let Some(path) = path() else { return };
        let mut memory = load();
        if memory.get(model).is_some_and(|s| s == shop) {
            return;
        }
        memory.insert(model.to_string(), shop.to_string());
        let written = path
            .parent()
            .map(std::fs::create_dir_all)
            .unwrap_or(Ok(()))
            .and_then(|_| {
                let tmp = path.with_extension(format!("json.{}", std::process::id()));
                let text = serde_json::to_string_pretty(&memory).unwrap_or_default();
                std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &path))
            });
        if let Err(e) = written {
            tracing::warn!(path = %path.display(), err = %e, "shop memory not saved");
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use super::Memory;

    pub fn load() -> Memory {
        Memory::new()
    }

    pub fn remember(_model: &str, _shop: &str) {}
}

pub use imp::{load, remember};

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::imp::path_from;
    use std::path::PathBuf;

    #[test]
    fn lives_in_xdg_state_home_or_local_state() {
        assert_eq!(
            path_from(Some("/xs".into()), Some("/h".into())),
            Some(PathBuf::from("/xs/wryme/shop-memory.json"))
        );
        assert_eq!(
            path_from(None, Some("/h".into())),
            Some(PathBuf::from("/h/.local/state/wryme/shop-memory.json"))
        );
        assert_eq!(
            path_from(Some("relative".into()), Some("/h".into())),
            Some(PathBuf::from("/h/.local/state/wryme/shop-memory.json"))
        );
        assert_eq!(path_from(None, None), None);
    }
}
