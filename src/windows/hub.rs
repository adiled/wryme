use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::{Sink, SinkExt, Stream, StreamExt};
use tokio::sync::mpsc::UnboundedSender;

use super::next_msg_id;

const DOORWAY_PORT: u16 = 9630;
const PAGES_ORIGIN: &str = "https://adiled.github.io";

static VOLUNTEER: AtomicBool = AtomicBool::new(false);

type Conns = Arc<Mutex<HashMap<String, UnboundedSender<String>>>>;

fn temp_id() -> String {
    let n = super::NEXT_MSG.fetch_add(1, Ordering::Relaxed);
    format!("w{n}")
}

pub fn spawn_volunteer() -> Option<std::thread::JoinHandle<()>> {
    if VOLUNTEER.swap(true, Ordering::SeqCst) {
        return None;
    }
    Some(std::thread::spawn(move || {
        if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            rt.block_on(serve(DOORWAY_PORT));
        }
        VOLUNTEER.store(false, Ordering::SeqCst);
    }))
}

async fn serve(port: u16) {
    let Ok(tcp) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await else {
        return;
    };
    let Some(hubpath) = super::hub_path() else {
        return;
    };
    let _ = std::fs::remove_file(&hubpath);
    let Ok(unix) = tokio::net::UnixListener::bind(&hubpath) else {
        return;
    };
    let conns: Conns = Arc::default();
    tokio::spawn(serve_unix(unix, conns.clone()));
    serve_doorway(tcp, conns).await;
}

fn roster(conns: &Conns) -> Vec<String> {
    let mut out: Vec<String> = conns
        .lock()
        .map(|c| c.keys().cloned().collect())
        .unwrap_or_default();
    out.sort();
    out
}

fn broadcast_roster(conns: &Conns) {
    let msg = serde_json::json!({"roster": roster(conns)}).to_string();
    if let Ok(c) = conns.lock() {
        for s in c.values() {
            let _ = s.send(msg.clone());
        }
    }
}

fn hello(id: &str) -> String {
    serde_json::json!({"hello": true, "you": id}).to_string()
}

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
    let mut note = serde_json::json!({"from": from, "id": id, "text": text});
    if let Some(re) = v.get("reply_to").and_then(|r| r.as_str())
        && !re.trim().is_empty()
    {
        note["reply_to"] = serde_json::json!(re);
    }
    if let Ok(c) = conns.lock()
        && let Some(s) = c.get(to)
        && s.send(note.to_string()).is_ok()
    {
        return serde_json::json!({"ok": true, "id": id});
    }
    serde_json::json!({"ok": false, "error": format!("no live window wryme#{to}")})
}

fn claim(conns: &Conns, id: &mut String, tx: &UnboundedSender<String>, want: &str) {
    let want = want.trim();
    let took = !want.is_empty()
        && want != id.as_str()
        && conns
            .lock()
            .map(|mut c| {
                if c.contains_key(want) {
                    return false;
                }
                c.remove(id.as_str());
                c.insert(want.to_string(), tx.clone());
                true
            })
            .unwrap_or(false);
    if took {
        *id = want.to_string();
        broadcast_roster(conns);
    }
    let _ = tx.send(hello(id));
}

async fn session<I, O>(conns: Conns, mut incoming: I, mut outgoing: O)
where
    I: Stream<Item = String> + Unpin,
    O: Sink<String> + Unpin,
{
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let mut id = temp_id();
    if let Ok(mut c) = conns.lock() {
        c.insert(id.clone(), tx.clone());
    }
    broadcast_roster(&conns);
    let _ = tx.send(hello(&id));
    loop {
        tokio::select! {
            out = rx.recv() => {
                let Some(out) = out else { break };
                if outgoing.send(out).await.is_err() {
                    break;
                }
            }
            line = incoming.next() => {
                let Some(line) = line else { break };
                let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                if let Some(want) = v.get("claim").and_then(|c| c.as_str()) {
                    let want = want.to_string();
                    claim(&conns, &mut id, &tx, &want);
                    continue;
                }
                let req = v.get("req").cloned();
                v["from"] = serde_json::json!(id);
                let mut ack = route(&conns, &v);
                if let Some(req) = req {
                    ack["req"] = req;
                }
                let _ = tx.send(ack.to_string());
            }
        }
    }
    let gone = conns
        .lock()
        .map(|mut c| {
            let ours = c.get(&id).is_some_and(|s| s.same_channel(&tx));
            if ours {
                c.remove(&id);
            }
            ours
        })
        .unwrap_or(false);
    if gone {
        broadcast_roster(&conns);
    }
}

