// Keyboard and mouse handlers. main.rs owns the terminal and the event
// loop, but the actual dispatch (which key triggers which intent) lives
// here so it stays small and small-LLM-readable.
//
// Three entry points:
//   handle_key   the main input is focused (default state)
//   popup_key    the station popup is open and capturing keys
//   handle_mouse mouse events, currently only the scroll wheel
//
// handle_key checks app.popup.mode first; if the popup is open it
// forwards to popup_key.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind};
use tokio::sync::mpsc;

use crate::api::{Client, StreamEvent};
use crate::app::{App, ViewMode};
use crate::input::Input;
use crate::popup;

pub fn handle_key(
    k: KeyEvent,
    app: &mut App,
    input: &mut Input,
    client: &Client,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    in_flight: &mut Option<tokio::task::JoinHandle<()>>,
) {
    // Ignore key-release events; we only act on press.
    if k.kind == KeyEventKind::Release {
        return;
    }
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);

    // Global: Ctrl-C exits immediately, no questions asked.
    if ctrl && matches!(k.code, KeyCode::Char('c')) {
        if let Some(t) = in_flight.take() {
            t.abort();
        }
        app.should_quit = true;
        return;
    }

    // When the station popup is open, it captures input.
    if app.popup.mode != popup::Mode::Closed {
        popup_key(k, app);
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
            app.scroll_row = 0;
            app.wheel_accum = 0;
            app.note("");

            let msgs = app.api_messages();
            let prev_id = app.last_response_id.clone();
            let shop = app.active_shop.clone();
            let station = app.active_station.clone();
            let client = client.clone();
            let tx = tx.clone();
            *in_flight = Some(tokio::spawn(async move {
                client
                    .stream_completion(shop, station, msgs, prev_id, tx)
                    .await;
            }));
        }
        KeyCode::PageUp => match app.view_mode {
            ViewMode::Page => {
                app.current_page = app.current_page.saturating_add(1);
            }
            ViewMode::Scroll => {
                let step = app.last_viewport_h.saturating_sub(1).max(1);
                app.scroll_row = app.scroll_row.saturating_add(step);
            }
        },
        KeyCode::PageDown => match app.view_mode {
            ViewMode::Page => {
                app.current_page = app.current_page.saturating_sub(1);
            }
            ViewMode::Scroll => {
                let step = app.last_viewport_h.saturating_sub(1).max(1);
                app.scroll_row = app.scroll_row.saturating_sub(step);
            }
        },
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
                    't' => toggle_view_mode(app),
                    's' => popup::toggle(app),
                    _ => {}
                }
            } else {
                input.insert_char(c);
            }
        }
        _ => {}
    }
}

/// Mouse events. Only the scroll wheel does anything; everything else is
/// ignored. View-mode-aware: in Page mode the wheel steps pages with a
/// three-tick accumulator (trackpad friendly); in Scroll mode it steps
/// two rows per tick.
pub fn handle_mouse(m: MouseEvent, app: &mut App) {
    match app.view_mode {
        ViewMode::Page => {
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
        ViewMode::Scroll => {
            const ROWS_PER_TICK: usize = 2;
            match m.kind {
                MouseEventKind::ScrollUp => {
                    app.scroll_row = app.scroll_row.saturating_add(ROWS_PER_TICK);
                }
                MouseEventKind::ScrollDown => {
                    app.scroll_row = app.scroll_row.saturating_sub(ROWS_PER_TICK);
                }
                _ => {}
            }
        }
    }
}

/// Flip between Page and Scroll view modes. Resets scroll offsets and
/// wheel accumulator so the new mode starts fresh.
fn toggle_view_mode(app: &mut App) {
    app.view_mode = match app.view_mode {
        ViewMode::Page => ViewMode::Scroll,
        ViewMode::Scroll => ViewMode::Page,
    };
    app.current_page = 0;
    app.scroll_row = 0;
    app.wheel_accum = 0;
    app.note(match app.view_mode {
        ViewMode::Page => "view: page",
        ViewMode::Scroll => "view: scroll",
    });
}

/// Keys when the station popup is open. Two sub-modes:
///   Browse: arrow nav, ←/→ adjust, Enter act, Esc close, Ctrl-S close.
///   SaveAs: text editing of the name field, Enter commit, Esc cancel.
fn popup_key(k: KeyEvent, app: &mut App) {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);

    // Ctrl-S always closes from any sub-mode.
    if ctrl && matches!(k.code, KeyCode::Char('s')) {
        popup::close(app);
        return;
    }

    match app.popup.mode {
        popup::Mode::Closed => {}
        popup::Mode::Browse => match k.code {
            KeyCode::Esc => popup::close(app),
            KeyCode::Up => popup::move_selection(app, -1),
            KeyCode::Down => popup::move_selection(app, 1),
            KeyCode::Left => popup::adjust(app, -1),
            KeyCode::Right => popup::adjust(app, 1),
            KeyCode::Enter => popup::activate(app),
            _ => {}
        },
        popup::Mode::SaveAs => match k.code {
            KeyCode::Esc => {
                app.popup.mode = popup::Mode::Browse;
                app.popup.name_input = Input::new();
            }
            KeyCode::Enter => popup::commit_save_as(app),
            KeyCode::Left => app.popup.name_input.move_left(),
            KeyCode::Right => app.popup.name_input.move_right(),
            KeyCode::Home => app.popup.name_input.home(),
            KeyCode::End => app.popup.name_input.end(),
            KeyCode::Backspace => app.popup.name_input.backspace(),
            KeyCode::Delete => app.popup.name_input.delete_forward(),
            KeyCode::Char(c) => {
                if ctrl {
                    match c {
                        'u' => app.popup.name_input.kill_to_start(),
                        'k' => app.popup.name_input.kill_to_end(),
                        'a' => app.popup.name_input.home(),
                        'e' => app.popup.name_input.end(),
                        'w' => app.popup.name_input.kill_prev_word(),
                        _ => {}
                    }
                } else {
                    app.popup.name_input.insert_char(c);
                }
            }
            _ => {}
        },
    }
}
