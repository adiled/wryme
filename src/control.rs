use tokio::sync::mpsc;

use crate::action::{Action, Core};
use crate::api::{Client, StreamEvent};
use crate::app::{App, ViewMode};
use crate::platform::Task;
use crate::popup;

pub enum Intent {
    Act(Action),
    Submit(String, Vec<String>),
    Search(char),
    Find(String),
    Tab(usize),
    Pick(usize),
    Say(Notice),
    Note(crate::windows::Note),
    Reload,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Notice {
    Copied,
    NoPicture,
}

impl Notice {
    pub const ALL: &[Notice] = &[Notice::Copied, Notice::NoPicture];

    pub fn text(self) -> &'static str {
        match self {
            Notice::Copied => "copied",
            Notice::NoPicture => "no image on the clipboard",
        }
    }
}

pub fn apply(
    intent: Intent,
    app: &mut App,
    client: &Client,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    in_flight: &mut Option<Task>,
) {
    match intent {
        Intent::Act(Action::Core(c)) => core(c, app, client, tx, in_flight),
        Intent::Act(Action::Edit(_) | Action::Pane(_)) | Intent::Reload => {}
        Intent::Submit(text, images) => submit(text, images, app, client, tx, in_flight),
        Intent::Search(c) => popup::type_search(app, c),
        Intent::Find(text) => popup::set_search(app, text),
        Intent::Tab(i) => popup::jump_tab(app, i),
        Intent::Pick(i) => popup::pick(app, i),
        Intent::Say(notice) => app.note(notice.text()),
        Intent::Note(note) => take_note(app, client, tx, in_flight, note),
    }
    start_shop_check(app, tx);
}

fn core(
    c: Core,
    app: &mut App,
    client: &Client,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    in_flight: &mut Option<Task>,
) {
    match c {
        Core::Quit => {
            if let Some(t) = in_flight.take() {
                t.abort();
            }
            app.shutdown_voice();
            app.should_quit = true;
        }
        Core::Help => popup::open_help(app),
        Core::Voice => popup::toggle_voice(app),
        Core::Settings => popup::toggle(app),
        Core::ViewMode => toggle_view_mode(app),
        Core::Interrupt => interrupt(app, client, tx, in_flight),
        Core::Close => popup::close(app),
        Core::Cancel => popup::cancel_field(app),
        Core::NextTab => popup::switch_tab(app),
        Core::Up => popup::move_selection(app, -1),
        Core::Down => popup::move_selection(app, 1),
        Core::Less => popup::adjust(app, -1),
        Core::More => popup::adjust(app, 1),
        Core::Activate => popup::activate(app),
        Core::SearchBack => popup::backspace_search(app),
    }
}

fn interrupt(
    app: &mut App,
    client: &Client,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    in_flight: &mut Option<Task>,
) {
    if app.voice_on && !app.voice_muted && app.in_flight {
        app.mute_voice();
        app.note("quiet");
        return;
    }
    app.clearing = false;
    app.stop_voice();
    match in_flight.take() {
        Some(t) => {
            t.abort();
            app.finish_streaming();
            app.settle_wire();
            if app.awaiting.is_empty() && app.awaiting_images.is_empty() {
                app.note("cancelled");
            } else {
                let pending = std::mem::take(&mut app.awaiting);
                let images = std::mem::take(&mut app.awaiting_images);
                start_turn(app, client, tx, in_flight, pending, images);
            }
        }
        None => {
            if !app.status.is_empty() {
                app.note("");
            }
        }
    }
}

fn submit(
    text: String,
    images: Vec<String>,
    app: &mut App,
    client: &Client,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    in_flight: &mut Option<Task>,
) {
    match app.popup.mode {
        popup::Mode::Closed => {
            let text = text.trim().to_string();
            if text.is_empty() && images.is_empty() {
                return;
            }
            if let Some(word) = crate::proword::parse(&text) {
                match word {
                    Ok(crate::proword::Proword::Cls) => cls(app, client, tx, in_flight),
                    Err(w) => app.note(format!("unknown proword /{w}")),
                }
                return;
            }
            if !app.in_flight {
                start_turn(app, client, tx, in_flight, text, images);
                return;
            }
            if !app.awaiting.is_empty() && !text.is_empty() {
                app.awaiting.push('\n');
            }
            app.awaiting.push_str(&text);
            app.awaiting_images.extend(images);
        }
        popup::Mode::SaveAs => popup::commit_save_as(app, &text),
        popup::Mode::DialEdit => popup::commit_dial_edit(app, &text),
        popup::Mode::Browse => {}
    }
}

fn cls(
    app: &mut App,
    client: &Client,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    in_flight: &mut Option<Task>,
) {
    if app.in_flight {
        app.note("let the reply finish, or Esc, then /cls");
        return;
    }
    if !crate::api::client_runs_tools() {
        app.clear_screen();
        app.note("cleared · the browser keeps no book");
        return;
    }
    let unfiled = app.engine.lock().map(|e| e.unfiled()).unwrap_or(false);
    if !unfiled {
        settle_cls(app);
        return;
    }
    if let Ok(mut engine) = app.engine.lock() {
        engine.demand_filing();
    }
    app.clearing = true;
    app.note("filing this stretch into the book…");
    launch(app, client, tx, in_flight);
}

