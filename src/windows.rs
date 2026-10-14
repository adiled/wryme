#[cfg(not(target_arch = "wasm32"))]
use std::collections::hash_map::DefaultHasher;
#[cfg(not(target_arch = "wasm32"))]
use std::hash::{Hash, Hasher};

#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

#[cfg(not(target_arch = "wasm32"))]
const TEXT_CAP: usize = 4000;

static CURRENT: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
static NEXT_MSG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

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
    return "web".to_string();
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
fn dir() -> Option<PathBuf> {
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
fn sock_path(id: &str) -> Option<PathBuf> {
    dir().map(|d| d.join(format!("{id}.sock")))
}

pub fn register(id: &str) {
    set_current(id);
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(d) = dir() {
            let _ = std::fs::create_dir_all(&d);
        }
        let _ = try_bind(id);
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = id;
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn try_bind(id: &str) -> Option<tokio::net::UnixListener> {
    let path = sock_path(id)?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if path.exists() && std::os::unix::net::UnixStream::connect(&path).is_err() {
        let _ = std::fs::remove_file(&path);
    }
    tokio::net::UnixListener::bind(&path).ok()
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn serve(id: &str, tx: tokio::sync::mpsc::UnboundedSender<(String, String, String)>) {
    use tokio::io::AsyncBufReadExt;
    let Some(listener) = try_bind(id) else {
        return;
    };
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(stream).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
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
                let id = next_msg_id();
                let mut text = text.to_string();
                if let Some(re) = v.get("reply_to").and_then(|r| r.as_str())
                    && !re.trim().is_empty()
                {
                    text = format!("[re {re}] {text}");
                }
                let _ = tx.send((from.to_string(), id, text));
            }
        });
    }
}

pub fn unregister(id: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(p) = sock_path(id) {
            let _ = std::fs::remove_file(&p);
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = id;
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn probe_live(path: &PathBuf) -> bool {
    if std::os::unix::net::UnixStream::connect(path).is_ok() {
        return true;
    }
    let _ = std::fs::remove_file(path);
    false
}

pub fn active() -> Vec<String> {
    #[cfg(target_arch = "wasm32")]
    return Vec::new();
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(d) = dir() else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir(&d) else {
            return Vec::new();
        };
        let mut out: Vec<String> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("sock"))
            .filter(probe_live)
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

#[cfg(not(target_arch = "wasm32"))]
pub async fn send(to: &str, from: &str, text: &str, reply_to: &str) -> Result<String, String> {
    use tokio::io::AsyncWriteExt;
    let text: String = text.chars().take(TEXT_CAP).collect();
    if text.trim().is_empty() {
        return Err("empty message".to_string());
    }
    let Some(path) = sock_path(to) else {
        return Err("no window dir".to_string());
    };
    let mut stream = match tokio::net::UnixStream::connect(&path).await {
        Ok(s) => s,
        Err(_) => {
            let _ = std::fs::remove_file(&path);
            return Err(format!("no live window wryme#{to}"));
        }
    };
    let mut note = serde_json::json!({"from": from, "text": text});
    if !reply_to.trim().is_empty() {
        note["reply_to"] = serde_json::json!(reply_to);
    }
    let line = note.to_string() + "\n";
    match stream.write_all(line.as_bytes()).await {
        Ok(()) => Ok(format!("sent to wryme#{to}")),
        Err(e) => Err(format!("delivery failed: {e}")),
    }
}

#[cfg(target_arch = "wasm32")]
pub async fn send(to: &str, from: &str, text: &str, reply_to: &str) -> Result<String, String> {
    let _ = (to, from, text, reply_to);
    Err("window messaging is native only".to_string())
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
    async fn socket_roundtrip_delivers_guest_note() {
        let dir = std::env::temp_dir().join(format!("wryme_win_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("WRYME_WINDOWS_DIR", &dir) };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move { serve("aa01", tx).await });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let out = send("aa01", "bb02", "hello guest", "").await.unwrap();
        assert!(out.contains("aa01"));
        let (from, id, text) = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");
        assert_eq!(from.as_str(), "bb02");
        assert!(id.starts_with('m'));
        assert_eq!(text.as_str(), "hello guest");
        let out = send("aa01", "bb02", "got it", &id).await.unwrap();
        assert!(out.contains("aa01"));
        let (_, _, text) = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");
        assert!(text.contains(&format!("[re {id}]")));
        assert!(send("zz99", "bb02", "hi", "").await.is_err());
        unsafe { std::env::remove_var("WRYME_WINDOWS_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
    }
}
