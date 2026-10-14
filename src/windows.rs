#[cfg(not(target_arch = "wasm32"))]
pub mod hub;

#[cfg(not(target_arch = "wasm32"))]
use std::collections::hash_map::DefaultHasher;
#[cfg(not(target_arch = "wasm32"))]
use std::hash::{Hash, Hasher};

#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(not(target_arch = "wasm32"))]
const TEXT_CAP: usize = 4000;

static CURRENT: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
#[cfg(not(target_arch = "wasm32"))]
pub(crate) static NEXT_MSG: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(1);
#[cfg(target_arch = "wasm32")]
static NEXT_MSG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn next_msg_id() -> String {
    let n = NEXT_MSG.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("m{n}")
}

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

pub fn tag(id: &str) -> String {
    if id == "web" {
        "wryme".to_string()
    } else {
        format!("wryme#{id}")
    }
}

pub fn mint() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let r = js_sys::Math::random() * 65536.0;
        return format!("{:04x}", r as u32 & 0xffff);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let mut h = DefaultHasher::new();
        (std::process::id(), now).hash(&mut h);
        format!("{:04x}", h.finish() & 0xffff)
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn dir() -> Option<PathBuf> {
    if let Ok(custom) = std::env::var("WRYME_WINDOWS_DIR")
        && !custom.is_empty()
    {
        return Some(PathBuf::from(custom));
    }
    std::env::var("HOME").ok().map(|h| {
        PathBuf::from(h)
            .join(".config")
            .join("wryme")
            .join("windows")
    })
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn hub_path() -> Option<PathBuf> {
    dir().map(|d| d.join("hub.sock"))
}

pub fn register(id: &str) {
    set_current(id);
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(d) = dir() {
            let _ = std::fs::create_dir_all(&d);
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = id;
    }
}

pub fn unregister(_id: &str) {}

#[cfg(not(target_arch = "wasm32"))]
pub fn ensure_reader(
    id: &str,
    tx: tokio::sync::mpsc::UnboundedSender<(String, String, String)>,
) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let id = id.to_string();
    std::thread::spawn(move || {
        use std::io::{BufRead, Write};
        loop {
            if let Some(path) = hub_path()
                && let Ok(stream) = std::os::unix::net::UnixStream::connect(&path)
            {
                let mut writer = stream.try_clone().ok();
                if let Some(w) = writer.as_mut() {
                    let _ = w.write_all(format!("{{\"hello\":\"{id}\"}}\n").as_bytes());
                }
                let reader = std::io::BufReader::new(stream);
                for line in reader.lines() {
                    let Ok(line) = line else {
                        break;
                    };
                    let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                        continue;
                    };
                    let (Some(from), Some(text)) = (
                        v.get("from").and_then(|f| f.as_str()),
                        v.get("text").and_then(|t| t.as_str()),
                    ) else {
                        continue;
                    };
                    if text.trim().is_empty() {
                        continue;
                    }
                    let mid = v
                        .get("id")
                        .and_then(|i| i.as_str())
                        .map(str::to_string)
                        .unwrap_or_else(next_msg_id);
                    let text = match v.get("reply_to").and_then(|r| r.as_str()) {
                        Some(re) if !re.trim().is_empty() => {
                            format!("[re {re}] {text}")
                        }
                        _ => text.to_string(),
                    };
                    let _ = tx.send((from.to_string(), mid, text));
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    });
}

pub fn active() -> Vec<String> {
    #[cfg(target_arch = "wasm32")]
    {
        let mut out: Vec<String> = crate::config::roster_text()
            .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok())
            .unwrap_or_default();
        out.sort();
        return out;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::io::{BufRead, Write};
        let Some(path) = hub_path() else {
            return Vec::new();
        };
        let Ok(stream) = std::os::unix::net::UnixStream::connect(&path) else {
            return Vec::new();
        };
        let _ = stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)));
        let mut stream = stream;
        if stream.write_all(b"{\"list\":true}\n").is_err() {
            return Vec::new();
        }
        let mut line = String::new();
        if std::io::BufReader::new(&stream).read_line(&mut line).is_err() {
            return Vec::new();
        }
        let mut out: Vec<String> = serde_json::from_str::<serde_json::Value>(&line)
            .ok()
            .and_then(|v| v.get("roster").cloned())
            .and_then(|r| serde_json::from_value(r).ok())
            .unwrap_or_default();
        out.sort();
        out
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn send(to: &str, from: &str, text: &str, reply_to: &str) -> Result<String, String> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let text: String = text.chars().take(TEXT_CAP).collect();
    if text.trim().is_empty() {
        return Err("empty message".to_string());
    }
    let Some(path) = hub_path() else {
        return Err("no window dir".to_string());
    };
    let mut stream = tokio::net::UnixStream::connect(&path)
        .await
        .map_err(|_| "mesh down, no hub".to_string())?;
    let mut note = serde_json::json!({"from": from, "to": to, "id": next_msg_id(), "text": text});
    if !reply_to.trim().is_empty() {
        note["reply_to"] = serde_json::json!(reply_to);
    }
    stream
        .write_all((note.to_string() + "\n").as_bytes())
        .await
        .map_err(|e| format!("delivery failed: {e}"))?;
    let mut line = String::new();
    tokio::io::BufReader::new(&mut stream)
        .read_line(&mut line)
        .await
        .map_err(|e| format!("delivery failed: {e}"))?;
    let v: serde_json::Value =
        serde_json::from_str(&line).map_err(|_| "bad hub reply".to_string())?;
    if v.get("ok").and_then(|o| o.as_bool()).unwrap_or(false) {
        return Ok(format!("sent to wryme#{to}"));
    }
    Err(v
        .get("error")
        .and_then(|e| e.as_str())
        .unwrap_or("delivery failed")
        .to_string())
}

