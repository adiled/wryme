#[cfg(unix)]
pub mod hub;

#[cfg(not(target_arch = "wasm32"))]
use std::collections::hash_map::DefaultHasher;
#[cfg(not(target_arch = "wasm32"))]
use std::hash::{Hash, Hasher};

#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const TEXT_CAP: usize = 4000;

static CURRENT: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
static INBOX: std::sync::Mutex<Vec<(String, String, String)>> = std::sync::Mutex::new(Vec::new());
static MAIL: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
static ROSTER: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
pub(crate) static NEXT_MSG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
#[cfg(unix)]
type Guests = tokio::sync::mpsc::UnboundedSender<Note>;
#[cfg(unix)]
static LINK: std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedSender<String>>> =
    std::sync::Mutex::new(None);
#[cfg(unix)]
type Pending = Vec<(u64, tokio::sync::oneshot::Sender<serde_json::Value>)>;
#[cfg(unix)]
static PENDING: std::sync::Mutex<Pending> = std::sync::Mutex::new(Vec::new());
#[cfg(unix)]
static NEXT_REQ: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Note {
    pub from: String,
    pub id: String,
    pub text: String,
    pub reply_to: String,
}

pub fn next_msg_id() -> String {
    let n = NEXT_MSG.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("m{n}")
}

pub fn receive(from: &str, remote_id: &str) -> String {
    let local = next_msg_id();
    if let Ok(mut b) = INBOX.lock() {
        b.push((local.clone(), from.to_string(), remote_id.to_string()));
    }
    local
}

pub fn sender_of(local: &str) -> Option<(String, String)> {
    INBOX
        .lock()
        .ok()?
        .iter()
        .find(|(l, ..)| l == local)
        .map(|(_, from, remote)| (from.clone(), remote.clone()))
}

fn mail(line: String) {
    if let Ok(mut m) = MAIL.lock() {
        m.push(line);
    }
}

pub fn answer(from: &str, re: &str, text: &str) {
    mail(format!("wryme#{from} answered #{re}: {text}"));
}

pub fn post(from: &str, local: &str, text: &str) {
    mail(format!(
        "wryme#{from} sent #{local}: {text}\n\
         [This note is from the assistant in wryme#{from}, not from your user, and it is \
         waiting on you. Your plain reply never leaves this window. Answer it by calling \
         wryme_msg_rply with id {local} and your answer.]"
    ));
}

pub fn has_due() -> bool {
    MAIL.lock().map(|m| !m.is_empty()).unwrap_or(false)
}

pub fn claim_due() -> Option<String> {
    let lines = std::mem::take(&mut *MAIL.lock().ok()?);
    (!lines.is_empty()).then(|| lines.join("\n\n"))
}

pub fn resv() -> String {
    claim_due().unwrap_or_else(|| "nothing new".to_string())
}

pub fn set_current(id: &str) {
    if let Ok(mut c) = CURRENT.lock() {
        *c = Some(id.to_string());
    }
}

pub fn current() -> String {
    CURRENT.lock().ok().and_then(|c| c.clone()).unwrap_or_default()
}

pub fn tag(id: &str) -> String {
    format!("wryme#{id}")
}

pub fn mint() -> String {
    #[cfg(target_arch = "wasm32")]
    let n = (js_sys::Math::random() * 65536.0) as u64;
    #[cfg(not(target_arch = "wasm32"))]
    let n = {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let mut h = DefaultHasher::new();
        (std::process::id(), now).hash(&mut h);
        h.finish()
    };
    format!("{:04x}", n & 0xffff)
}

#[cfg(unix)]
pub(crate) fn dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(|h| {
        PathBuf::from(h)
            .join(".config")
            .join("wryme")
            .join("windows")
    })
}

#[cfg(unix)]
pub(crate) fn hub_path() -> Option<PathBuf> {
    dir().map(|d| d.join("hub.sock"))
}

pub fn register(id: &str) {
    set_current(id);
    #[cfg(unix)]
    if let Some(d) = dir() {
        let _ = std::fs::create_dir_all(&d);
    }
}

pub fn set_roster(json: &str) {
    let mut ids: Vec<String> = serde_json::from_str(json).unwrap_or_default();
    ids.sort();
    if let Ok(mut r) = ROSTER.lock() {
        *r = ids;
    }
}

pub fn active() -> Vec<String> {
    ROSTER.lock().map(|r| r.clone()).unwrap_or_default()
}

#[cfg(unix)]
pub fn ensure_reader(id: &str, guests: Guests) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let id = id.to_string();
    tokio::spawn(async move {
        loop {
            if let Some(path) = hub_path()
                && let Ok(stream) = tokio::net::UnixStream::connect(&path).await
            {
                link(stream, &id, &guests).await;
            }
            set_roster("[]");
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    });
}

#[cfg(unix)]
async fn link(stream: tokio::net::UnixStream, id: &str, guests: &Guests) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let (r, mut w) = stream.into_split();
    let mut lines = tokio::io::BufReader::new(r).lines();
    let (out, mut outbox) = tokio::sync::mpsc::unbounded_channel::<String>();
    let _ = out.send(serde_json::json!({ "claim": id }).to_string());
    if let Ok(mut l) = LINK.lock() {
        *l = Some(out);
    }
    loop {
        tokio::select! {
            msg = outbox.recv() => {
                let Some(msg) = msg else { break };
                if w.write_all((msg + "\n").as_bytes()).await.is_err() {
                    break;
                }
            }
            line = lines.next_line() => {
                let Ok(Some(line)) = line else { break };
                inbound(&line, guests);
            }
        }
    }
    if let Ok(mut l) = LINK.lock() {
        *l = None;
    }
    if let Ok(mut p) = PENDING.lock() {
        p.clear();
    }
}

