// The "station" popup. The single place in the TUI where the user can:
//   - tune the active station (model + dials) for the current session
//   - jump to a different saved station
//   - save the active configuration under a new name
//
// State machine:
//   Closed   -> the popup is not visible
//   Browse   -> popup is open, arrow-key navigation
//   SaveAs   -> popup is open, a sub-input is collecting a name
//
// All popup actions mutate App. Dials and model changes apply to the
// next turn (the next time the user hits Enter on the main input).

use crate::app::App;
use crate::input::Input;
use crate::shop::Shop;
use crate::station::{Patience, Station};

/// Popup lifecycle state. Default is closed.
#[derive(Debug, Default)]
pub struct Popup {
    pub mode: Mode,
    /// Index of the currently-focused row when in Browse mode. The list
    /// of rows is rebuilt each frame from the current app state; the
    /// renderer clamps this to a legal value.
    pub selected: usize,
    /// Used while in SaveAs mode.
    pub name_input: Input,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Closed,
    Browse,
    SaveAs,
}

/// One row of the popup. The renderer turns these into Lines; the
/// keyboard handler dispatches based on which one is selected.
#[derive(Debug, Clone)]
pub enum Row {
    SectionHeader(&'static str),
    Model,
    Boldness,
    Patience,
    Verbosity,
    SavedStation(usize), // index into App.stations
    UpdateAction,        // only present when origin is set AND dirty
    SaveAsAction,
    Blank,
}

/// Build the row list from current app state. Order is fixed: active
/// section header, model, three dials, blank, saved header, each saved
/// station (skipping the demo placeholder), blank, conditional update
/// action, save-as action.
pub fn rows(app: &App) -> Vec<Row> {
    let mut out = vec![
        Row::SectionHeader("active"),
        Row::Model,
        Row::Boldness,
        Row::Patience,
        Row::Verbosity,
        Row::Blank,
        Row::SectionHeader("saved"),
    ];
    for (i, st) in app.stations.iter().enumerate() {
        if st.name == "demo" {
            continue;
        }
        out.push(Row::SavedStation(i));
    }
    out.push(Row::Blank);
    if app.active_origin.is_some() && app.is_dirty() {
        out.push(Row::UpdateAction);
    }
    out.push(Row::SaveAsAction);
    out
}

/// Indexes within `rows()` that represent a selectable item (not a
/// section header or blank). Arrow up/down moves between these.
pub fn selectable_indices(rows: &[Row]) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, r)| {
            !matches!(r, Row::SectionHeader(_) | Row::Blank)
        })
        .map(|(i, _)| i)
        .collect()
}

/// Open the popup if it is closed; close it if it is open. Bound to Ctrl-S.
pub fn toggle(app: &mut App) {
    match app.popup.mode {
        Mode::Closed => {
            app.popup.mode = Mode::Browse;
            // Land on the model row by default.
            app.popup.selected = first_selectable(app);
        }
        Mode::Browse | Mode::SaveAs => {
            close(app);
        }
    }
}

pub fn close(app: &mut App) {
    app.popup.mode = Mode::Closed;
    app.popup.name_input = Input::new();
    app.popup.selected = 0;
}

fn first_selectable(app: &App) -> usize {
    let r = rows(app);
    selectable_indices(&r).first().copied().unwrap_or(0)
}

/// Move the selection by `delta` (+1 / -1) through the selectable rows.
pub fn move_selection(app: &mut App, delta: i32) {
    let r = rows(app);
    let sel = selectable_indices(&r);
    if sel.is_empty() {
        return;
    }
    // Find the position of the current selection within the selectable
    // list; if it isn't there, snap to the first.
    let pos = sel
        .iter()
        .position(|&i| i == app.popup.selected)
        .unwrap_or(0);
    let new_pos = ((pos as i32 + delta).rem_euclid(sel.len() as i32)) as usize;
    app.popup.selected = sel[new_pos];
}