async fn serve_unix(listener: tokio::net::UnixListener, conns: Conns) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            continue;
        };
        let conns = conns.clone();
        tokio::spawn(async move {
            let (r, w) = stream.into_split();
            let lines = tokio::io::BufReader::new(r).lines();
            let incoming = futures_util::stream::unfold(lines, |mut l| async move {
                l.next_line().await.ok().flatten().map(|s| (s, l))
            });
            let outgoing = futures_util::sink::unfold(w, |mut w, msg: String| async move {
                w.write_all((msg + "\n").as_bytes()).await.map(|_| w)
            });
            session(conns, Box::pin(incoming), Box::pin(outgoing)).await;
        });
    }
}

fn allowed_origin(origin: &str) -> bool {
    origin == PAGES_ORIGIN
        || ["http://127.0.0.1", "http://localhost"].iter().any(|h| {
            origin
                .strip_prefix(h)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(':'))
        })
}

#[allow(clippy::result_large_err)]
fn gate(
    req: &tokio_tungstenite::tungstenite::handshake::server::Request,
    resp: tokio_tungstenite::tungstenite::handshake::server::Response,
) -> Result<
    tokio_tungstenite::tungstenite::handshake::server::Response,
    tokio_tungstenite::tungstenite::handshake::server::ErrorResponse,
> {
    let origin = req
        .headers()
        .get("origin")
        .and_then(|o| o.to_str().ok())
        .unwrap_or("");
    if allowed_origin(origin) {
        return Ok(resp);
    }
    let mut no = tokio_tungstenite::tungstenite::handshake::server::ErrorResponse::new(Some(
        "origin not allowed".to_string(),
    ));
    *no.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::FORBIDDEN;
    Err(no)
}

