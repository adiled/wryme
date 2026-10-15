use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::next_msg_id;

#[cfg(not(target_arch = "wasm32"))]
const FIXED_DOORWAY_PORT: u16 = 17834;

static VOLUNTEER: AtomicBool = AtomicBool::new(false);

#[cfg(not(target_arch = "wasm32"))]
type Conns = Arc<Mutex<HashMap<String, tokio::sync::mpsc::UnboundedSender<String>>>>;

#[cfg(not(target_arch = "wasm32"))]
fn next_web_id() -> String {
    let n = super::NEXT_MSG.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("w{n}")
}

#[cfg(not(target_arch = "wasm32"))]
fn doorway_port() -> u16 {
    std::env::var("WRYME_DOORWAY_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(FIXED_DOORWAY_PORT)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn spawn_volunteer() -> Option<std::thread::JoinHandle<()>> {
    if VOLUNTEER.swap(true, Ordering::SeqCst) {
        return None;
    }
    let port = doorway_port();
    Some(std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            VOLUNTEER.store(false, Ordering::SeqCst);
            return;
        };
        rt.block_on(async move {
            let Ok(tcp) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await
            else {
                VOLUNTEER.store(false, Ordering::SeqCst);
                return;
            };
            let Some(hubpath) = super::hub_path() else {
                VOLUNTEER.store(false, Ordering::SeqCst);
                return;
            };
            let _ = std::fs::remove_file(&hubpath);
            let Ok(unix) = tokio::net::UnixListener::bind(&hubpath) else {
                VOLUNTEER.store(false, Ordering::SeqCst);
                return;
            };
            let conns: Conns = Arc::new(Mutex::new(HashMap::new()));
            let c = conns.clone();
            tokio::spawn(async move { serve_unix(unix, c).await });
            serve_doorway(tcp, conns).await;
        });
    }))
}

#[cfg(not(target_arch = "wasm32"))]
fn roster(conns: &Conns) -> Vec<String> {
    let mut out: Vec<String> = conns
        .lock()
        .map(|c| c.keys().cloned().collect())
        .unwrap_or_default();
    out.sort();
    out
}

