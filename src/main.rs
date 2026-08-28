// Entry point. Owns the terminal, the tokio runtime, the API client, and
// the event loop that selects between keyboard events and streaming deltas.

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    event::{
        Event, EventStream, KeyEventKind, DisableMouseCapture, EnableMouseCapture,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures_util::StreamExt;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io::Stdout;
use tokio::sync::mpsc;

mod api;
mod api_chat;
mod api_responses;
mod app;
mod demo;
mod input;
mod keys;
mod md;
mod popup;
mod popup_ui;
mod shop;
mod station;
mod station_save;
mod ui;

use api::{Client, StreamEvent};
use app::App;
use input::Input;

#[derive(Parser, Debug)]
#[command(
    name = "wryme",
    version,
    about = "streaming LLM chat TUI. Input on top, newest reply right below it."
)]
struct Args {
    /// Name of a saved station to use. Defaults to the first saved station,
    /// or a synthesized "untitled" station built from the newest model the
    /// first shop advertises, or the built-in demo if nothing is configured.
    #[arg(long)]
    station: Option<String>,

    /// Optional system prompt prepended to every request.
    #[arg(long)]
    system: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let mut shops = shop::load_all().context("loading shops")?;
    let discovery_errors = shop::discover_all(&mut shops).await;
    let stations = station::load_all().context("loading stations")?;
    let (active, active_origin) = station::pick(&stations, &shops, args.station.as_deref())?;

    // Resolve the shop that advertises this station's model.
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

    let mut terminal = setup_terminal().context("entering tui")?;
    install_panic_hook();

    let result = run(
        &mut terminal,
        client,
        args.system,
        shops,
        stations,
        active,
        active_shop,
        active_origin,
        discovery_errors,
    )
    .await;

    restore_terminal(&mut terminal).ok();
    result
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;
    Ok(())
}

fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        prev(info);
    }));
}

async fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    client: Client,
    system: Option<String>,
    shops: Vec<shop::Shop>,
    stations: Vec<station::Station>,
    active_station: station::Station,
    active_shop: shop::Shop,
    active_origin: Option<String>,
    discovery_errors: Vec<(String, String)>,
) -> Result<()> {
    let mut app = App::new(
        system,
        shops,
        stations,
        active_station,
        active_shop,
        active_origin,
    );
    if !discovery_errors.is_empty() {
        let summary = discovery_errors
            .iter()
            .map(|(s, e)| format!("{}: {}", s, e))
            .collect::<Vec<_>>()
            .join("; ");
        app.note(format!("discovery: {}", summary));
    }
    let mut input = Input::new();
    let mut events = EventStream::new();
    let (tx, mut rx) = mpsc::unbounded_channel::<StreamEvent>();
    let mut in_flight_task: Option<tokio::task::JoinHandle<()>> = None;

    loop {
        terminal.draw(|f| ui::draw(f, &mut app, &input))?;
        if app.should_quit {
            break;
        }

        tokio::select! {
            maybe_ev = events.next() => {
                let Some(Ok(ev)) = maybe_ev else { continue };
                match ev {
                    Event::Key(k) if k.kind != KeyEventKind::Release => {
                        keys::handle_key(k, &mut app, &mut input, &client, &tx, &mut in_flight_task);
                    }
                    Event::Mouse(m) => {
                        keys::handle_mouse(m, &mut app);
                    }
                    Event::Resize(_, _) => { /* redraw on next loop */ }
                    _ => {}
                }
            }
            Some(stream_ev) = rx.recv() => {
                match stream_ev {
                    StreamEvent::Delta { text } => {
                        app.append_to_last_assistant(&text);
                    }
                    StreamEvent::Brain { text } => {
                        app.append_to_last_brain(&text);
                    }
                    StreamEvent::ToolCall { name } => {
                        app.record_tool_call(name);
                    }
                    StreamEvent::ResponseId { id } => {
                        app.last_response_id = Some(id);
                    }
                    StreamEvent::Done => {
                        app.finish_streaming();
                        if let Some(t) = in_flight_task.take() {
                            drop(t);
                        }
                    }
                    StreamEvent::Error { message } => {
                        app.note(format!("upstream: {message}"));
                    }
                }
            }
        }
    }

    if let Some(t) = in_flight_task.take() {
        t.abort();
    }
    Ok(())
}

