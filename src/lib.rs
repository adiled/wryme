//! wryme — library root.
//!
//! Two front doors: [`run_native`] (the terminal binary) and [`web::start`]
//! (the browser wasm entry). Everything in between — app state, ui, api — is
//! platform agnostic; the two seams (events in, pixels out) live in
//! [`platform`] and [`web`].

pub mod api;
pub mod api_chat;
pub mod api_responses;
pub mod app;
pub mod book;
pub mod config;
pub mod demo;
pub mod explore;
pub mod input;
pub mod jobs;
pub mod keys;
pub mod md;
pub mod platform;
pub mod popup;
pub mod popup_ui;
#[cfg(feature = "reservoir")]
pub mod reservoir;
#[cfg(not(feature = "reservoir"))]
#[path = "reservoir_stub.rs"]
pub mod reservoir;
pub mod shell_env;
pub mod shop;
pub mod station;
pub mod station_save;
pub mod tools;
pub mod ui;
pub mod voice;

#[cfg(target_arch = "wasm32")]
pub mod web;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use ratatui::{Terminal, backend::Backend};
use tokio::sync::mpsc;

use api::{Client, StreamEvent};
use app::App;
use input::Input;
use platform::{Event, Task};

/// Everything resolved at launch: a client, a shop, a station.
pub struct Boot {
    pub client: Client,
    pub shops: Vec<shop::Shop>,
    pub stations: Vec<station::Station>,
    pub active_station: station::Station,
    pub active_shop: shop::Shop,
    pub active_origin: Option<String>,
}

/// Find shops, load stations, pick the active pair, build the http client.
pub async fn boot(station_arg: Option<&str>) -> Result<Boot> {
    let mut shops = shop::load_all().context("loading shops")?;
    shop::discover_all(&mut shops).await;
    let stations = station::load_all().context("loading stations")?;
    let (active, active_origin) = station::pick(&stations, &shops, station_arg)?;

    let active_shop = shop::find_for_model(&shops, &active.model)
        .cloned()
        .with_context(|| {
            format!(
                "station '{}' wants model '{}' but no shop advertises it. \
                 add this model to a shop's `models = [...]` list in shops.toml.",
                active.name, active.model
            )
        })?;

    let client = Client::new().context("building api client")?;

    Ok(Boot {
        client,
        shops,
        stations,
        active_station: active,
        active_shop,
        active_origin,
    })
}