#[cfg(not(target_arch = "wasm32"))]
fn broadcast_roster(conns: &Conns) {
    let msg = serde_json::json!({"roster": roster(conns)}).to_string();
    if let Ok(c) = conns.lock() {
        for s in c.values() {
            let _ = s.send(msg.clone());
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn decorate(text: &str, reply_to: Option<&str>) -> String {
    match reply_to {
        Some(re) if !re.trim().is_empty() => format!("[re {re}] {text}"),
        _ => text.to_string(),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn route(conns: &Conns, v: &serde_json::Value) -> serde_json::Value {
    let (Some(to), Some(from), Some(text)) = (
        v.get("to").and_then(|t| t.as_str()),
        v.get("from").and_then(|f| f.as_str()),
        v.get("text").and_then(|t| t.as_str()),
    ) else {
        return serde_json::json!({"ok": false, "error": "bad note"});
    };
    if text.trim().is_empty() {
        return serde_json::json!({"ok": false, "error": "empty message"});
    }
    let id = v
        .get("id")
        .and_then(|i| i.as_str())
        .map(str::to_string)
        .unwrap_or_else(next_msg_id);
    let note = serde_json::json!({
        "from": from,
        "id": id,
        "text": decorate(text, v.get("reply_to").and_then(|r| r.as_str())),
    })
    .to_string();
    if let Ok(c) = conns.lock()
        && let Some(s) = c.get(to)
        && s.send(note).is_ok()
    {
        return serde_json::json!({"ok": true, "id": id});
    }
    serde_json::json!({"ok": false, "error": format!("no live window wryme#{to}")})
}

#[cfg(not(target_arch = "wasm32"))]
async fn serve_unix(listener: tokio::net::UnixListener, conns: Conns) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let conns = conns.clone();
        tokio::spawn(async move {
            let (r, mut w) = stream.into_split();
            let mut lines = tokio::io::BufReader::new(r).lines();
            let Ok(Some(first)) = lines.next_line().await else {
                return;
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&first) else {
                return;
            };
            if let Some(id) = v.get("hello").and_then(|h| h.as_str()) {
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
                if let Ok(mut c) = conns.lock() {
                    c.insert(id.to_string(), tx);
                }
                broadcast_roster(&conns);
                while let Some(msg) = rx.recv().await {
                    if w.write_all((msg + "\n").as_bytes()).await.is_err() {
                        break;
                    }
                }
                if let Ok(mut c) = conns.lock() {
                    c.remove(id);
                }
                broadcast_roster(&conns);
                return;
            }
            if v.get("list").and_then(|l| l.as_bool()).unwrap_or(false) {
                let reply = serde_json::json!({"roster": roster(&conns)}).to_string();
                let _ = w.write_all((reply + "\n").as_bytes()).await;
                return;
            }
            let ack = route(&conns, &v);
            let _ = w.write_all((ack.to_string() + "\n").as_bytes()).await;
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn handle_client_note(conns: &Conns, web_id: &str, v: &serde_json::Value) {
    let req = v.get("req").cloned();
    let mut note = v.clone();
    note["from"] = serde_json::json!(web_id);
    let mut ack = route(conns, &note);
    if let Some(req) = req {
        ack["req"] = req;
    }
    if let Ok(c) = conns.lock()
        && let Some(s) = c.get(web_id)
    {
        let _ = s.send(ack.to_string());
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn serve_doorway(listener: tokio::net::TcpListener, conns: Conns) {
    use futures_util::StreamExt;
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let conns = conns.clone();
        tokio::spawn(async move {
            let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
                return;
            };
            let (mut write, mut read) = ws.split();
            let (wtx, mut wrx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let mut web_id = next_web_id();
            if let Ok(mut c) = conns.lock() {
                c.insert(web_id.clone(), wtx.clone());
            }
            broadcast_roster(&conns);
            let hello = serde_json::json!({"hello": true, "you": web_id}).to_string();
            let _ = futures_util::SinkExt::send(
                &mut write,
                tokio_tungstenite::tungstenite::Message::Text(hello.into()),
            )
            .await;
            let pump = tokio::spawn(async move {
                use futures_util::SinkExt;
                use tokio_tungstenite::tungstenite::Message;
                while let Some(msg) = wrx.recv().await {
                    if write.send(Message::Text(msg.into())).await.is_err() {
                        break;
                    }
                }
            });
            use tokio_tungstenite::tungstenite::Message;
            while let Some(Ok(msg)) = read.next().await {
                let Message::Text(body) = msg else {
                    continue;
                };
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
                    continue;
                };
                if v.get("to").is_none()
                    && v.get("from").is_none()
                    && let Some(claim) = v.get("claim").and_then(|c| c.as_str())
                    && !claim.trim().is_empty()
                {
                    let free = conns
                        .lock()
                        .map(|c| !c.contains_key(claim))
                        .unwrap_or(false);
                    if free {
                        if let Ok(mut c) = conns.lock() {
                            c.remove(&web_id);
                            c.insert(claim.to_string(), wtx.clone());
                        }
                        web_id = claim.to_string();
                        broadcast_roster(&conns);
                        let hello =
                            serde_json::json!({"hello": true, "you": web_id}).to_string();
                        if let Ok(c) = conns.lock()
                            && let Some(s) = c.get(&web_id)
                        {
                            let _ = s.send(hello);
                        }
                    }
                    continue;
                }
                handle_client_note(&conns, &web_id, &v).await;
            }
            pump.abort();
            if let Ok(mut c) = conns.lock() {
                c.remove(&web_id);
            }
            broadcast_roster(&conns);
        });
    }
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    use super::super::*;
    use super::{serve_doorway, serve_unix};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    async fn next_note(
        lines: &mut tokio::io::Lines<tokio::io::BufReader<&mut tokio::net::UnixStream>>,
    ) -> serde_json::Value {
        loop {
            let line = tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line())
                .await
                .expect("timed out")
                .unwrap()
                .unwrap();
            let v: serde_json::Value = serde_json::from_str(&line).unwrap();
            if v.get("from").is_some() {
                return v;
            }
        }
    }
    async fn test_hub(dir: &std::path::Path) -> u16 {
        let unix = tokio::net::UnixListener::bind(dir.join("hub.sock")).unwrap();
        let tcp = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = tcp.local_addr().unwrap().port();
        let conns: super::Conns = Arc::new(Mutex::new(HashMap::new()));
        let c = conns.clone();
        tokio::spawn(async move { serve_unix(unix, c).await });
        tokio::spawn(async move { serve_doorway(tcp, conns).await });
        port
    }

    #[tokio::test]
    async fn hub_routes_native_to_native_with_reply() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let _g = TEST_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("wryme_hub_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        unsafe { std::env::set_var("WRYME_WINDOWS_DIR", &dir) };
        let _port = test_hub(&dir).await;
        let hub = dir.join("hub.sock");
        let mut reader = tokio::net::UnixStream::connect(&hub).await.unwrap();
        reader
            .write_all(b"{\"hello\":\"aa01\"}\n")
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let mut lines = tokio::io::BufReader::new(&mut reader).lines();
        let names = tokio::task::spawn_blocking(active).await.unwrap();
        assert!(names.contains(&"aa01".to_string()));
        let out = send("aa01", "bb02", "hello guest", "").await.unwrap();
        assert!(out.contains("aa01"));
        let v = next_note(&mut lines).await;
        assert_eq!(v.get("from").and_then(|f| f.as_str()), Some("bb02"));
        let id = v.get("id").and_then(|i| i.as_str()).unwrap().to_string();
        let out = send("aa01", "bb02", "got it", &id).await.unwrap();
        assert!(out.contains("aa01"));
        let line = next_note(&mut lines).await;
        assert!(line.to_string().contains(&format!("[re {id}]")));
        let err = send("zz99", "bb02", "hi", "").await.unwrap_err();
        assert!(err.contains("no live window"));
        unsafe { std::env::remove_var("WRYME_WINDOWS_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn hub_relays_web_note_to_native() {
        use futures_util::{SinkExt, StreamExt};
        use tokio::io::AsyncWriteExt;
        let _g = TEST_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("wryme_hubw_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        unsafe { std::env::set_var("WRYME_WINDOWS_DIR", &dir) };
        let port = test_hub(&dir).await;
        let hub = dir.join("hub.sock");
        let mut reader = tokio::net::UnixStream::connect(&hub).await.unwrap();
        reader
            .write_all(b"{\"hello\":\"cc01\"}\n")
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/"))
            .await
            .unwrap();
        let hello: serde_json::Value = match ws.next().await.unwrap().unwrap() {
            tokio_tungstenite::tungstenite::Message::Text(t) => serde_json::from_str(&t).unwrap(),
            _ => panic!("expected hello"),
        };
        let web_id = hello.get("you").and_then(|y| y.as_str()).unwrap().to_string();
        assert!(web_id.starts_with('w'));
        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::json!({"to": "cc01", "text": "knock knock"}).to_string().into(),
        ))
        .await
        .unwrap();
        use tokio::io::AsyncBufReadExt;
        let mut lines = tokio::io::BufReader::new(&mut reader).lines();
        let v = next_note(&mut lines).await;
        assert_eq!(v.get("from").and_then(|f| f.as_str()), Some(web_id.as_str()));
        assert_eq!(v.get("text").and_then(|t| t.as_str()), Some("knock knock"));
        unsafe { std::env::remove_var("WRYME_WINDOWS_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
    }
}
