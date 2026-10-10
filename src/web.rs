use anyhow::Result;
use ratatui::Terminal;
use std::time::Duration;
use wasm_bindgen::prelude::Closure;
use wasm_bindgen::{JsCast, JsValue};

use crate::platform;
use crate::platform::web::backend::WebBackend;

pub fn start() {
    console_error_panic_hook::set_once();
    let (tx, events) = platform::web::channel();
    platform::web::install(&tx);

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
    crate::run(
        &mut terminal,
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