/// The one true loop: draw, wait for an event / a stream delta / the tick.
///
/// `events` is whatever the platform feeds us — crossterm's `EventStream` on
/// native, DOM key/wheel listeners in the browser.
#[allow(clippy::too_many_arguments)]
pub async fn run<B, S>(
    terminal: &mut Terminal<B>,
    client: Client,
    system: Option<String>,
    shops: Vec<shop::Shop>,
    stations: Vec<station::Station>,
    active_station: station::Station,
    active_shop: shop::Shop,
    active_origin: Option<String>,
    mut events: S,
) -> Result<()>
where
    B: Backend<Error = std::io::Error>,
    S: futures_util::Stream<Item = Event> + Unpin,
{
    let mut app = App::new(
        system,
        shops,
        stations,
        active_station,
        active_shop,
        active_origin,
    );
    let mut input = Input::new();
    let (tx, mut rx) = mpsc::unbounded_channel::<StreamEvent>();
    let mut in_flight_task: Option<Task> = None;

    loop {
        terminal
            .draw(|f| ui::draw(f, &mut app, &input))
            .map_err(|e| anyhow::anyhow!("draw: {e}"))?;
        if app.should_quit {
            break;
        }

        tokio::select! {
            maybe_ev = events.next() => {
                if let Some(ev) = maybe_ev {
                    match ev {
                        Event::Key(k) => {
                            keys::handle_key(k, &mut app, &mut input, &client, &tx, &mut in_flight_task);
                        }
                        Event::Mouse(m) => keys::handle_mouse(m, &mut app),
                        Event::Paste(text) => keys::handle_paste(&text, &mut app, &mut input),
                        #[cfg(target_arch = "wasm32")]
                        Event::ReloadConfig => {
                            match crate::boot(None).await {
                                Ok(b) => {
                                    app.reconfigure(
                                        b.shops,
                                        b.stations,
                                        b.active_station,
                                        b.active_shop,
                                        b.active_origin,
                                    );
                                    app.note("config connected");
                                }
                                Err(e) => app.note(format!("config: {e:#}")),
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some(stream_ev) = rx.recv() => {
                handle_stream_event(stream_ev, &mut app, &mut in_flight_task);
            }
            _ = platform::sleep(std::time::Duration::from_millis(250)) => {
                if jobs::has_due() && !app.in_flight {
                    let _ = app.reservoir.lock().map(|mut r| r.turn_started());
                    app.begin_assistant();
                    app.in_flight = true;
                    let msgs = app.api_messages();
                    let prev_id = app.last_response_id.clone();
                    let shop = app.active_shop.clone();
                    let station = app.active_station.clone();
                    let client = client.clone();
                    let engine = app.engine.clone();
                    let tx = tx.clone();
                    in_flight_task = Some(platform::spawn(async move {
                        client
                            .stream_completion(shop, station, msgs, prev_id, engine, tx)
                            .await;
                    }));
                }
            }
        }
    }

    if let Some(t) = in_flight_task.take() {
        t.abort();
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_stream_event(
    stream_ev: StreamEvent,
    app: &mut App,
    in_flight_task: &mut Option<Task>,
) {
    match stream_ev {
        StreamEvent::Delta { text } => {
            app.append_to_last_assistant(&text);
            if app.voice_on && !app.voice_muted {
                let voice = app.active_station.voice.clone();
                app.ensure_speaker(voice);
                app.voice_buffer.push_str(&text);
                for s in crate::voice::split_sentences(&mut app.voice_buffer) {
                    if let Some(sp) = &app.voice_speaker {
                        sp.say(s);
                    }
                }
            }
        }
        StreamEvent::Brain { text } => {
            app.append_to_last_brain(&text);
        }
        StreamEvent::Heart { text } => {
            app.append_to_last_heart(&text);
        }
        StreamEvent::ToolCall { name } => {
            app.record_tool_call(name);
        }
        StreamEvent::ToolResult {
            call_id,
            name,
            arguments,
            output,
        } => {
            app.record_tool_result(call_id, name, arguments, output);
        }
        StreamEvent::ResponseId { id } => {
            app.last_response_id = Some(id);
        }
        StreamEvent::WindowUnsupported { shop } => {
            for s in app.shops.iter_mut() {
                if s.name == shop {
                    s.window = crate::shop::WindowMode::Full;
                }
            }
            if app.active_shop.name == shop {
                app.active_shop.window = crate::shop::WindowMode::Full;
            }
            app.note(format!("{shop}: warm window unsupported, using full"));
        }
        StreamEvent::Usage { input, output } => {
            app.usage_ctx = input;
            app.usage_out += output;
            let station = app.active_station.name.clone();
            let model = app.active_station.model.clone();
            let full = app.active_shop.window == crate::shop::WindowMode::Full;
            if let Ok(mut res) = app.reservoir.lock() {
                res.note_round(&station, &model, input, output, full);
            }
        }
        StreamEvent::Done => {
            app.finish_streaming();
            if app.voice_on && !app.voice_muted {
                let tail = app.voice_buffer.trim().to_string();
                app.voice_buffer.clear();
                let voice = app.active_station.voice.clone();
                app.ensure_speaker(voice);
                if let Some(sp) = &app.voice_speaker {
                    if !tail.is_empty() {
                        sp.say(tail);
                    }
                    sp.flush();
                }
            }
            if let Some(t) = in_flight_task.take() {
                drop(t);
            }
        }
        StreamEvent::Error { message } => {
            tracing::error!(err = %message, "turn error");
            let station = app.active_station.name.clone();
            let friendly = app
                .reservoir
                .lock()
                .ok()
                .and_then(|mut r| r.note_error(&station, &message));
            match friendly {
                Some(f) => app.note(f),
                None => app.note(format!("upstream: {message}")),
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn one_shot(
    client: &Client,
    station: &station::Station,
    shop: &shop::Shop,
    system: Option<&str>,
    prompt: &str,
) -> Result<()> {
    let mut messages: Vec<api::ApiMessage> = Vec::new();
    if let Some(sys) = system {
        messages.push(api::ApiMessage {
            role: "system".into(),
            content: sys.to_string(),
            images: Vec::new(),
            tool_calls: Vec::new(),
            tool_call_id: String::new(),
            tool_result: String::new(),
        });
    }
    messages.push(api::ApiMessage {
        role: "user".into(),
        content: prompt.to_string(),
        images: Vec::new(),
        tool_calls: Vec::new(),
        tool_call_id: String::new(),
        tool_result: String::new(),
    });

    let engine = std::sync::Arc::new(std::sync::Mutex::new(
        crate::book::open_engine(&std::env::temp_dir().join("wryme-oneshot")).expect("open book"),
    ));
    let (tx, mut rx) = mpsc::unbounded_channel::<api::StreamEvent>();
    let task = platform::spawn({
        let shop = shop.clone();
        let station = station.clone();
        let client = client.clone();
        async move {
            client
                .stream_completion(shop, station, messages, None, engine, tx)
                .await;
        }
    });

    let mut out = String::new();
    let mut err: Option<String> = None;
    let mut done = false;
    while let Some(ev) = rx.recv().await {
        match ev {
            api::StreamEvent::Delta { text } => out.push_str(&text),
            api::StreamEvent::Error { message } => err = Some(message),
            api::StreamEvent::Done => {
                done = true;
                break;
            }
            _ => {}
        }
    }
    drop(task);

    if let Some(e) = err {
        anyhow::bail!(e);
    }
    if !done {
        anyhow::bail!("stream ended without Done");
    }
    print!("{out}");
    Ok(())
}

// ---------------------------------------------------------------------------
// native front door
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
pub fn run_native() -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building runtime")?;
    rt.block_on(native_main())
}

#[cfg(not(target_arch = "wasm32"))]
async fn native_main() -> Result<()> {
    use crossterm::event::EventStream;
    use clap::Parser as _;

    init_logging();
    shell_env::bootstrap();
    let _sentry = sentry::init(sentry::ClientOptions {
        dsn: std::env::var("SENTRY_DSN")
            .ok()
            .and_then(|s| s.parse().ok())
            .or_else(|| {
                "https://114f188d49c0df6af704e00e13a7f512@o4510982366625792.ingest.us.sentry.io/4512044113068032"
                    .parse()
                    .ok()
            }),
        release: Some(env!("WRYME_VERSION").into()),
        traces_sample_rate: 0.0,
        ..Default::default()
    });
    tracing::info!(version = env!("WRYME_VERSION"), "wme launch");
    let args = cli::Args::parse();

    let boot = boot(args.station.as_deref()).await?;

    if let Some(prompt) = args.prompt.as_deref() {
        return one_shot(
            &boot.client,
            &boot.active_station,
            &boot.active_shop,
            args.system.as_deref(),
            prompt,
        )
        .await;
    }
    tracing::info!(
        shop = %boot.active_shop.name,
        model = %boot.active_station.model,
        protocol = ?boot.active_shop.protocol,
        window = ?boot.active_shop.window,
        "wme start"
    );

    let mut terminal = setup_terminal().context("entering tui")?;
    install_panic_hook();

    let events = EventStream::new().filter_map(|r| futures_util::future::ready(r.ok()));
    let result = run(
        &mut terminal,
        boot.client,
        args.system,
        boot.shops,
        boot.stations,
        boot.active_station,
        boot.active_shop,
        boot.active_origin,
        events,
    )
    .await;

    restore_terminal(&mut terminal).ok();
    result
}

#[cfg(not(target_arch = "wasm32"))]
mod cli {
    use clap::Parser;

    #[derive(Parser, Debug)]
    #[command(
        name = "wryme",
        version = env!("WRYME_VERSION"),
        about = "wryme • that small, calm window where agents come to meet you"
    )]
    pub struct Args {
        #[arg(long)]
        pub station: Option<String>,

        #[arg(short, long)]
        pub prompt: Option<String>,

        #[arg(long)]
        pub system: Option<String>,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn setup_terminal() -> anyhow::Result<ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>>
{
    use crossterm::{
        event::{EnableBracketedPaste, EnableMouseCapture},
        execute,
        terminal::EnterAlternateScreen,
    };
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;
    terminal.clear()?;
    Ok(terminal)
}

#[cfg(not(target_arch = "wasm32"))]
fn restore_terminal(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
) -> anyhow::Result<()> {
    use crossterm::{
        event::{DisableBracketedPaste, DisableMouseCapture},
        execute,
        terminal::LeaveAlternateScreen,
    };
    crossterm::terminal::disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste
    )?;
    terminal.show_cursor()?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn install_panic_hook() {
    use crossterm::{
        event::{DisableBracketedPaste, DisableMouseCapture},
        execute,
        terminal::LeaveAlternateScreen,
    };
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = execute!(
            std::io::stdout(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            DisableBracketedPaste
        );
        prev(info);
    }));
}

#[cfg(not(target_arch = "wasm32"))]
fn init_logging() {
    let dir = std::env::var("HOME").ok().map(|h| {
        std::path::PathBuf::from(h)
            .join(".local")
            .join("share")
            .join("wryme")
    });
    let Some(dir) = dir else { return };
    let _ = std::fs::create_dir_all(&dir);
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("wryme.log"))
    else {
        return;
    };
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "wme=info".into());
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false)
        .try_init();
}