/// Left/right arrow on the focused row. Cycles model choices or dial
/// preset values.
pub fn adjust(app: &mut App, delta: i32) {
    let r = rows(app);
    let row = r.get(app.popup.selected).cloned();
    match row {
        Some(Row::Model) => cycle_model(app, delta),
        Some(Row::Boldness) => cycle_boldness(app, delta),
        Some(Row::Patience) => cycle_patience(app, delta),
        Some(Row::Verbosity) => cycle_verbosity(app, delta),
        _ => {}
    }
}

/// Enter on the focused row. For dial rows, the same as a right-adjust.
/// For a saved station, load it. For the update action, write current
/// state back to the saved entry. For the save-as action, switch to
/// SaveAs mode.
pub fn activate(app: &mut App) {
    let r = rows(app);
    let row = r.get(app.popup.selected).cloned();
    match row {
        Some(Row::SavedStation(idx)) => {
            if let Some(st) = app.stations.get(idx).cloned() {
                load_station(app, st);
            }
        }
        Some(Row::UpdateAction) => {
            commit_update(app);
        }
        Some(Row::SaveAsAction) => {
            app.popup.mode = Mode::SaveAs;
            app.popup.name_input = Input::new();
        }
        Some(Row::Model | Row::Boldness | Row::Patience | Row::Verbosity) => {
            adjust(app, 1);
        }
        _ => {}
    }
}

/// Commit the SaveAs name input: append a new station to the stations
/// file with the current model + dials. Then claim that name as the new
/// origin so the session becomes "clean."
pub fn commit_save_as(app: &mut App) {
    let name = app.popup.name_input.text.trim().to_string();
    if name.is_empty() {
        app.note("name can't be empty");
        return;
    }
    if app.stations.iter().any(|s| s.name == name) {
        app.note(format!("station '{}' already exists", name));
        return;
    }
    let new_station = Station {
        name: name.clone(),
        model: app.active_station.model.clone(),
        dials: app.active_station.dials,
    };
    let Some(path) = crate::station::save_path() else {
        app.note("save failed: no $HOME");
        return;
    };
    if let Err(e) = crate::station::append_to_file(&path, &new_station) {
        app.note(format!("save failed: {}", e));
        return;
    }
    app.stations.push(new_station);
    app.active_station.name = name.clone();
    app.active_origin = Some(name.clone());
    app.note(format!("saved station '{}'", name));
    app.popup.mode = Mode::Browse;
    app.popup.name_input = Input::new();
}

/// Overwrite the saved entry for `active_origin` with the current
/// active state. Surgical edit: other [[station]] blocks and comments
/// in the file stay intact.
pub fn commit_update(app: &mut App) {
    let Some(origin) = app.active_origin.clone() else {
        app.note("nothing to update; this is an untitled session");
        return;
    };
    let Some(path) = crate::station::save_path() else {
        app.note("save failed: no $HOME");
        return;
    };
    let updated = Station {
        name: origin.clone(),
        model: app.active_station.model.clone(),
        dials: app.active_station.dials,
    };
    if let Err(e) = crate::station::update_in_file(&path, &updated) {
        app.note(format!("update failed: {}", e));
        return;
    }
    // Replace the in-memory entry too.
    if let Some(saved) = app.stations.iter_mut().find(|s| s.name == origin) {
        *saved = updated;
    }
    app.note(format!("updated station '{}'", origin));
}

/// Replace the active station and re-resolve the shop for its model.
/// Sets `active_origin` so the session traces back to the loaded entry.
fn load_station(app: &mut App, st: Station) {
    let shop = crate::shop::find_for_model(&app.shops, &st.model).cloned();
    if let Some(shop) = shop {
        let name = st.name.clone();
        app.active_origin = Some(name.clone());
        app.active_station = st;
        app.active_shop = shop;
        app.last_response_id = None;
        app.note(format!("loaded station '{}'", name));
    } else {
        app.note(format!(
            "can't load '{}': no shop runs '{}'",
            st.name, st.model
        ));
    }
}

