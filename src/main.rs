#[cfg(not(target_arch = "wasm32"))]
fn main() -> anyhow::Result<()> {
    wryme::run_native()
}

/// The browser build runs from the `web/` package, not this binary.
#[cfg(target_arch = "wasm32")]
fn main() {}
