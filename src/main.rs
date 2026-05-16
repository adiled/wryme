// Entry point. Owns the terminal, the tokio runtime, the API client, and
// the event loop that selects between keyboard events and streaming deltas.

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
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
mod ui;

use api::{Client, StreamEvent};
use app::App;
use input::Input;

#[derive(Parser, Debug)]
#[command(
    name = "wryme",
    version,
    about = "streaming LLM chat TUI — input on top, newest reply right below it"
)]
struct Args {
    /// Model name to send upstream. Defaults to $WRYME_MODEL or gpt-4o-mini.
    #[arg(long)]
    model: Option<String>,

    /// Base URL of an OpenAI-compatible /chat/completions endpoint.
    /// Defaults to $OPENAI_BASE_URL or https://api.openai.com/v1.
    #[arg(long)]
    base_url: Option<String>,

    /// API key. Defaults to $OPENAI_API_KEY. May be empty for local servers
    /// (Ollama, LM Studio) that don't authenticate.
    #[arg(long)]
    api_key: Option<String>,

    /// Optional system prompt prepended to every request.
    #[arg(long)]
    system: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let base_url = args
        .base_url
        .or_else(|| std::env::var("OPENAI_BASE_URL").ok())
        .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
    let api_key = args
        .api_key
        .or_else(|| std::env::var("OPENAI_API_KEY").ok())
        .unwrap_or_default();
    let user_model = args.model.or_else(|| std::env::var("WRYME_MODEL").ok());

    // Demo mode = the user has nothing configured. As soon as they set
    // OPENAI_API_KEY or point us at a local server, we switch to real.
    let is_default_url = base_url == "https://api.openai.com/v1";
    let demo = api_key.is_empty() && is_default_url;
    let model = user_model.unwrap_or_else(|| {
        if demo {
            "demo (no OPENAI_API_KEY set)".to_string()
        } else {
            "gpt-4o-mini".to_string()
        }
    });

    let client = Client::new(base_url, api_key, model, demo).context("building api client")?;

    let mut terminal = setup_terminal().context("entering tui")?;
    install_panic_hook();

    let result = run(&mut terminal, client, args.system).await;

    restore_terminal(&mut terminal).ok();
    result
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
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
        terminal.draw(|f| ui::draw(f, &app, &input, client.model()))?;
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
                    Event::Resize(_, _) => { /* redraw on next loop */ }
                    _ => {}
                }
            }
            Some(stream_ev) = rx.recv() => {
                match stream_ev {
                    StreamEvent::Delta { text } => {
                        app.append_to_last_assistant(&text);
                    }
                    StreamEvent::Done => {
                        app.finish_streaming();
                        if let Some(t) = in_flight_task.take() {
                            // The task closed the channel on its own; just drop the handle.
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
                app.note("still streaming — Esc to cancel");
                return;
            }
            let text = input.take().trim().to_string();
            if text.is_empty() {
                return;
            }
            app.push_user(text);
            app.begin_assistant();
            app.in_flight = true;
            app.note("");

            let msgs = app.api_messages();
            let client = client.clone();
            let tx = tx.clone();
            *in_flight = Some(tokio::spawn(async move {
                client.stream_completion(msgs, tx).await;
            }));
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
