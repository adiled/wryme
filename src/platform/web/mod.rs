//! Browser seam: DOM events in, ratatui `Backend` out, JS timers and the
//! single-threaded executor in between.

pub mod backend;

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_util::Stream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{ClipboardEvent, KeyboardEvent, MouseEvent as WebMouseEvent, WheelEvent};

/// The element the terminal paints into.
pub const TERM_ID: &str = "wryme-term";

// ---------------------------------------------------------------------------
// event types — a faithful subset of crossterm's, so `keys.rs` reads the same
// on both platforms
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyCode {
    Backspace,
    Enter,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Tab,
    BackTab,
    Delete,
    Insert,
    F(u8),
    Char(char),
    Null,
    Esc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyModifiers(u8);

impl KeyModifiers {
    pub const NONE: Self = Self(0);
    pub const SHIFT: Self = Self(1);
    pub const CONTROL: Self = Self(2);
    pub const ALT: Self = Self(4);
    pub const SUPER: Self = Self(8);

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyEventKind {
    Press,
    Release,
    Repeat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyEvent {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
    pub kind: KeyEventKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseEventKind {
    Down(MouseButton),
    Up(MouseButton),
    Drag(MouseButton),
    Moved,
    ScrollDown,
    ScrollUp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MouseEvent {
    pub kind: MouseEventKind,
    pub column: u16,
    pub row: u16,
    pub modifiers: KeyModifiers,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Event {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize(u16, u16),
    Paste(String),
    FocusGained,
    FocusLost,
    ReloadConfig,
}

// ---------------------------------------------------------------------------
// runtime
// ---------------------------------------------------------------------------

pub fn sleep(d: Duration) -> impl Future<Output = ()> {
    gloo_timers::future::TimeoutFuture::new(d.as_millis() as u32)
}

pub async fn timeout<F: Future>(d: Duration, f: F) -> Option<F::Output> {
    use futures_util::future::{Either, select};
    let timer = gloo_timers::future::TimeoutFuture::new(d.as_millis() as u32);
    match select(Box::pin(f), Box::pin(timer)).await {
        Either::Left((v, _)) => Some(v),
        Either::Right(_) => None,
    }
}

/// A cancellable task. There is one thread: aborting means dropping the
/// future at its next await point.
pub struct Task {
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Task {
    pub fn abort(mut self) {
        self.cancel.take();
    }
}

pub fn spawn<F>(fut: F) -> Task
where
    F: Future<Output = ()> + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    wasm_bindgen_futures::spawn_local(async move {
        let _ = futures_util::future::select(Box::pin(fut), Box::pin(rx)).await;
    });
    Task { cancel: Some(tx) }
}

#[derive(Debug, Clone, Copy)]
pub struct Instant(f64);

impl Instant {
    pub fn now() -> Self {
        Self(
            web_sys::window()
                .and_then(|w| w.performance())
                .map(|p| p.now())
                .unwrap_or(0.0),
        )
    }

    pub fn elapsed(&self) -> Duration {
        let now = web_sys::window()
            .and_then(|w| w.performance())
            .map(|p| p.now())
            .unwrap_or(0.0);
        Duration::from_millis((now - self.0).max(0.0) as u64)
    }
}

pub fn unix_secs() -> f64 {
    js_sys::Date::now() / 1000.0
}

// ---------------------------------------------------------------------------
// input: DOM listeners -> tokio channel -> the run loop
// ---------------------------------------------------------------------------

/// Creates the event channel. The sender half goes to the listeners, the
/// receiver half becomes the `events` stream in `run()`.
pub fn channel() -> (UnboundedSender<Event>, EventStream) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    (tx, EventStream(rx))
}

pub struct EventStream(UnboundedReceiver<Event>);

impl Unpin for EventStream {}

impl Stream for EventStream {
    type Item = Event;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Event>> {
        self.0.poll_recv(cx)
    }
}

/// Wires `keydown` on the document and `wheel` on the terminal element.
/// Listeners live for the life of the page, so they are leaked on purpose.
pub fn install(tx: &UnboundedSender<Event>) {
    let doc = match web_sys::window().and_then(|w| w.document()) {
        Some(d) => d,
        None => return,
    };

    let key_tx = tx.clone();
    let on_key: Closure<dyn FnMut(KeyboardEvent)> = Closure::new(move |ev| {
        if let Some(k) = map_key(&ev) {
            ev.prevent_default();
            let _ = key_tx.send(Event::Key(k));
        }
    });
    // Document is not an Element, but it derefs to EventTarget.
    let _ = doc.add_event_listener_with_callback("keydown", on_key.as_ref().unchecked_ref());
    on_key.forget();

    let wheel_tx = tx.clone();
    let mut wheel_accum = 0.0f64;
    let on_wheel: Closure<dyn FnMut(WheelEvent)> = Closure::new(move |ev: WheelEvent| {
        ev.prevent_default();
        let mut dy = ev.delta_y();
        match ev.delta_mode() {
            1 => dy *= 16.0,
            2 => dy *= 400.0,
            _ => {}
        }
        if !dy.is_finite() || dy == 0.0 {
            return;
        }
        if wheel_accum != 0.0 && wheel_accum.signum() != dy.signum() {
            wheel_accum = 0.0;
        }
        wheel_accum += dy;
        while wheel_accum >= 30.0 || wheel_accum <= -30.0 {
            let kind = if wheel_accum < 0.0 {
                wheel_accum += 30.0;
                MouseEventKind::ScrollUp
            } else {
                wheel_accum -= 30.0;
                MouseEventKind::ScrollDown
            };
            let _ = wheel_tx.send(Event::Mouse(MouseEvent {
                kind,
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            }));
        }
    });
    if let Some(term) = doc.get_element_by_id(TERM_ID) {
        let opts = web_sys::AddEventListenerOptions::new();
        opts.set_passive(false);
        let _ = term.add_event_listener_with_callback_and_add_event_listener_options(
            "wheel",
            on_wheel.as_ref().unchecked_ref(),
            &opts,
        );
    }
    on_wheel.forget();

    let paste_tx = tx.clone();
    let on_paste: Closure<dyn FnMut(ClipboardEvent)> = Closure::new(move |ev: ClipboardEvent| {
        ev.prevent_default();
        let text = ev
            .clipboard_data()
            .and_then(|d| d.get_data("text").ok())
            .filter(|t| !t.is_empty());
        if let Some(text) = text {
            let _ = paste_tx.send(Event::Paste(text));
        }
    });
    let _ = doc.add_event_listener_with_callback("paste", on_paste.as_ref().unchecked_ref());
    on_paste.forget();

    if let Some(term) = doc.get_element_by_id(TERM_ID) {
        let down_tx = tx.clone();
        let on_down: Closure<dyn FnMut(WebMouseEvent)> = Closure::new(move |ev: WebMouseEvent| {
            if ev.button() != 0 {
                return;
            }
            ev.prevent_default();
            if let Some((column, row)) = cell_point(&ev) {
                let _ = down_tx.send(Event::Mouse(MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column,
                    row,
                    modifiers: KeyModifiers::NONE,
                }));
            }
        });
        let _ = term.add_event_listener_with_callback("mousedown", on_down.as_ref().unchecked_ref());
        on_down.forget();

        let move_tx = tx.clone();
        let on_move: Closure<dyn FnMut(WebMouseEvent)> = Closure::new(move |ev: WebMouseEvent| {
            if ev.buttons() & 1 == 0 {
                return;
            }
            if let Some((column, row)) = cell_point(&ev) {
                let _ = move_tx.send(Event::Mouse(MouseEvent {
                    kind: MouseEventKind::Drag(MouseButton::Left),
                    column,
                    row,
                    modifiers: KeyModifiers::NONE,
                }));
            }
        });
        let _ = term.add_event_listener_with_callback("mousemove", on_move.as_ref().unchecked_ref());
        on_move.forget();
    }

    let up_tx = tx.clone();
    let on_up: Closure<dyn FnMut(WebMouseEvent)> = Closure::new(move |ev: WebMouseEvent| {
        if ev.button() != 0 {
            return;
        }
        let _ = up_tx.send(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }));
    });
    let _ = doc.add_event_listener_with_callback("mouseup", on_up.as_ref().unchecked_ref());
    on_up.forget();
}

fn metrics(doc: &web_sys::Document) -> (f64, f64) {
    if let Some(el) = doc.get_element_by_id("wryme-measure")
        && let r = el.get_bounding_client_rect()
        && r.width() > 0.0
        && r.height() > 0.0
    {
        return (r.width() / 20.0, r.height());
    }
    (9.0, 18.0)
}

fn cell_point(ev: &WebMouseEvent) -> Option<(u16, u16)> {
    let doc = web_sys::window()?.document()?;
    let term = doc.get_element_by_id(TERM_ID)?;
    let rect = term.get_bounding_client_rect();
    let (char_w, line_h) = metrics(&doc);
    let col = ((f64::from(ev.client_x()) - rect.left()) / char_w).floor().max(0.0) as u16;
    let row = ((f64::from(ev.client_y()) - rect.top()) / line_h).floor().max(0.0) as u16;
    Some((col, row))
}

pub fn copy(text: &str) {
    if let Some(win) = web_sys::window() {
        let _ = win.navigator().clipboard().write_text(text);
    }
}

/// DOM key -> crossterm-shaped key. Returns `None` for keys we don't model
/// (IME composition, dead keys, media keys).
fn map_key(ev: &KeyboardEvent) -> Option<KeyEvent> {
    if (ev.ctrl_key() || ev.meta_key()) && ev.key().eq_ignore_ascii_case("v") {
        return None;
    }
    let mut modifiers = KeyModifiers::NONE;
    if ev.shift_key() {
        modifiers = modifiers.with(KeyModifiers::SHIFT);
    }
    if ev.alt_key() {
        modifiers = modifiers.with(KeyModifiers::ALT);
    }
    // Cmd plays the role of Ctrl so the muscle memory (Ctrl-S popup, Ctrl-T
    // view, Ctrl-V voice) survives the move to the browser.
    if ev.ctrl_key() {
        modifiers = modifiers.with(KeyModifiers::CONTROL);
    }
    if ev.meta_key() {
        modifiers = modifiers
            .with(KeyModifiers::CONTROL)
            .with(KeyModifiers::SUPER);
    }

    let code = match ev.key().as_str() {
        "Enter" => KeyCode::Enter,
        "Escape" => KeyCode::Esc,
        "Backspace" => KeyCode::Backspace,
        "Tab" => KeyCode::Tab,
        "ArrowLeft" => KeyCode::Left,
        "ArrowRight" => KeyCode::Right,
        "ArrowUp" => KeyCode::Up,
        "ArrowDown" => KeyCode::Down,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,
        "Delete" => KeyCode::Delete,
        "Insert" => KeyCode::Insert,
        "F1" => KeyCode::F(1),
        "F2" => KeyCode::F(2),
        "F3" => KeyCode::F(3),
        "F4" => KeyCode::F(4),
        "F5" => KeyCode::F(5),
        "F6" => KeyCode::F(6),
        "F7" => KeyCode::F(7),
        "F8" => KeyCode::F(8),
        "F9" => KeyCode::F(9),
        "F10" => KeyCode::F(10),
        "F11" => KeyCode::F(11),
        "F12" => KeyCode::F(12),
        " " => KeyCode::Char(' '),
        k if k.chars().count() == 1 && !ev.ctrl_key() => KeyCode::Char(k.chars().next()?),
        // ctrl/cmd combos report the base char; keep it lowercase the way a
        // terminal would
        k if k.chars().count() == 1 => KeyCode::Char(k.chars().next()?.to_ascii_lowercase()),
        _ => return None,
    };

    Some(KeyEvent {
        code,
        modifiers,
        kind: KeyEventKind::Press,
    })
}
