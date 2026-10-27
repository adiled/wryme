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
pub use native::{Instant, Task, copy, sleep, spawn, timeout, unix_secs};

#[cfg(target_arch = "wasm32")]
pub use web::{Instant, Task, copy, sleep, spawn, timeout, unix_secs};