async fn serve_doorway(listener: tokio::net::TcpListener, conns: Conns) {
    use tokio_tungstenite::tungstenite::{Error, Message};
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            continue;
        };
        let conns = conns.clone();
        tokio::spawn(async move {
            let Ok(ws) = tokio_tungstenite::accept_hdr_async(stream, gate).await else {
                return;
            };
            let (write, read) = ws.split();
            let incoming = read
                .take_while(|m| futures_util::future::ready(m.is_ok()))
                .filter_map(|m| {
                    futures_util::future::ready(match m {
                        Ok(Message::Text(t)) => Some(t.to_string()),
                        _ => None,
                    })
                });
            let outgoing = write.with(|s: String| {
                futures_util::future::ready(Ok::<_, Error>(Message::Text(s.into())))
            });
            session(conns, Box::pin(incoming), Box::pin(outgoing)).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

    struct Raw {
        lines: tokio::io::Lines<tokio::io::BufReader<OwnedReadHalf>>,
        w: OwnedWriteHalf,
    }

    impl Raw {
        async fn say(&mut self, v: serde_json::Value) {
            self.w
                .write_all((v.to_string() + "\n").as_bytes())
                .await
                .unwrap();
        }

        async fn until(&mut self, pick: impl Fn(&serde_json::Value) -> bool) -> serde_json::Value {
            loop {
                let line = tokio::time::timeout(std::time::Duration::from_secs(5), self.lines.next_line())
                    .await
                    .expect("timed out")
                    .unwrap()
                    .unwrap();
                let v: serde_json::Value = serde_json::from_str(&line).unwrap();
                if pick(&v) {
                    return v;
                }
            }
        }
    }

    async fn raw(dir: &std::path::Path, id: &str) -> Raw {
        let stream = tokio::net::UnixStream::connect(dir.join("hub.sock")).await.unwrap();
        let (r, w) = stream.into_split();
        let mut raw = Raw {
            lines: tokio::io::BufReader::new(r).lines(),
            w,
        };
        raw.say(serde_json::json!({"claim": id})).await;
        raw.until(|v| v["you"] == id).await;
        raw
    }

    fn in_roster(v: &serde_json::Value, id: &str) -> Option<bool> {
        let r = v.get("roster")?.as_array()?;
        Some(r.iter().any(|x| x == id))
    }

    async fn test_hub(tag: &str) -> (std::path::PathBuf, u16) {
        let dir = std::env::temp_dir().join(format!("wryme_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let unix = tokio::net::UnixListener::bind(dir.join("hub.sock")).unwrap();
        let tcp = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = tcp.local_addr().unwrap().port();
        let conns: Conns = Arc::default();
        tokio::spawn(serve_unix(unix, conns.clone()));
        tokio::spawn(serve_doorway(tcp, conns));
        (dir, port)
    }

    async fn ws(
        port: u16,
        origin: &str,
    ) -> Result<
        tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
        tokio_tungstenite::tungstenite::Error,
    > {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = format!("ws://127.0.0.1:{port}/").into_client_request().unwrap();
        req.headers_mut().insert("origin", origin.parse().unwrap());
        tokio_tungstenite::connect_async(req).await.map(|(s, _)| s)
    }

    #[tokio::test]
    async fn hub_routes_acks_and_replies_with_stamped_from() {
        let (dir, _) = test_hub("hubn").await;
        let mut a = raw(&dir, "aa01").await;
        let mut b = raw(&dir, "bb02").await;
        b.say(serde_json::json!({"to": "aa01", "from": "evil", "id": "m1", "text": "hello guest", "req": 1}))
            .await;
        let ack = b.until(|v| v["req"] == 1).await;
        assert_eq!(ack["ok"], true);
        let note = a.until(|v| v.get("text").is_some()).await;
        assert_eq!(note["from"], "bb02");
        assert_eq!(note["text"], "hello guest");
        a.say(serde_json::json!({"to": "bb02", "id": "m2", "text": "got it", "reply_to": "m1", "req": 7}))
            .await;
        let back = b.until(|v| v.get("text").is_some()).await;
        assert_eq!(back["from"], "aa01");
        assert_eq!(back["reply_to"], "m1");
        b.say(serde_json::json!({"to": "zz99", "text": "hi", "req": 2})).await;
        let ack = b.until(|v| v["req"] == 2).await;
        assert!(ack["error"].as_str().unwrap().contains("no live window"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn closed_window_leaves_and_taken_claim_is_refused() {
        let (dir, _) = test_hub("hubc").await;
        let a = raw(&dir, "aa01").await;
        let mut b = raw(&dir, "bb02").await;
        b.say(serde_json::json!({"claim": "aa01"})).await;
        let refused = b.until(|v| v.get("you").is_some()).await;
        assert_eq!(refused["you"], "bb02");
        drop(a);
        b.until(|v| in_roster(v, "aa01") == Some(false)).await;
        b.say(serde_json::json!({"to": "aa01", "text": "still there?", "req": 3})).await;
        let ack = b.until(|v| v["req"] == 3).await;
        assert_eq!(ack["ok"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn doorway_relays_web_note_and_gates_origin() {
        let (dir, port) = test_hub("hubw").await;
        assert!(ws(port, "https://evil.example").await.is_err());
        let mut native = raw(&dir, "cc01").await;
        let mut tab = ws(port, "http://127.0.0.1:8080").await.unwrap();
        tab.send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::json!({"to": "cc01", "text": "knock knock"}).to_string().into(),
        ))
        .await
        .unwrap();
        let v = native.until(|v| v.get("text").is_some()).await;
        assert!(v["from"].as_str().unwrap().starts_with('w'));
        assert_eq!(v["text"], "knock knock");
        assert!(allowed_origin("https://adiled.github.io"));
        assert!(allowed_origin("http://localhost:8080"));
        assert!(!allowed_origin("http://localhost.evil.example"));
        assert!(!allowed_origin(""));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
