//! wasm entry for the browser build. The crate name is what trunk links;
//! everything real lives in `wryme::web`.

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
pub fn start() {
    wryme::web::start();
}
