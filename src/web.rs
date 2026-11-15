use anyhow::Result;
use ratatui::Terminal;
use std::time::Duration;
use wasm_bindgen::prelude::Closure;
use wasm_bindgen::{JsCast, JsValue};

use crate::control::Intent;
use crate::platform;
use crate::platform::web::Event;
use crate::platform::web::backend::WebBackend;
use crate::scene::Scene;
use crate::term::Term;
use crate::Surface;

struct Browser {
    term: Term<WebBackend>,
    pick: Option<u64>,
}

impl Surface for Browser {
    type Event = Event;

    fn present(&mut self, scene: &Scene) -> Result<()> {
        if self.pick.is_some_and(|p| p != scene.sync.pick) {
            pick_folder();
        }
        self.pick = Some(scene.sync.pick);
        self.term.present(scene)
    }

    fn feed(&mut self, ev: Event, scene: &Scene) -> Vec<Intent> {
        match ev {
            Event::GuestNote(note) => vec![Intent::Note(note)],
            Event::ReloadConfig => vec![Intent::Reload],
            ev => self.term.feed(ev, scene),
        }
    }
}

pub fn start() {
    console_error_panic_hook::set_once();
    let (tx, events) = platform::web::channel();
    platform::web::install_listeners(&tx);

    let cfg_tx = tx.clone();
    let setter = Closure::<dyn Fn(Option<String>, Option<String>, Option<bool>)>::new(
        move |shops: Option<String>, stations: Option<String>, connected: Option<bool>| {
            crate::config::set_text(shops, stations, connected.unwrap_or(false));
            let _ = cfg_tx.send(platform::web::Event::ReloadConfig);
        },
    );
    let _ = js_sys::Reflect::set(
        &js_sys::global(),
        &JsValue::from_str("wrymeSetConfig"),
        setter.as_ref(),
    );
    setter.forget();

    let prefs_tx = tx.clone();
    let set_prefs = Closure::<dyn Fn(Option<String>)>::new(move |prefs: Option<String>| {
        crate::config::set_prefs_text(prefs);
        let _ = prefs_tx.send(platform::web::Event::ReloadConfig);
    });
    let _ = js_sys::Reflect::set(
        &js_sys::global(),
        &JsValue::from_str("wrymeSetPrefs"),
        set_prefs.as_ref(),
    );
    set_prefs.forget();

    let guest_tx = tx.clone();
    let guest_note = Closure::<dyn Fn(String, String, String, String)>::new(
        move |from: String, id: String, text: String, reply_to: String| {
            let note = crate::windows::Note { from, id, text, reply_to };
            let _ = guest_tx.send(platform::web::Event::GuestNote(note));
        },
    );
    let _ = js_sys::Reflect::set(
        &js_sys::global(),
        &JsValue::from_str("wrymeGuestNote"),
        guest_note.as_ref(),
    );
    guest_note.forget();

    let type_tx = tx.clone();
    let typed = Closure::<dyn Fn(String)>::new(move |text: String| {
        for ev in platform::web::typed(&text) {
            let _ = type_tx.send(ev);
        }
    });
    let _ = js_sys::Reflect::set(&js_sys::global(), &JsValue::from_str("wrymeType"), typed.as_ref());
    typed.forget();

    let set_roster = Closure::<dyn Fn(String)>::new(move |roster: String| {
        crate::windows::set_roster(&roster);
    });
    let _ = js_sys::Reflect::set(
        &js_sys::global(),
        &JsValue::from_str("wrymeSetRoster"),
        set_roster.as_ref(),
    );
    set_roster.forget();

    wasm_bindgen_futures::spawn_local(async {
        if let Err(e) = boot_and_run(events).await {
            log(&format!("wryme: {e:#}"));
        }
    });

    wasm_bindgen_futures::spawn_local(async {
        for _ in 0..200 {
            if call_global("wrymeReady") {
                return;
            }
            platform::sleep(Duration::from_millis(25)).await;
        }
    });
}

async fn boot_and_run(events: crate::platform::web::EventStream) -> Result<()> {
    let boot = crate::boot(None).await?;
    let backend = WebBackend::new()?;
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    let mut surface = Browser {
        term: Term::new(terminal),
        pick: None,
    };
    crate::run(
        &mut surface,
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

fn call_global(name: &str) -> bool {
    let global = js_sys::global();
    let Ok(f) = js_sys::Reflect::get(&global, &JsValue::from_str(name)) else {
        return false;
    };
    let Some(f) = f.dyn_ref::<js_sys::Function>() else {
        return false;
    };
    let _ = f.call0(&global);
    true
}

fn log(msg: &str) {
    web_sys::console::log_1(&JsValue::from_str(msg));
}

fn pick_folder() {
    call_global("wrymePickFolder");
}

pub fn announce_window(id: &str) {
    let global = js_sys::global();
    let Ok(f) = js_sys::Reflect::get(&global, &JsValue::from_str("wrymeSetWindowId")) else {
        return;
    };
    let Some(f) = f.dyn_ref::<js_sys::Function>() else {
        return;
    };
    let _ = f.call1(&global, &JsValue::from_str(id));
}