pub(crate) fn settle_cls(app: &mut App) {
    app.clearing = false;
    if app.engine.lock().map(|e| e.unfiled()).unwrap_or(false) {
        app.note("nothing was filed in the book, so the screen stays; /cls to try again");
        return;
    }
    match app.clear_screen() {
        Some(topic) => app.note(format!("cleared · carrying on from \"{topic}\"")),
        None => app.note("cleared"),
    }
}

pub(crate) fn toggle_view_mode(app: &mut App) {
    app.view_mode = match app.view_mode {
        ViewMode::Page => ViewMode::Scroll,
        ViewMode::Scroll => ViewMode::Page,
    };
    app.note(match app.view_mode {
        ViewMode::Page => "view: page",
        ViewMode::Scroll => "view: scroll",
    });
}

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

pub fn start_shop_check(app: &mut App, tx: &mpsc::UnboundedSender<StreamEvent>) {
    let Some((station, origin)) = app.shop_check.take() else {
        return;
    };
    if let Some(t) = app.shop_check_task.take() {
        t.abort();
    }
    let mut shops = app.shops.clone();
    let tx = tx.clone();
    app.shop_check_task = Some(crate::platform::spawn(async move {
        crate::shop::discover_all(&mut shops).await;
        let _ = tx.send(StreamEvent::ShopsChecked {
            shops,
            station,
            origin,
        });
    }));
}

pub fn start_turn(
    app: &mut App,
    client: &Client,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    in_flight: &mut Option<crate::platform::Task>,
    text: String,
    mut images: Vec<String>,
) {
    images.extend(attached_images(&text));
    if app.messages.is_empty() {
        tracing::info!("window start");
    }
    app.push_user(text, images);
    app.note("");
    launch(app, client, tx, in_flight);
}

fn launch(
    app: &mut App,
    client: &Client,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    in_flight: &mut Option<crate::platform::Task>,
) {
    let _ = app.reservoir.lock().map(|mut r| r.turn_started());
    app.begin_assistant();
    app.stop_voice();
    app.unmute_voice();
    app.in_flight = true;

    let msgs = app.api_messages();
    let prev_id = app.last_response_id.clone();
    let wire = app.wire_for_turn();
    let shops = app.turn_shops();
    let station = app.active_station.clone();
    let client = client.clone();
    let engine = app.engine.clone();
    let tx = tx.clone();
    *in_flight = Some(crate::platform::spawn(async move {
        client
            .stream_completion(shops, station, msgs, prev_id, wire, engine, tx)
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
    app.note("");
    launch(app, client, tx, in_flight);
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

    fn window() -> (App, std::path::PathBuf) {
        use crate::shop::Shop;
        use crate::station::Station;
        let dir = std::env::temp_dir().join(format!("wryme_cls_{}_{}", std::process::id(), crate::windows::mint()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = App::new(None, vec![Shop::demo()], vec![Station::demo()], Station::demo(), Shop::demo(), None);
        app.engine = std::sync::Arc::new(std::sync::Mutex::new(crate::book::open_engine(&dir, "t").unwrap()));
        (app, dir)
    }

    #[test]
    fn cls_keeps_the_screen_until_the_stretch_is_filed() {
        let (mut app, dir) = window();
        app.push_user("let us plan the garden".into(), Vec::new());
        assert!(app.engine.lock().unwrap().unfiled());
        app.clearing = true;
        settle_cls(&mut app);
        assert_eq!(app.messages.len(), 1);
        assert!(app.status.contains("nothing was filed"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cls_clears_and_carries_on_from_the_page_it_filed() {
        let (mut app, dir) = window();
        app.push_user("let us plan the garden".into(), Vec::new());
        let filed = app
            .engine
            .lock()
            .unwrap()
            .deem(&crate::book::Bookmark {
                topic: "garden".into(),
                facts: vec!["roses by the gate".into()],
                ..Default::default()
            })
            .unwrap();
        assert!(filed.contains("deemed rows"));
        app.clearing = true;
        settle_cls(&mut app);
        assert!(app.messages.is_empty());
        assert!(!app.clearing);
        assert!(app.status.contains("carrying on from \"garden\""));
        assert_eq!(app.engine.lock().unwrap().preamble().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_prowords_say_so_and_send_nothing() {
        assert!(matches!(crate::proword::parse("/nope"), Some(Err(w)) if w == "nope"));
    }

    #[test]
    fn the_filing_reply_does_not_count_as_a_new_unfiled_turn() {
        let (mut app, dir) = window();
        app.push_user("eggs and oil balloons".into(), Vec::new());
        app.clearing = true;
        app.begin_assistant();
        app.engine
            .lock()
            .unwrap()
            .deem(&crate::book::Bookmark {
                topic: "egg rap".into(),
                facts: vec!["oil balloons are a joke".into()],
                ..Default::default()
            })
            .unwrap();
        app.append_to_last_assistant("Saved to egg rap.");
        app.finish_streaming();
        settle_cls(&mut app);
        assert!(app.messages.is_empty(), "screen should clear: {}", app.status);
        assert!(app.status.contains("carrying on from \"egg rap\""));
        let _ = std::fs::remove_dir_all(&dir);
    }

}
