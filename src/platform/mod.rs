//! The two seams, and nothing else.
//!
//! * **pixels out** — [`web::backend`] implements ratatui's `Backend` for the
//!   browser; native uses ratatui's `CrosstermBackend`.
//! * **events in** — native replays crossterm's `EventStream`, the browser
//!   replays DOM key/wheel listeners. Both are fed to `run()` as a stream of
//!   [`Event`].
//!
//! Above this module the code never asks which world it lives in.

#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(target_arch = "wasm32")]
pub mod web;

#[cfg(not(target_arch = "wasm32"))]
pub use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

#[cfg(target_arch = "wasm32")]
pub use web::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

#[cfg(not(target_arch = "wasm32"))]
pub use native::{Instant, Task, sleep, spawn, timeout, unix_secs};

#[cfg(target_arch = "wasm32")]
pub use web::{Instant, Task, sleep, spawn, timeout, unix_secs};
