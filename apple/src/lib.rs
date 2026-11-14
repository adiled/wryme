use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use anyhow::Result;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;
use wryme::action::{Action, Core};
use wryme::control::Intent;
use wryme::scene::Scene;

type Emit = extern "C" fn(*const c_char);

const FRAME: Duration = Duration::from_millis(16);

static INBOX: OnceLock<mpsc::UnboundedSender<Intent>> = OnceLock::new();

#[derive(Default)]
struct Latest {
    scene: Mutex<Option<Value>>,
    ready: Condvar,
}

struct Screen {
    latest: Arc<Latest>,
}

impl wryme::Surface for Screen {
    type Event = Intent;

    fn present(&mut self, scene: &Scene) -> Result<()> {
        let value = serde_json::to_value(scene)?;
        if let Ok(mut slot) = self.latest.scene.lock() {
            *slot = Some(value);
        }
        self.latest.ready.notify_one();
        Ok(())
    }

    fn feed(&mut self, ev: Intent, _: &Scene) -> Vec<Intent> {
        vec![ev]
    }
}

#[derive(Default)]
struct Sent {
    stage: Value,
    order: Vec<String>,
    bubbles: HashMap<String, Value>,
}

impl Sent {
    fn frame(&mut self, mut scene: Value) -> Option<Value> {
        let list = match scene.as_object_mut().and_then(|m| m.remove("bubbles")) {
            Some(Value::Array(list)) => list,
            _ => Vec::new(),
        };
        let mut order = Vec::with_capacity(list.len());
        let mut changed = Map::new();
        let mut kept = HashMap::with_capacity(list.len());
        for bubble in list {
            let id = bubble["id"].as_str().unwrap_or_default().to_string();
            if self.bubbles.get(&id) != Some(&bubble) {
                changed.insert(id.clone(), bubble.clone());
            }
            order.push(id.clone());
            kept.insert(id, bubble);
        }
        let quiet = changed.is_empty() && order == self.order && scene == self.stage;
        self.bubbles = kept;
        self.order = order;
        if quiet {
            return None;
        }
        self.stage = scene;
        Some(json!({"stage": self.stage, "order": self.order, "bubbles": changed}))
    }
}

fn flush(emit: Emit, latest: Arc<Latest>) {
    let mut sent = Sent::default();
    loop {
        let scene = {
            let Ok(mut slot) = latest.scene.lock() else {
                return;
            };
            while slot.is_none() {
                slot = match latest.ready.wait(slot) {
                    Ok(slot) => slot,
                    Err(_) => return,
                };
            }
            slot.take()
        };
        if let Some(frame) = scene.and_then(|s| sent.frame(s)) {
            send(emit, &frame.to_string());
        }
        std::thread::sleep(FRAME);
    }
}

fn send(emit: Emit, text: &str) {
    if let Ok(text) = CString::new(text) {
        emit(text.as_ptr());
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Inbound {
    Do(Core),
    Submit(String),
    Search(char),
    Find(String),
    Tab(usize),
    Pick(usize),
    Reload,
}

impl From<Inbound> for Intent {
    fn from(msg: Inbound) -> Self {
        match msg {
            Inbound::Do(c) => Intent::Act(Action::Core(c)),
            Inbound::Submit(text) => Intent::Submit(text),
            Inbound::Search(c) => Intent::Search(c),
            Inbound::Find(text) => Intent::Find(text),
            Inbound::Tab(i) => Intent::Tab(i),
            Inbound::Pick(i) => Intent::Pick(i),
            Inbound::Reload => Intent::Reload,
        }
    }
}

async fn session(emit: Emit, rx: mpsc::UnboundedReceiver<Intent>) -> Result<()> {
    let _sentry = wryme::instrument();
    send(emit, &json!({"bindings": wryme::action::table()}).to_string());
    let boot = wryme::boot(None).await?;
    let latest = Arc::new(Latest::default());
    let flusher = latest.clone();
    std::thread::spawn(move || flush(emit, flusher));
    let events = Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|ev| (ev, rx))
    }));
    wryme::run(
        &mut Screen { latest },
        boot.client,
        None,
        boot.shops,
        boot.stations,
        boot.active_station,
        boot.active_shop,
        boot.active_origin,
        events,
    )
    .await
}

#[unsafe(no_mangle)]
pub extern "C" fn wryme_start(emit: Emit) {
    let (tx, rx) = mpsc::unbounded_channel();
    if INBOX.set(tx).is_err() {
        return;
    }
    std::thread::spawn(move || {
        let ended = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(anyhow::Error::from)
            .and_then(|rt| rt.block_on(session(emit, rx)));
        match ended {
            Ok(()) => std::process::exit(0),
            Err(e) => send(emit, &json!({"fatal": format!("{e:#}")}).to_string()),
        }
    });
}

#[allow(clippy::missing_safety_doc)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wryme_send(msg: *const c_char) {
    if msg.is_null() {
        return;
    }
    let text = unsafe { CStr::from_ptr(msg) }.to_string_lossy();
    let (Ok(msg), Some(tx)) = (serde_json::from_str::<Inbound>(&text), INBOX.get()) else {
        return;
    };
    let _ = tx.send(msg.into());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn swift() -> String {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/app");
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "swift"))
            .map(|e| std::fs::read_to_string(e.path()).unwrap())
            .collect()
    }

    fn names<T: serde::Serialize>(all: &[T]) -> Vec<String> {
        all.iter()
            .map(|v| serde_json::to_value(v).unwrap().as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn swift_handles_every_shared_case() {
        use wryme::action::{Edit, Pane, Scope};
        use wryme::app::ViewMode;
        use wryme::config::{NametagMode, TYPEFACES};
        let src = swift();
        let mut wanted = names(Edit::ALL);
        wanted.extend(names(Pane::ALL));
        wanted.extend(names(Scope::ALL));
        wanted.extend(names(&[ViewMode::Page, ViewMode::Scroll]));
        wanted.extend(names(&[NametagMode::Standout, NametagMode::Inlined]));
        wanted.extend(TYPEFACES.iter().map(|(name, _)| name.to_string()));
        let missing: Vec<&String> = wanted
            .iter()
            .filter(|name| !src.contains(&format!("case {name}")) && !src.contains(&format!(", {name}")))
            .collect();
        assert!(missing.is_empty(), "Swift has no case for {missing:?}");
    }

    #[test]
    fn frames_send_only_changed_bubbles() {
        let mut sent = Sent::default();
        let scene = |reply: &str| {
            json!({"turn": 1, "bubbles": [{"id": "0", "reply": "hi"}, {"id": "1", "reply": reply}]})
        };
        let first = sent.frame(scene("a")).unwrap();
        assert_eq!(first["bubbles"].as_object().unwrap().len(), 2);
        let next = sent.frame(scene("ab")).unwrap();
        assert_eq!(next["bubbles"].as_object().unwrap().keys().collect::<Vec<_>>(), ["1"]);
        assert_eq!(next["order"], json!(["0", "1"]));
        assert!(sent.frame(scene("ab")).is_none());
    }

    #[test]
    fn inbound_maps_to_intents() {
        let parse = |s: &str| Intent::from(serde_json::from_str::<Inbound>(s).unwrap());
        assert!(matches!(parse(r#"{"do":"settings"}"#), Intent::Act(Action::Core(Core::Settings))));
        assert!(matches!(parse(r#"{"pick":4}"#), Intent::Pick(4)));
        assert!(matches!(parse(r#""reload""#), Intent::Reload));
        assert!(matches!(parse(r#"{"search":"q"}"#), Intent::Search('q')));
    }
}
