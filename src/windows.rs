#[cfg(not(target_arch = "wasm32"))]
use std::collections::hash_map::DefaultHasher;
#[cfg(not(target_arch = "wasm32"))]
use std::hash::{Hash, Hasher};
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
#[cfg(not(target_arch = "wasm32"))]
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(not(target_arch = "wasm32"))]
const LIVE_SECS: u64 = 20;
#[cfg(not(target_arch = "wasm32"))]
const INBOX_CAP: usize = 50;
#[cfg(not(target_arch = "wasm32"))]
const TEXT_CAP: usize = 4000;

#[cfg(not(target_arch = "wasm32"))]
fn dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(|h| {
        PathBuf::from(h)
            .join(".config")
            .join("wryme")
            .join("windows")
    })
}

static CURRENT: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub fn set_current(id: &str) {
    if let Ok(mut c) = CURRENT.lock() {
        *c = Some(id.to_string());
    }
}

pub fn current() -> String {
    CURRENT
        .lock()
        .ok()
        .and_then(|c| c.clone())
        .unwrap_or_else(|| "web".to_string())
}

pub fn mint() -> String {
    #[cfg(target_arch = "wasm32")]
    return "web".to_string();
    #[cfg(not(target_arch = "wasm32"))]
    {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let mut h = DefaultHasher::new();
        (std::process::id(), now).hash(&mut h);
        format!("{:04x}", h.finish() & 0xffff)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn presence_path(id: &str) -> Option<PathBuf> {
    dir().map(|d| d.join(format!("{id}.presence")))
}

#[cfg(not(target_arch = "wasm32"))]
fn inbox_path(id: &str) -> Option<PathBuf> {
    dir().map(|d| d.join(format!("{id}.inbox")))
}

pub fn register(id: &str) {
    set_current(id);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(d) = dir() else { return };
        let _ = std::fs::create_dir_all(&d);
        touch(id);
        prune();
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = id;
    }
}

pub fn touch(id: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(p) = presence_path(id) else {
            return;
        };
        let pid = std::process::id();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = std::fs::write(&p, format!("{pid} {now}"));
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = id;
    }
}

pub fn unregister(id: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(p) = presence_path(id) {
            let _ = std::fs::remove_file(&p);
        }
        if let Some(p) = inbox_path(id) {
            let _ = std::fs::remove_file(&p);
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = id;
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn live(path: &PathBuf) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    SystemTime::now()
        .duration_since(modified)
        .map(|d| d.as_secs() < LIVE_SECS)
        .unwrap_or(false)
}

#[cfg(not(target_arch = "wasm32"))]
fn prune() {
    let Some(d) = dir() else { return };
    let Ok(entries) = std::fs::read_dir(&d) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) == Some("presence") && !live(&p) {
            let _ = std::fs::remove_file(&p);
        }
    }
}

pub fn active() -> Vec<String> {
    #[cfg(target_arch = "wasm32")]
    return Vec::new();
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(d) = dir() else {
            return Vec::new();
        };
        prune();
        let Ok(entries) = std::fs::read_dir(&d) else {
            return Vec::new();
        };
        let mut out: Vec<String> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension().and_then(|x| x.to_str()) == Some("presence") && live(p)
            })
            .filter_map(|p| {
                p.file_stem()
                    .and_then(|s| s.to_str())
                    .map(str::to_string)
            })
            .collect();
        out.sort();
        out
    }
}

pub fn send(to: &str, from: &str, text: &str) -> Result<String, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (to, from, text);
        return Err("window messaging is native only".to_string());
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if !active().iter().any(|id| id == to) {
            return Err(format!("no live window wryme#{to}"));
        }
        let Some(path) = inbox_path(to) else {
            return Err("no window dir".to_string());
        };
        let text: String = text.chars().take(TEXT_CAP).collect();
        if text.trim().is_empty() {
            return Err("empty message".to_string());
        }
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let line = serde_json::json!({"from": from, "text": text, "at": at}).to_string();
        let mut existing: Vec<String> = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect();
        existing.push(line);
        if existing.len() > INBOX_CAP {
            existing = existing.split_off(existing.len() - INBOX_CAP);
        }
        match std::fs::write(&path, existing.join("\n") + "\n") {
            Ok(()) => Ok(format!("sent to wryme#{to}")),
            Err(e) => Err(format!("inbox write failed: {e}")),
        }
    }
}

pub fn claim(id: &str) -> Vec<(String, String)> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = id;
        return Vec::new();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(path) = inbox_path(id) else {
            return Vec::new();
        };
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Vec::new();
        };
        let _ = std::fs::remove_file(&path);
        raw.lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter_map(|v| {
                let from = v.get("from")?.as_str()?.to_string();
                let text = v.get("text")?.as_str()?.to_string();
                Some((from, text))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_is_short_hex() {
        let id = mint();
        assert!(!id.is_empty());
        assert!(id.len() <= 4 || cfg!(target_arch = "wasm32"));
    }
}
