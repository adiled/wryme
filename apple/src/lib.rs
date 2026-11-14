use std::ffi::{CStr, CString, c_char};
use std::sync::OnceLock;

use anyhow::Result;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use wryme::app::App;
use wryme::input::Input;
use wryme::platform::{Event, KeyCode, KeyEvent, KeyModifiers};

type Emit = extern "C" fn(*const c_char);

static EVENTS: OnceLock<mpsc::UnboundedSender<Event>> = OnceLock::new();

struct Screen {
    emit: Emit,
    last: String,
}

impl wryme::Surface for Screen {
    fn present(&mut self, app: &mut App, _: &Input) -> Result<()> {
        let scene = wryme::scene::wire(app);
        if scene != self.last {
            send(self.emit, &scene);
            self.last = scene;
        }
        Ok(())
    }
}

fn send(emit: Emit, text: &str) {
    if let Ok(text) = CString::new(text) {
        emit(text.as_ptr());
    }
}

async fn session(emit: Emit, rx: mpsc::UnboundedReceiver<Event>) -> Result<()> {
    wryme::shell_env::bootstrap();
    let boot = wryme::boot(None).await?;
    let events = Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|ev| (ev, rx))
    }));
    let mut screen = Screen {
        emit,
        last: String::new(),
    };
    wryme::run(
        &mut screen,
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

fn key(code: KeyCode, ctrl: bool) -> Event {
    let mods = if ctrl {
        KeyModifiers::CONTROL
    } else {
        KeyModifiers::NONE
    };
    Event::Key(KeyEvent::new(code, mods))
}

fn events(msg: &Value) -> Vec<Event> {
    if let Some(text) = msg["submit"].as_str() {
        return vec![Event::Paste(text.to_string()), key(KeyCode::Enter, false)];
    }
    let name = msg["key"].as_str().unwrap_or_default();
    let code = match name {
        "esc" => KeyCode::Esc,
        "enter" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "backspace" => KeyCode::Backspace,
        "delete" => KeyCode::Delete,
        "f1" => KeyCode::F(1),
        _ => {
            let mut chars = name.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => KeyCode::Char(c),
                _ => return Vec::new(),
            }
        }
    };
    vec![key(code, msg["ctrl"] == true)]
}

#[unsafe(no_mangle)]
pub extern "C" fn wryme_start(emit: Emit) {
    let (tx, rx) = mpsc::unbounded_channel();
    if EVENTS.set(tx).is_err() {
        return;
    }
    std::thread::spawn(move || {
        let ended = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(anyhow::Error::from)
            .and_then(|rt| rt.block_on(session(emit, rx)));
        match ended {
            Ok(()) => std::process::exit(0),
            Err(e) => send(emit, &json!({"fatal": format!("{e:#}")}).to_string()),
        }
    });
}

#[allow(clippy::missing_safety_doc)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wryme_send(msg: *const c_char) {
    if msg.is_null() {
        return;
    }
    let text = unsafe { CStr::from_ptr(msg) }.to_string_lossy();
    let (Ok(msg), Some(tx)) = (serde_json::from_str::<Value>(&text), EVENTS.get()) else {
        return;
    };
    for ev in events(&msg) {
        let _ = tx.send(ev);
    }
}
