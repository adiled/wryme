use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use anyhow::Result;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

use crate::action::{Action, Core};
use crate::control::Intent;
use crate::scene::Scene;

const FRAME: Duration = Duration::from_millis(16);

pub type Emit = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Inbound {
    Do(Core),
    Submit(String),
    Send { text: String, images: Vec<String> },
    Search(char),
    Find(String),
    Tab(usize),
    Pick(usize),
    Say(String),
    Reload,
}

impl From<Inbound> for Intent {
    fn from(msg: Inbound) -> Self {
        match msg {
            Inbound::Do(c) => Intent::Act(Action::Core(c)),
            Inbound::Submit(text) => Intent::Submit(text, Vec::new()),
            Inbound::Send { text, images } => Intent::Submit(text, images),
            Inbound::Search(c) => Intent::Search(c),
            Inbound::Find(text) => Intent::Find(text),
            Inbound::Tab(i) => Intent::Tab(i),
            Inbound::Pick(i) => Intent::Pick(i),
            Inbound::Say(text) => Intent::Say(text),
            Inbound::Reload => Intent::Reload,
        }
    }
}

pub fn parse(line: &str) -> Option<Intent> {
    serde_json::from_str::<Inbound>(line).ok().map(Intent::from)
}

#[derive(Default)]
struct Latest {
    scene: Mutex<Option<Value>>,
    ready: Condvar,
}

struct Screen {
    latest: Arc<Latest>,
}

impl crate::Surface for Screen {
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
struct Frames {
    stage: Value,
    order: Vec<String>,
    bubbles: HashMap<String, Value>,
}

impl Frames {
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
    let mut frames = Frames::default();
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
        if let Some(frame) = scene.and_then(|s| frames.frame(s)) {
            emit(&frame.to_string());
        }
        std::thread::sleep(FRAME);
    }
}

pub async fn serve(emit: Emit, inbox: mpsc::UnboundedReceiver<Intent>) -> Result<()> {
    emit(&json!({"bindings": crate::action::table()}).to_string());
    let boot = crate::boot(None).await?;
    let latest = Arc::new(Latest::default());
    let (out, flusher) = (emit.clone(), latest.clone());
    std::thread::spawn(move || flush(out, flusher));
    let events = Box::pin(futures_util::stream::unfold(inbox, |mut rx| async move {
        rx.recv().await.map(|ev| (ev, rx))
    }));
    crate::run(
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

pub fn fatal(emit: &Emit, e: &anyhow::Error) {
    emit(&json!({"fatal": format!("{e:#}")}).to_string());
}

pub fn stdio() -> Result<()> {
    let _sentry = crate::instrument();
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines() {
            let Ok(line) = line else { break };
            if let Some(intent) = parse(&line)
                && tx.send(intent).is_err()
            {
                return;
            }
        }
        let _ = tx.send(Intent::Act(Action::Core(Core::Quit)));
    });
    let emit: Emit = Arc::new(|line: &str| {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    });
    let ended = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(anyhow::Error::from)
        .and_then(|rt| rt.block_on(serve(emit.clone(), rx)));
    if let Err(e) = &ended {
        fatal(&emit, e);
    }
    ended
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_send_only_changed_bubbles() {
        let mut frames = Frames::default();
        let scene = |reply: &str| {
            json!({"turn": 1, "bubbles": [{"id": "0", "reply": "hi"}, {"id": "1", "reply": reply}]})
        };
        let first = frames.frame(scene("a")).unwrap();
        assert_eq!(first["bubbles"].as_object().unwrap().len(), 2);
        let next = frames.frame(scene("ab")).unwrap();
        assert_eq!(next["bubbles"].as_object().unwrap().keys().collect::<Vec<_>>(), ["1"]);
        assert_eq!(next["order"], json!(["0", "1"]));
        assert!(frames.frame(scene("ab")).is_none());
    }

    #[test]
    fn inbound_maps_to_intents() {
        assert!(matches!(parse(r#"{"do":"settings"}"#), Some(Intent::Act(Action::Core(Core::Settings)))));
        assert!(matches!(parse(r#"{"pick":4}"#), Some(Intent::Pick(4))));
        assert!(matches!(parse(r#""reload""#), Some(Intent::Reload)));
        assert!(matches!(parse(r#"{"search":"q"}"#), Some(Intent::Search('q'))));
        assert!(parse("not json").is_none());
        let sent = parse(r#"{"send":{"text":"look","images":["/tmp/a.png"]}}"#);
        assert!(matches!(sent, Some(Intent::Submit(t, i)) if t == "look" && i == ["/tmp/a.png"]));
    }
}
