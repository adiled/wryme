#[cfg(not(target_arch = "wasm32"))]
fn main() -> anyhow::Result<()> {
    wryme::run_native()
}

#[cfg(target_arch = "wasm32")]
fn main() {}
