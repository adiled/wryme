//! Browser entry. Boots into demo mode (no config files, no network), wires
//! DOM input into the same `run()` loop the terminal uses, paints through
//! [`WebBackend`](crate::platform::web::backend::WebBackend).

use anyhow::Result;
use ratatui::Terminal;

use crate::platform;
use crate::platform::web::backend::WebBackend;

/// Boots the terminal in the page: panic hooks, DOM input, the run loop.
/// Called by the `web/` package's wasm entry.
pub fn start() {
    console_error_panic_hook::set_once();
    let (tx, events) = platform::web::channel();
    platform::web::install(&tx);
    wasm_bindgen_futures::spawn_local(async {
        if let Err(e) = boot_and_run(events).await {
            log(&format!("wryme: {e:#}"));
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
        boot.discovery_errors,
        events,
    )
    .await
}

fn log(msg: &str) {
    web_sys::console::log_1(&wasm_bindgen::JsValue::from_str(msg));
}