// ---- model cycling ----

fn cycle_model(app: &mut App, delta: i32) {
    let all_models = collect_models(&app.shops);
    if all_models.is_empty() {
        return;
    }
    let cur = all_models
        .iter()
        .position(|m| m == &app.active_station.model)
        .unwrap_or(0);
    let n = all_models.len() as i32;
    let next = ((cur as i32 + delta).rem_euclid(n)) as usize;
    let next_model = all_models[next].clone();
    if let Some(shop) = crate::shop::find_for_model(&app.shops, &next_model).cloned() {
        app.active_station.model = next_model;
        app.active_shop = shop;
        app.last_response_id = None;
    }
}

fn collect_models(shops: &[Shop]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in shops {
        for m in &s.models {
            if !out.contains(m) {
                out.push(m.clone());
            }
        }
    }
    out
}

// ---- dial cycling ----

const BOLDNESS_PRESETS: &[(&str, f32)] = &[
    ("mild", 0.2),
    ("balanced", 0.7),
    ("spicy", 1.2),
    ("wild", 1.8),
];

fn cycle_boldness(app: &mut App, delta: i32) {
    let states: Vec<Option<f32>> = std::iter::once(None)
        .chain(BOLDNESS_PRESETS.iter().map(|(_, v)| Some(*v)))
        .collect();
    let cur = states
        .iter()
        .position(|s| match (s, app.active_station.dials.boldness) {
            (None, None) => true,
            (Some(a), Some(b)) => (*a - b).abs() < 1e-6,
            _ => false,
        })
        .unwrap_or(0);
    let n = states.len() as i32;
    let next = ((cur as i32 + delta).rem_euclid(n)) as usize;
    app.active_station.dials.boldness = states[next];
}

pub fn boldness_label(v: Option<f32>) -> String {
    match v {
        None => "—".into(),
        Some(x) => {
            let preset = BOLDNESS_PRESETS
                .iter()
                .find(|(_, val)| (val - x).abs() < 1e-6);
            match preset {
                Some((name, _)) => format!("{} ({:.1})", name, x),
                None => format!("{:.2}", x),
            }
        }
    }
}

fn cycle_patience(app: &mut App, delta: i32) {
    let states: &[Option<Patience>] = &[
        None,
        Some(Patience::Quick),
        Some(Patience::Steady),
        Some(Patience::Slow),
    ];
    let cur = states
        .iter()
        .position(|s| *s == app.active_station.dials.patience)
        .unwrap_or(0);
    let n = states.len() as i32;
    let next = ((cur as i32 + delta).rem_euclid(n)) as usize;
    app.active_station.dials.patience = states[next];
}

pub fn patience_label(v: Option<Patience>) -> &'static str {
    match v {
        None => "—",
        Some(Patience::Quick) => "quick",
        Some(Patience::Steady) => "steady",
        Some(Patience::Slow) => "slow",
    }
}

const VERBOSITY_PRESETS: &[(&str, u32)] = &[
    ("small", 256),
    ("medium", 1024),
    ("large", 4096),
    ("heaping", 8192),
];

fn cycle_verbosity(app: &mut App, delta: i32) {
    let states: Vec<Option<u32>> = std::iter::once(None)
        .chain(VERBOSITY_PRESETS.iter().map(|(_, v)| Some(*v)))
        .collect();
    let cur = states
        .iter()
        .position(|s| *s == app.active_station.dials.verbosity)
        .unwrap_or(0);
    let n = states.len() as i32;
    let next = ((cur as i32 + delta).rem_euclid(n)) as usize;
    app.active_station.dials.verbosity = states[next];
}

pub fn verbosity_label(v: Option<u32>) -> String {
    match v {
        None => "—".into(),
        Some(x) => {
            let preset = VERBOSITY_PRESETS.iter().find(|(_, val)| *val == x);
            match preset {
                Some((name, _)) => format!("{} ({})", name, x),
                None => format!("{}", x),
            }
        }
    }
}
