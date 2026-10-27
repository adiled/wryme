use tokio::sync::mpsc;

use crate::platform::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

const POPUP_ROWS_PER_TICK: i32 = 2;

fn attached_images(text: &str) -> Vec<String> {
    let t = text.trim();
    if t.is_empty() || t.chars().any(|c| c.is_whitespace()) {
        return Vec::new();
    }
    let p = std::path::Path::new(t);
    if !p.is_file() {
        return Vec::new();
    }
    let img = matches!(
        p.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp")
    );
    if img { vec![t.to_string()] } else { Vec::new() }
}

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
    in_flight: &mut Option<crate::platform::Task>,
) {
    if k.kind == KeyEventKind::Release {
        return;
    }
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let shift = k.modifiers.contains(KeyModifiers::SHIFT);
    let super_ = k.modifiers.contains(KeyModifiers::SUPER);

    if matches!(k.code, KeyCode::Char('c')) && (super_ || (ctrl && shift)) {
        finish_selection(app);
        return;
    }

    if ctrl && !shift && !super_ && matches!(k.code, KeyCode::Char('c')) {
        if let Some(t) = in_flight.take() {
            t.abort();
        }
        app.shutdown_voice();
        app.should_quit = true;
        return;
    }

    if k.code == KeyCode::F(1) {
        popup::open_help(app);
        return;
    }

    if ctrl && matches!(k.code, KeyCode::Char('v')) {
        crate::popup::toggle_voice(app);
        return;
    }

    if app.popup.mode != popup::Mode::Closed {
        popup_key(k, app);
        return;
    }

    if shift {
        let (dc, dr) = match k.code {
            KeyCode::Left => (-1, 0),
            KeyCode::Right => (1, 0),
            KeyCode::Up => (0, -1),
            KeyCode::Down => (0, 1),
            _ => (0, 0),
        };
        if (dc, dr) != (0, 0)
            && let Some(sel) = app.selection.as_mut()
        {
            sel.move_head(dc, dr);
            return;
        }
    }

    match k.code {
        KeyCode::Esc => {
            if app.selection.take().is_some() {
                return;
            }
            if app.voice_on && !app.voice_muted && app.in_flight {
                app.mute_voice();
                app.note("quiet");
                return;
            }
            app.stop_voice();
            match in_flight.take() {
                Some(t) => {
                    t.abort();
                    app.finish_streaming();
                    if app.awaiting.is_empty() {
                        app.note("cancelled");
                    } else {
                        let pending = std::mem::take(&mut app.awaiting);
                        start_turn(app, client, tx, in_flight, pending);
                    }
                }
                _ => {
                    if !app.status.is_empty() {
                        app.note("");
                    }
                }
            }
        }
        KeyCode::Enter => {
            let text = input.take().trim().to_string();
            if text.is_empty() {
                return;
            }
            if app.in_flight {
                if app.awaiting.is_empty() {
                    app.awaiting = text;
                } else {
                    app.awaiting.push('\n');
                    app.awaiting.push_str(&text);
                }
                return;
            }
            start_turn(app, client, tx, in_flight, text);
        }
        KeyCode::PageUp => match app.view_mode {
            ViewMode::Page => {
                let base = app.page_base;
                if base > 0 && app.current_page == 0 {
                    let step = app.last_viewport_h.saturating_sub(1).max(1);
                    let trigger = base.saturating_sub(app.last_viewport_h / 2);
                    let cur = app.scroll_row.min(trigger);
                    if cur.saturating_add(step) >= trigger {
                        app.scroll_row = trigger;
                        app.current_page = 1;
                    } else {
                        app.scroll_row = cur.saturating_add(step);
                    }
                    app.wheel_accum = 0;
                } else {
                    app.current_page = app.current_page.saturating_add(1);
                }
            }
            ViewMode::Scroll => {
                let step = app.last_viewport_h.saturating_sub(1).max(1);
                app.scroll_row = app.scroll_row.saturating_add(step);
            }
        },
        KeyCode::PageDown => match app.view_mode {
            ViewMode::Page => {
                let base = app.page_base;
                if base > 0 {
                    if app.current_page >= 2 {
                        app.current_page -= 1;
                    } else if app.current_page == 1 {
                        app.current_page = 0;
                        app.scroll_row = base.saturating_sub(app.last_viewport_h / 2);
                        app.wheel_accum = 0;
                    } else {
                        let step = app.last_viewport_h.saturating_sub(1).max(1);
                        app.scroll_row = app.scroll_row.saturating_sub(step);
                        app.wheel_accum = 0;
                    }
                } else {
                    app.current_page = app.current_page.saturating_sub(1);
                }
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

pub fn start_turn(
    app: &mut App,
    client: &Client,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    in_flight: &mut Option<crate::platform::Task>,
    text: String,
) {
    let images = attached_images(&text);
    if app.messages.is_empty() {
        tracing::info!("window start");
    }
    app.push_user(text, images);
    let _ = app.reservoir.lock().map(|mut r| r.turn_started());
    app.begin_assistant();
    app.stop_voice();
    app.unmute_voice();
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
    let engine = app.engine.clone();
    let tx = tx.clone();
    *in_flight = Some(crate::platform::spawn(async move {
        client
            .stream_completion(shop, station, msgs, prev_id, engine, tx)
            .await;
    }));
}

pub fn take_note(
    app: &mut App,
    client: &Client,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    in_flight: &mut Option<crate::platform::Task>,
    note: crate::windows::Note,
) {
    if app.in_flight {
        app.guest_queue.push(note);
    } else {
        start_guest_turn(app, client, tx, in_flight, note);
    }
}

pub fn start_guest_turn(
    app: &mut App,
    client: &Client,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    in_flight: &mut Option<crate::platform::Task>,
    note: crate::windows::Note,
) {
    app.push_guest(note);
    let _ = app.reservoir.lock().map(|mut r| r.turn_started());
    app.begin_assistant();
    app.stop_voice();
    app.unmute_voice();
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
    let engine = app.engine.clone();
    let tx = tx.clone();
    *in_flight = Some(crate::platform::spawn(async move {
        client
            .stream_completion(shop, station, msgs, prev_id, engine, tx)
            .await;
    }));
}

pub fn handle_paste(text: &str, app: &mut App, input: &mut Input) {
    match app.popup.mode {
        popup::Mode::Closed => input.insert_paste(text),
        popup::Mode::SaveAs => app.popup.name_input.insert_paste(text),
        popup::Mode::DialEdit => app.popup.dial_input.insert_paste(text),
        popup::Mode::Browse => {}
    }
}

fn copy_selection(app: &mut App) -> bool {
    let Some(sel) = app.selection else {
        return false;
    };
    let text = sel.extract(&app.screen);
    if text.trim().is_empty() {
        return false;
    }
    crate::platform::copy(&text);
    true
}

fn finish_selection(app: &mut App) {
    if copy_selection(app) {
        app.note("copied");
    }
    app.selection = None;
}

pub fn handle_mouse(m: MouseEvent, app: &mut App) {
    if app.popup.mode == popup::Mode::Closed {
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                app.selection = Some(crate::select::Selection::new(crate::select::Point {
                    col: m.column,
                    row: m.row,
                }));
                return;
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(sel) = app.selection.as_mut() {
                    sel.extend(crate::select::Point {
                        col: m.column,
                        row: m.row,
                    });
                }
                return;
            }
            MouseEventKind::Up(MouseButton::Left) => {
                finish_selection(app);
                return;
            }
            _ => {}
        }
    }
    if app.popup.mode != popup::Mode::Closed {
        if matches!(
            m.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        ) {
            let delta = if matches!(m.kind, MouseEventKind::ScrollUp) {
                1
            } else {
                -1
            };
            popup::scroll(app, delta * POPUP_ROWS_PER_TICK);
        }
        return;
    }
    match app.view_mode {
        ViewMode::Page => {
            let base = app.page_base;
            let trigger = base.saturating_sub(app.last_viewport_h / 2);
            if base > 0 && app.current_page == 0 {
                const ROWS_PER_TICK: usize = 2;
                match m.kind {
                    MouseEventKind::ScrollUp => {
                        let cur = app.scroll_row.min(trigger);
                        if cur >= trigger {
                            app.scroll_row = trigger;
                            app.current_page = 1;
                            app.wheel_accum = 0;
                        } else {
                            app.scroll_row = cur.saturating_add(ROWS_PER_TICK).min(trigger);
                        }
                    }
                    MouseEventKind::ScrollDown => {
                        app.scroll_row = app.scroll_row.saturating_sub(ROWS_PER_TICK);
                    }
                    _ => return,
                }
                return;
            }
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
            if app.current_page == 0 && base > 0 {
                app.scroll_row = base.saturating_sub(app.last_viewport_h / 2);
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

pub(crate) fn toggle_view_mode(app: &mut App) {
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

fn popup_key(k: KeyEvent, app: &mut App) {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);

    if ctrl && matches!(k.code, KeyCode::Char('s')) {
        popup::close(app);
        return;
    }

    match app.popup.mode {
        popup::Mode::Closed => {}
        popup::Mode::Browse => match k.code {
            KeyCode::Esc => popup::close(app),
            KeyCode::Tab => popup::switch_tab(app),
            KeyCode::F(1) => popup::open_help(app),
            KeyCode::PageUp => popup::scroll(app, 1),
            KeyCode::PageDown => popup::scroll(app, -1),
            KeyCode::Up => popup::move_selection(app, -1),
            KeyCode::Down => popup::move_selection(app, 1),
            KeyCode::Left => popup::adjust(app, -1),
            KeyCode::Right => popup::adjust(app, 1),
            KeyCode::Enter => popup::activate(app),
            KeyCode::Backspace => popup::backspace_search(app),
            KeyCode::Char(c) if !ctrl => popup::type_search(app, c),
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
        popup::Mode::DialEdit => match k.code {
            KeyCode::Esc => {
                app.popup.mode = popup::Mode::Browse;
                app.popup.dial_input = Input::new();
                app.popup.dial_idx = None;
            }
            KeyCode::Enter => popup::commit_dial_edit(app),
            KeyCode::Left => app.popup.dial_input.move_left(),
            KeyCode::Right => app.popup.dial_input.move_right(),
            KeyCode::Home => app.popup.dial_input.home(),
            KeyCode::End => app.popup.dial_input.end(),
            KeyCode::Backspace => app.popup.dial_input.backspace(),
            KeyCode::Delete => app.popup.dial_input.delete_forward(),
            KeyCode::Char(c) => {
                if ctrl {
                    match c {
                        'u' => app.popup.dial_input.kill_to_start(),
                        'k' => app.popup.dial_input.kill_to_end(),
                        'a' => app.popup.dial_input.home(),
                        'e' => app.popup.dial_input.end(),
                        'w' => app.popup.dial_input.kill_prev_word(),
                        _ => {}
                    }
                } else {
                    app.popup.dial_input.insert_char(c);
                }
            }
            _ => {}
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_path_attaches_single_file() {
        let dir = std::env::temp_dir();
        let path = dir.join("wryme_test_img.png");
        std::fs::write(&path, b"fakeimage").unwrap();
        let got = attached_images(&path.to_string_lossy());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0], path.to_string_lossy().to_string());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn non_image_or_text_is_not_attached() {
        let dir = std::env::temp_dir();
        let path = dir.join("wryme_test.txt");
        std::fs::write(&path, b"hi").unwrap();
        assert_eq!(attached_images(&path.to_string_lossy()).len(), 0);
        assert_eq!(attached_images("just some words").len(), 0);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn data_url_roundtrip() {
        let dir = std::env::temp_dir();
        let path = dir.join("wryme_test2.png");
        std::fs::write(&path, b"\x89PNG\r\n\x1a\nabc").unwrap();
        let (mime, b64) = crate::api::image_data_url(&path.to_string_lossy()).unwrap();
        assert_eq!(mime, "image/png");
        use base64::Engine;
        let dec = base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .unwrap();
        assert_eq!(&dec[..8], b"\x89PNG\r\n\x1a\n");
        std::fs::remove_file(&path).ok();
    }
}
