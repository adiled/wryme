use std::sync::Arc;

use anyhow::Result;
use hum_nest::{ForagerAdvert, serve_forager};
use tracing_subscriber::EnvFilter;

mod dispatch;

use dispatch::WrymeDispatcher;

#[tokio::main]
async fn main() -> Result<()> {
    hum_paths::init();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("HUM_LOG_LEVEL")
                .unwrap_or_else(|_| EnvFilter::new("info,wryme_hive=trace")),
        )
        .init();

    let dispatcher = Arc::new(WrymeDispatcher::new()?);
    let advert = ForagerAdvert {
        hive: "wryme-hive".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        source: Some("https://github.com/adiled/wryme/tree/main/hive".into()),
        provides: vec!["shell".into()],
    };
    serve_forager(dispatcher, advert).await
}