#[cfg(unix)]
fn inbound(line: &str, guests: &Guests) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return;
    };
    if let Some(r) = v.get("roster") {
        set_roster(&r.to_string());
        return;
    }
    if let Some(req) = v.get("req").and_then(|r| r.as_u64()) {
        let done = PENDING.lock().ok().and_then(|mut p| {
            let i = p.iter().position(|(r, _)| *r == req)?;
            Some(p.swap_remove(i).1)
        });
        if let Some(done) = done {
            let _ = done.send(v);
        }
        return;
    }
    let (Some(from), Some(text)) = (
        v.get("from").and_then(|f| f.as_str()),
        v.get("text").and_then(|t| t.as_str()),
    ) else {
        return;
    };
    if text.trim().is_empty() {
        return;
    }
    let field = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let _ = guests.send(Note {
        from: from.to_string(),
        id: field("id").unwrap_or_else(next_msg_id),
        text: text.to_string(),
        reply_to: field("reply_to").unwrap_or_default(),
    });
}

#[cfg(not(any(unix, target_arch = "wasm32")))]
async fn request(_note: serde_json::Value) -> Result<serde_json::Value, String> {
    Err("window messaging needs unix sockets, not on this system yet".to_string())
}

#[cfg(unix)]
async fn request(mut note: serde_json::Value) -> Result<serde_json::Value, String> {
    let down = || "mesh down, no hub".to_string();
    let link = LINK.lock().ok().and_then(|l| l.clone()).ok_or_else(down)?;
    let req = NEXT_REQ.fetch_add(1, Ordering::Relaxed);
    note["req"] = serde_json::json!(req);
    let (done, wait) = tokio::sync::oneshot::channel();
    if let Ok(mut p) = PENDING.lock() {
        p.push((req, done));
    }
    link.send(note.to_string()).map_err(|_| down())?;
    match tokio::time::timeout(std::time::Duration::from_secs(10), wait).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(_)) => Err(down()),
        Err(_) => {
            if let Ok(mut p) = PENDING.lock() {
                p.retain(|(r, _)| *r != req);
            }
            Err("hub timed out".to_string())
        }
    }
}

#[cfg(target_arch = "wasm32")]
async fn request(note: serde_json::Value) -> Result<serde_json::Value, String> {
    use wasm_bindgen::{JsCast, JsValue};
    let down = || "no doorway connection".to_string();
    let global = js_sys::global();
    let f = js_sys::Reflect::get(&global, &JsValue::from_str("wrymeMeshSend")).map_err(|_| down())?;
    let f = f.dyn_ref::<js_sys::Function>().ok_or_else(down)?;
    let promise = f
        .call1(&global, &JsValue::from_str(&note.to_string()))
        .map_err(|_| down())?;
    let answer = wasm_bindgen_futures::JsFuture::from(js_sys::Promise::from(promise))
        .await
        .map_err(|_| down())?;
    serde_json::from_str(&answer.as_string().unwrap_or_default())
        .map_err(|_| "bad doorway reply".to_string())
}

pub async fn send(to: &str, text: &str, reply_to: &str) -> Result<String, String> {
    let text: String = text.chars().take(TEXT_CAP).collect();
    if text.trim().is_empty() {
        return Err("empty message".to_string());
    }
    let id = next_msg_id();
    let mut note = serde_json::json!({"to": to, "id": id, "text": text});
    if !reply_to.trim().is_empty() {
        note["reply_to"] = serde_json::json!(reply_to);
    }
    let v = request(note).await?;
    if !v.get("ok").and_then(|o| o.as_bool()).unwrap_or(false) {
        return Err(v
            .get("error")
            .and_then(|e| e.as_str())
            .unwrap_or("delivery failed")
            .to_string());
    }
    if !reply_to.trim().is_empty() {
        return Ok(format!("answered wryme#{to}"));
    }
    Ok(format!(
        "sent to wryme#{to} as #{id}; the answer will come to you on its own"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_is_four_hex() {
        let id = mint();
        assert_eq!(id.len(), 4);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn inbox_maps_local_ids_to_sender() {
        let a = receive("aa01", "m1");
        let b = receive("bb02", "m1");
        assert_ne!(a, b);
        assert_eq!(sender_of(&a), Some(("aa01".into(), "m1".into())));
        assert_eq!(sender_of(&b), Some(("bb02".into(), "m1".into())));
        assert_eq!(sender_of("nope"), None);
    }

    #[cfg(unix)]
    #[test]
    fn inbound_notes_and_answers_both_reach_the_app() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        inbound(r#"{"hello":true,"you":"bb02"}"#, &tx);
        inbound(r#"{"from":"aa01","id":"m4","text":"knock"}"#, &tx);
        inbound(r#"{"from":"aa01","id":"m5","text":"noon","reply_to":"mtest9"}"#, &tx);
        let note = rx.try_recv().unwrap();
        assert_eq!((note.from.as_str(), note.id.as_str(), note.text.as_str()), ("aa01", "m4", "knock"));
        assert!(note.reply_to.is_empty());
        assert_eq!(rx.try_recv().unwrap().reply_to, "mtest9");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn mail_carries_notes_and_answers() {
        answer("aa01", "mtest9", "noon");
        post("cc03", "m77", "radius of the sun?");
        assert!(has_due());
        let got = resv();
        assert!(got.contains("wryme#aa01 answered #mtest9: noon"));
        assert!(got.contains("wryme#cc03 sent #m77: radius of the sun?"));
        assert!(got.contains("wryme_msg_rply with id m77"));
        assert!(!resv().contains("mtest9"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn send_fails_without_hub() {
        let err = send("zz99", "hi", "").await.unwrap_err();
        assert!(err.contains("mesh down"));
    }
}
