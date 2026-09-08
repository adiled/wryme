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
use crate::station::{Dials, Patience, Station};

/// Popup lifecycle state. Default is closed.
#[derive(Debug, Default)]
pub struct Popup {
    pub mode: Mode,
    /// Which BIOS-style tab the popup is showing. Station is the tuning
    /// list; Help lists the shortcuts.
    pub tab: Tab,
    /// Index of the currently-focused row when in Browse mode. The list
    /// of rows is rebuilt each frame from the current app state; the
    /// renderer clamps this to a legal value.
    pub selected: usize,
    /// Vertical scroll offset for the popup body, in rows.
    pub scroll: usize,
    /// Used while in SaveAs mode.
    pub name_input: Input,
    /// Used while editing a tinker dial freeform.
    pub dial_input: Input,
    pub dial_idx: Option<usize>,
}

#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub enum Tab {
    #[default]
    Station,
    Help,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Closed,
    Browse,
    SaveAs,
    DialEdit,
}

/// One row of the popup. The renderer turns these into Lines; the
/// keyboard handler dispatches based on which one is selected.
#[derive(Debug, Clone)]
pub enum Row {
    SectionHeader(&'static str),
    Model,
    Dial(usize),         // index into DIALS — automatically renders from Station::Dials
    SavedStation(usize), // index into App.stations
    UpdateAction,        // only present when origin is set AND dirty
    SaveAsAction,
    Blank,
}

pub struct DialMeta {
    pub name: &'static str,
    pub label: fn(&Dials) -> String,
    pub cycle: fn(&mut Dials, i32),
}

pub fn dial_metas() -> Vec<DialMeta> {
    vec![
        DialMeta {
            name: "boldness",
            label: |d| boldness_label(d.boldness),
            cycle: |d, delta| cycle_boldness_dials(d, delta),
        },
        DialMeta {
            name: "patience",
            label: |d| patience_label(d.patience).to_string(),
            cycle: |d, delta| cycle_patience_dials(d, delta),
        },
        DialMeta {
            name: "verbosity",
            label: |d| verbosity_label(d.verbosity),
            cycle: |d, delta| cycle_verbosity_dials(d, delta),
        },
        DialMeta {
            name: "tinker_keep",
            label: |d| d.tinker_keep.label(),
            cycle: |d, delta| cycle_tinker_keep_dials(d, delta),
        },
        DialMeta {
            name: "tinker_clip",
            label: |d| d.tinker_clip.label(),
            cycle: |d, delta| cycle_tinker_clip_dials(d, delta),
        },
    ]
}

/// Build the row list from current app state. Order is fixed: active
/// section header, model, dials (auto from Dials), blank, saved header, each saved
/// station (skipping the demo placeholder), blank, conditional update
/// action, save-as action.
pub fn rows(app: &App) -> Vec<Row> {
    let mut out = vec![Row::SectionHeader("active"), Row::Model];
    for i in 0..dial_metas().len() {
        out.push(Row::Dial(i));
    }
    out.push(Row::Blank);
    out.push(Row::SectionHeader("saved"));
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
        .filter(|(_, r)| !matches!(r, Row::SectionHeader(_) | Row::Blank))
        .map(|(i, _)| i)
        .collect()
}

/// Open the popup if it is closed; close it if it is open. Bound to Ctrl-S.
pub fn toggle(app: &mut App) {
    match app.popup.mode {
        Mode::Closed => {
            app.popup.mode = Mode::Browse;
            app.popup.tab = Tab::Station;
            // Land on the model row by default.
            app.popup.selected = first_selectable(app);
            app.popup.scroll = 0;
        }
        Mode::Browse | Mode::SaveAs | Mode::DialEdit => {
            close(app);
        }
    }
}

pub fn close(app: &mut App) {
    app.popup.mode = Mode::Closed;
    app.popup.tab = Tab::Station;
    app.popup.name_input = Input::new();
    app.popup.dial_input = Input::new();
    app.popup.dial_idx = None;
    app.popup.selected = 0;
    app.popup.scroll = 0;
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
        Some(Row::Dial(idx)) => {
            if let Some(meta) = dial_metas().get(idx) {
                (meta.cycle)(&mut app.active_station.dials, delta);
            }
        }
        _ => {}
    }
}

/// Enter on the focused row. Dial rows enter freeform edit (type a number / all / 50%).
/// Saved station, update, save-as as before. Model still cycles.
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
        Some(Row::Dial(idx)) => {
            enter_dial_edit(app, idx);
        }
        Some(Row::Model) => {
            adjust(app, 1);
        }
        _ => {}
    }
}