#[cfg(target_arch = "wasm32")]
pub async fn send(to: &str, from: &str, text: &str, reply_to: &str) -> Result<String, String> {
    use wasm_bindgen::JsCast;
    let text: String = text.chars().take(4000).collect();
    if text.trim().is_empty() {
        return Err("empty message".to_string());
    }
    let global = js_sys::global();
    let Ok(f) = js_sys::Reflect::get(&global, &wasm_bindgen::JsValue::from_str("wrymeMeshSend"))
    else {
        return Err("no doorway connection".to_string());
    };
    let Some(f) = f.dyn_ref::<js_sys::Function>() else {
        return Err("no doorway connection".to_string());
    };
    let mut note = serde_json::json!({"to": to, "from": from, "text": text});
    if !reply_to.trim().is_empty() {
        note["reply_to"] = serde_json::json!(reply_to);
    }
    let arg = match serde_json::to_string(&note) {
        Ok(s) => s,
        Err(_) => return Err("could not encode note".to_string()),
    };
    let promise = match f.call1(&global, &wasm_bindgen::JsValue::from_str(&arg)) {
        Ok(p) => p,
        Err(_) => return Err("doorway send failed".to_string()),
    };
    let answer = match wasm_bindgen_futures::JsFuture::from(js_sys::Promise::from(promise)).await {
        Ok(v) => v.as_string().unwrap_or_default(),
        Err(_) => return Err("doorway send failed".to_string()),
    };
    let v: serde_json::Value = match serde_json::from_str(&answer) {
        Ok(v) => v,
        Err(_) => return Err("bad doorway reply".to_string()),
    };
    if v.get("ok").and_then(|o| o.as_bool()).unwrap_or(false) {
        return Ok(format!("sent to wryme#{to}"));
    }
    Err(v
        .get("error")
        .and_then(|e| e.as_str())
        .unwrap_or("delivery failed")
        .to_string())
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

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn send_fails_without_hub() {
        let _g = TEST_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("wryme_nohub_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        unsafe { std::env::set_var("WRYME_WINDOWS_DIR", &dir) };
        let err = send("zz99", "bb02", "hi", "").await.unwrap_err();
        assert!(err.contains("mesh down"));
        assert!(active().is_empty());
        unsafe { std::env::remove_var("WRYME_WINDOWS_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
    }
}
