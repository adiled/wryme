// Entry point. Owns the terminal, the tokio runtime, the API client, and
// the event loop that selects between keyboard events and streaming deltas.

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    event::{
        Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
        DisableMouseCapture, EnableMouseCapture, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures_util::StreamExt;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io::Stdout;
use tokio::sync::mpsc;

mod api;
mod app;
mod demo;
mod input;
mod station;
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
    /// Name of the station to use. Defaults to the env-defined default
    /// station, then the first station in ~/.config/wryme/stations.toml,
    /// then the built-in demo.
    #[arg(long)]
    station: Option<String>,

    /// Optional system prompt prepended to every request.
    #[arg(long)]
    system: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let stations = station::load_all().context("loading stations")?;
    let active = station::pick(&stations, args.station.as_deref())?.clone();

    let client = Client::new(active).context("building api client")?;

    let mut terminal = setup_terminal().context("entering tui")?;
    install_panic_hook();

    let result = run(&mut terminal, client, args.system).await;

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
) -> Result<()> {
    let mut app = App::new(system);
    let mut input = Input::new();
    let mut events = EventStream::new();
    let (tx, mut rx) = mpsc::unbounded_channel::<StreamEvent>();
    let mut in_flight_task: Option<tokio::task::JoinHandle<()>> = None;

    loop {
        terminal.draw(|f| ui::draw(f, &app, &input, client.station()))?;
        if app.should_quit {
            break;
        }

        tokio::select! {
            maybe_ev = events.next() => {
                let Some(Ok(ev)) = maybe_ev else { continue };
                match ev {
                    Event::Key(k) if k.kind != KeyEventKind::Release => {
                        handle_key(k, &mut app, &mut input, &client, &tx, &mut in_flight_task);
                    }
                    Event::Mouse(m) => {
                        handle_mouse(m, &mut app);
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

fn handle_mouse(m: MouseEvent, app: &mut App) {
    // Three wheel ticks per page flip. Keeps trackpad scrolling from
    // blowing through the conversation in one swipe.
    const TICKS_PER_PAGE: i32 = 3;
    match m.kind {
        MouseEventKind::ScrollUp => app.wheel_accum += 1,
        MouseEventKind::ScrollDown => app.wheel_accum -= 1,
        _ => return,
    }
    while app.wheel_accum >= TICKS_PER_PAGE {
        app.current_page = app.current_page.saturating_add(1);
        app.wheel_accum -= TICKS_PER_PAGE;
    }
    while app.wheel_accum <= -TICKS_PER_PAGE {
        app.current_page = app.current_page.saturating_sub(1);
        app.wheel_accum += TICKS_PER_PAGE;
    }
}

fn handle_key(
    k: KeyEvent,
    app: &mut App,
    input: &mut Input,
    client: &Client,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    in_flight: &mut Option<tokio::task::JoinHandle<()>>,
) {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);

    // Global: Ctrl-C exits immediately, no questions asked.
    if ctrl && matches!(k.code, KeyCode::Char('c')) {
        if let Some(t) = in_flight.take() {
            t.abort();
        }
        app.should_quit = true;
        return;
    }

    match k.code {
        KeyCode::Esc => {
            if let Some(t) = in_flight.take() {
                t.abort();
                app.finish_streaming();
                app.note("cancelled");
            } else if !app.status.is_empty() {
                app.note("");
            }
        }
        KeyCode::Enter => {
            if app.in_flight {
                app.note("still streaming. Esc to cancel");
                return;
            }
            let text = input.take().trim().to_string();
            if text.is_empty() {
                return;
            }
            app.push_user(text);
            app.begin_assistant();
            app.in_flight = true;
            app.current_page = 0;
            app.note("");

            let msgs = app.api_messages();
            let client = client.clone();
            let tx = tx.clone();
            *in_flight = Some(tokio::spawn(async move {
                client.stream_completion(msgs, tx).await;
            }));
        }
        KeyCode::PageUp => {
            app.current_page = app.current_page.saturating_add(1);
        }
        KeyCode::PageDown => {
            app.current_page = app.current_page.saturating_sub(1);
        }
        KeyCode::Left => input.move_left(),
        KeyCode::Right => input.move_right(),
        KeyCode::Home => input.home(),
        KeyCode::End => input.end(),
        KeyCode::Backspace => input.backspace(),
        KeyCode::Delete => input.delete_forward(),
        KeyCode::Char(c) => {
            if ctrl {
                match c {
                    'u' => input.kill_to_start(),
                    'k' => input.kill_to_end(),
                    'a' => input.home(),
                    'e' => input.end(),
                    'w' => input.kill_prev_word(),
                    _ => {}
                }
            } else {
                input.insert_char(c);
            }
        }
        _ => {}
    }
}