fn enter_dial_edit(app: &mut App, idx: usize) {
    if let Some(meta) = dial_metas().get(idx) {
        let cur = (meta.label)(&app.active_station.dials);
        let mut input = Input::new();
        input.text = cur;
        // place cursor at end
        input.home();
        for _ in 0..input.text.len() {
            input.end();
        }
        app.popup.dial_input = input;
        app.popup.dial_idx = Some(idx);
        app.popup.mode = Mode::DialEdit;
    }
}

pub fn commit_dial_edit(app: &mut App) {
    let idx = match app.popup.dial_idx {
        Some(i) => i,
        None => return,
    };
    let text = app.popup.dial_input.text.trim().to_string();
    if text.is_empty() {
        app.note("tinker value can't be empty");
        return;
    }
    let parsed = parse_tinker_val(&text);
    let Some(val) = parsed else {
        app.note("invalid tinker value: use all, 0, 12, or 50%");
        return;
    };
    if let Some(meta) = dial_metas().get(idx) {
        // Both tinker_keep and tinker_clip share TinkerVal, so we set via dial cycle to the exact value
        // by directly assigning.
        match meta.name {
            "tinker_keep" => app.active_station.dials.tinker_keep = val,
            "tinker_clip" => app.active_station.dials.tinker_clip = val,
            _ => {}
        }
        app.note(format!("{} = {}", meta.name, val.label()));
    }
    app.popup.mode = Mode::Browse;
    app.popup.dial_input = Input::new();
    app.popup.dial_idx = None;
}

