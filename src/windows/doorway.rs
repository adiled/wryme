use std::sync::atomic::{AtomicBool, Ordering};

use super::{WebConns, current, next_msg_id, transmit};

#[cfg(not(target_arch = "wasm32"))]
const FIXED_DOORWAY_PORT: u16 = 17834;

static VOLUNTEER: AtomicBool = AtomicBool::new(false);

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
pub fn doorway_alive() -> bool {
    std::net::TcpStream::connect(("127.0.0.1", doorway_port())).is_ok()
}

#[cfg(not(target_arch = "wasm32"))]
fn doorway_id_path() -> Option<std::path::PathBuf> {
    super::dir().map(|d| d.join("doorway.id"))
}

#[cfg(not(target_arch = "wasm32"))]
fn roster_path() -> Option<std::path::PathBuf> {
    super::dir().map(|d| d.join("web.ids"))
}

#[cfg(not(target_arch = "wasm32"))]
pub fn roster_read() -> Vec<String> {
    let Some(p) = roster_path() else {
        return Vec::new();
    };
    std::fs::read_to_string(&p)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(not(target_arch = "wasm32"))]
fn roster_write(ids: &[String]) {
    if let Some(p) = roster_path() {
        let _ = std::fs::write(&p, ids.join("\n") + "\n");
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn roster_add(id: &str) {
    let mut ids = roster_read();
    if !ids.iter().any(|x| x == id) {
        ids.push(id.to_string());
        roster_write(&ids);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn roster_remove(id: &str) {
    let ids: Vec<String> = roster_read().into_iter().filter(|x| x != id).collect();
    roster_write(&ids);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn clear_volunteer_hint() {
    if let Some(p) = doorway_id_path() {
        let _ = std::fs::remove_file(&p);
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn volunteer_id() -> String {
    doorway_id_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

#[cfg(not(target_arch = "wasm32"))]
pub fn spawn_volunteer(conns: WebConns) -> Option<std::thread::JoinHandle<()>> {
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
            let Ok(listener) =
                tokio::net::TcpListener::bind(("127.0.0.1", port)).await
            else {
                VOLUNTEER.store(false, Ordering::SeqCst);
                return;
            };
            if let Some(p) = doorway_id_path() {
                let _ = std::fs::write(&p, current());
            }
            serve_doorway(listener, conns).await;
        });
    }))
}

#[cfg(not(target_arch = "wasm32"))]
pub fn volunteering() -> bool {
    VOLUNTEER.load(Ordering::SeqCst)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn stop_volunteer() {
    VOLUNTEER.store(false, Ordering::SeqCst);
    clear_volunteer_hint();
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn serve_doorway(
    listener: tokio::net::TcpListener,
    conns: WebConns,
) {
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
            let web_id = next_web_id();
            let (wtx, mut wrx) = tokio::sync::mpsc::unbounded_channel::<String>();
            if let Ok(mut c) = conns.lock() {
                c.insert(web_id.clone(), wtx);
            }
            roster_add(&web_id);
            let (mut write, mut read) = ws.split();
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
                let (Some(to), Some(text)) = (
                    v.get("to").and_then(|t| t.as_str()),
                    v.get("text").and_then(|t| t.as_str()),
                ) else {
                    continue;
                };
                let reply_to = v.get("reply_to").and_then(|r| r.as_str()).unwrap_or("");
                let id = next_msg_id();
                transmit(to, &web_id, text, reply_to, &id).await.ok();
            }
            pump.abort();
            if let Ok(mut c) = conns.lock() {
                c.remove(&web_id);
            }
            roster_remove(&web_id);
        });
    }
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    use super::super::*;
    use super::serve_doorway;

    #[tokio::test]
    async fn doorway_relays_web_note_into_mesh() {
        use futures_util::{SinkExt, StreamExt};
        let _g = TEST_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("wryme_door_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("WRYME_WINDOWS_DIR", &dir) };
        let conns = new_conns();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let c = conns.clone();
        tokio::spawn(async move { serve("cc01", tx, c).await });
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let c = conns.clone();
        tokio::spawn(async move { serve_doorway(listener, c).await });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
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
        let (from, _, text) = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");
        assert_eq!(from, web_id);
        assert_eq!(text, "knock knock");
        unsafe { std::env::remove_var("WRYME_WINDOWS_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
    }
}