fn parse_tinker_val(s: &str) -> Option<crate::station::TinkerVal> {
    let s = s.trim().to_lowercase();
    if s == "all" {
        return Some(crate::station::TinkerVal::All);
    }
    if s == "0" {
        return Some(crate::station::TinkerVal::Count(0));
    }
    if let Some(pct) = s.strip_suffix('%') {
        if let Ok(p) = pct.trim().parse::<u8>() {
            if p <= 100 {
                return Some(crate::station::TinkerVal::Percent(p));
            }
        }
    }
    if let Ok(n) = s.parse::<usize>() {
        return Some(crate::station::TinkerVal::Count(n));
    }
    None
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
        voice: app.active_station.voice.clone(),
    };
    let Some(path) = crate::station::config_path() else {
        app.note("save failed: no $HOME");
        return;
    };
    if let Err(e) = crate::station_save::append_to_file(&path, &new_station) {
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
    let Some(path) = crate::station::config_path() else {
        app.note("save failed: no $HOME");
        return;
    };
    let updated = Station {
        name: origin.clone(),
        model: app.active_station.model.clone(),
        dials: app.active_station.dials,
        voice: app.active_station.voice.clone(),
    };
    if let Err(e) = crate::station_save::update_in_file(&path, &updated) {
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

pub fn patience_label(v: Option<Patience>) -> &'static str {
    match v {
        None => "—",
        Some(p) => p.label(),
    }
}

const VERBOSITY_PRESETS: &[(&str, u32)] = &[
    ("small", 256),
    ("medium", 1024),
    ("large", 4096),
    ("heaping", 8192),
];

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

const TINKER_KEEP_PRESETS: &[crate::station::TinkerVal] = &[
    crate::station::TinkerVal::All,
    crate::station::TinkerVal::Count(0),
    crate::station::TinkerVal::Count(1),
    crate::station::TinkerVal::Count(3),
    crate::station::TinkerVal::Count(8),
    crate::station::TinkerVal::Count(16),
    crate::station::TinkerVal::Percent(50),
];

const TINKER_CLIP_PRESETS: &[crate::station::TinkerVal] = &[
    crate::station::TinkerVal::All,
    crate::station::TinkerVal::Count(0),
    crate::station::TinkerVal::Count(300),
    crate::station::TinkerVal::Count(1000),
    crate::station::TinkerVal::Percent(50),
];

pub fn cycle_boldness_dials(dials: &mut Dials, delta: i32) {
    let states: Vec<Option<f32>> = std::iter::once(None)
        .chain(BOLDNESS_PRESETS.iter().map(|(_, v)| Some(*v)))
        .collect();
    let cur = states
        .iter()
        .position(|s| match (s, dials.boldness) {
            (None, None) => true,
            (Some(a), Some(b)) => (*a - b).abs() < 1e-6,
            _ => false,
        })
        .unwrap_or(0);
    let n = states.len() as i32;
    let next = ((cur as i32 + delta).rem_euclid(n)) as usize;
    dials.boldness = states[next];
}

pub fn cycle_patience_dials(dials: &mut Dials, delta: i32) {
    let states: &[Option<Patience>] = &[
        None,
        Some(Patience::Bare),
        Some(Patience::Swift),
        Some(Patience::Quick),
        Some(Patience::Steady),
        Some(Patience::Slow),
        Some(Patience::Deep),
        Some(Patience::Max),
    ];
    let cur = states
        .iter()
        .position(|s| *s == dials.patience)
        .unwrap_or(0);
    let n = states.len() as i32;
    let next = ((cur as i32 + delta).rem_euclid(n)) as usize;
    dials.patience = states[next];
}

pub fn cycle_verbosity_dials(dials: &mut Dials, delta: i32) {
    let states: Vec<Option<u32>> = std::iter::once(None)
        .chain(VERBOSITY_PRESETS.iter().map(|(_, v)| Some(*v)))
        .collect();
    let cur = states
        .iter()
        .position(|s| *s == dials.verbosity)
        .unwrap_or(0);
    let n = states.len() as i32;
    let next = ((cur as i32 + delta).rem_euclid(n)) as usize;
    dials.verbosity = states[next];
}

pub fn cycle_tinker_keep_dials(dials: &mut Dials, delta: i32) {
    let cur = TINKER_KEEP_PRESETS
        .iter()
        .position(|s| *s == dials.tinker_keep)
        .unwrap_or(0);
    let n = TINKER_KEEP_PRESETS.len() as i32;
    let next = ((cur as i32 + delta).rem_euclid(n)) as usize;
    dials.tinker_keep = TINKER_KEEP_PRESETS[next];
}

pub fn cycle_tinker_clip_dials(dials: &mut Dials, delta: i32) {
    let cur = TINKER_CLIP_PRESETS
        .iter()
        .position(|s| *s == dials.tinker_clip)
        .unwrap_or(0);
    let n = TINKER_CLIP_PRESETS.len() as i32;
    let next = ((cur as i32 + delta).rem_euclid(n)) as usize;
    dials.tinker_clip = TINKER_CLIP_PRESETS[next];
}

/// Cycle the tab bar: Station <-> Help. Returns to Browse mode and
/// snaps the selection to the top of the new tab.
pub fn switch_tab(app: &mut App) {
    app.popup.tab = match app.popup.tab {
        Tab::Station => Tab::Help,
        Tab::Help => Tab::Station,
    };
    app.popup.mode = Mode::Browse;
    app.popup.name_input = Input::new();
    app.popup.selected = 0;
    app.popup.scroll = 0;
}

/// Open the popup with the Help tab selected (bound to F1). If the
/// popup is already open, just switch to Help.
pub fn open_help(app: &mut App) {
    if app.popup.mode == Mode::Closed {
        app.popup.mode = Mode::Browse;
        app.popup.selected = 0;
    }
    app.popup.tab = Tab::Help;
    app.popup.mode = Mode::Browse;
    app.popup.name_input = Input::new();
    app.popup.scroll = 0;
}

/// Scroll the popup body by `delta` rows. Clamped by the renderer each
/// frame, but we keep the offset sane here too.
pub fn scroll(app: &mut App, delta: i32) {
    let r = if delta > 0 {
        app.popup.scroll.saturating_add(delta as usize)
    } else {
        app.popup.scroll.saturating_sub((-delta) as usize)
    };
    app.popup.scroll = r;
}

/// Static shortcut list for the Help tab. One line per binding, kept in
/// roughly the order they appear in keys.rs. The renderer shows this as
/// a two-column table: key on the left, meaning on the right.
pub fn help_rows() -> Vec<(String, String)> {
    vec![
        ("Enter".into(), "send input".into()),
        ("Esc".into(), "stop a streaming reply / clear note".into()),
        ("Ctrl-C".into(), "quit immediately".into()),
        ("Ctrl-T".into(), "toggle page / scroll view".into()),
        ("Ctrl-V".into(), "read replies aloud on / off".into()),
        ("PgUp / PgDn".into(), "page or scroll up / down".into()),
        ("← / →".into(), "move the input cursor".into()),
        ("Home / End".into(), "jump to input start / end".into()),
        (
            "Backspace / Delete".into(),
            "delete before / after cursor".into(),
        ),
        ("Ctrl-A / Ctrl-E".into(), "jump to input start / end".into()),
        ("Ctrl-U".into(), "kill to start of line".into()),
        ("Ctrl-K".into(), "kill to end of line".into()),
        ("Ctrl-W".into(), "kill previous word".into()),
        ("Ctrl-S".into(), "open / close this popup".into()),
        ("Tab / F1".into(), "switch Station / Help tab".into()),
        ("Mouse wheel".into(), "scroll in page / scroll view".into()),
        ("".into(), "".into()),
        ("In the Station tab:".into(), "".into()),
        ("↑ / ↓".into(), "move between selectable rows".into()),
        ("← / →".into(), "adjust model / dials (also Enter)".into()),
        ("Enter".into(), "load station / update / save-as".into()),
        ("PgUp / PgDn".into(), "scroll the popup body".into()),
        ("Esc".into(), "close popup".into()),
        ("".into(), "".into()),
        ("In the Help tab:".into(), "".into()),
        ("Esc / Tab".into(), "leave Help back to Station".into()),
        ("F1".into(), "open Help tab from anywhere".into()),
    ]
}
