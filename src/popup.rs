use crate::app::App;
use crate::shop::{Protocol, Shop, WindowMode};
use crate::station::{Brainy, Dials, Patience, Station};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Debug, Default)]
pub struct Popup {
    pub mode: Mode,
    pub tab: Tab,
    pub selected: usize,
    pub dial_idx: Option<usize>,
    pub search: String,
}

#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub enum Tab {
    #[default]
    Station,
    Shops,
    Pref,
    Help,
}

pub const TABS: [Tab; 4] = [Tab::Station, Tab::Shops, Tab::Pref, Tab::Help];

impl Tab {
    pub fn label(self) -> &'static str {
        match self {
            Tab::Station => "Station",
            Tab::Shops => "Shops",
            Tab::Pref => "Pref",
            Tab::Help => "Help",
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Closed,
    Browse,
    SaveAs,
    DialEdit,
}

#[derive(Debug, Clone)]
pub enum Row {
    SectionHeader(&'static str),
    ActiveStation,
    Model,
    Dial(usize),
    SavedStation(usize),
    Unstationed(String),
    UpdateAction,
    SaveAsAction,
    PrefVoice,
    PrefView,
    PrefAccent,
    PrefNametags,
    PrefFont,
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
            name: "brainy",
            label: |d| brainy_label(d.brainy).to_string(),
            cycle: |d, delta| cycle_brainy_dials(d, delta),
        },
        DialMeta {
            name: "tinker_clip",
            label: |d| d.tinker_clip.label(),
            cycle: |d, delta| cycle_tinker_clip_dials(d, delta),
        },
        DialMeta {
            name: "tinker_depth",
            label: |d| d.tinker_depth.label(),
            cycle: |d, delta| cycle_tinker_depth_dials(d, delta),
        },
    ]
}

pub fn rows(app: &App) -> Vec<Row> {
    match app.popup.tab {
        Tab::Station => station_rows(app),
        Tab::Pref => pref_rows(),
        Tab::Shops | Tab::Help => vec![],
    }
}

fn pref_rows() -> Vec<Row> {
    vec![
        Row::SectionHeader("preferences"),
        Row::PrefVoice,
        Row::PrefView,
        Row::PrefAccent,
        Row::PrefNametags,
        Row::PrefFont,
    ]
}

fn station_rows(app: &App) -> Vec<Row> {
    let needle = app.popup.search.to_lowercase();
    let hit = |s: &str| needle.is_empty() || s.to_lowercase().contains(&needle);
    let mut out = vec![
        Row::SectionHeader("active station"),
        Row::ActiveStation,
        Row::Model,
    ];
    for i in 0..dial_metas().len() {
        out.push(Row::Dial(i));
    }
    out.push(Row::Blank);
    out.push(Row::SectionHeader("saved stations"));
    for (i, st) in app.stations.iter().enumerate() {
        if st.name != crate::station::DEMO && (hit(&st.name) || hit(&st.model)) {
            out.push(Row::SavedStation(i));
        }
    }
    out.push(Row::Blank);
    if app.active_origin.is_some() && app.is_dirty() {
        out.push(Row::UpdateAction);
    }
    out.push(Row::SaveAsAction);
    let loose: Vec<Row> = unstationed(app)
        .into_iter()
        .filter(|(m, _)| hit(m))
        .map(|(m, _)| Row::Unstationed(m))
        .collect();
    if !loose.is_empty() {
        out.push(Row::Blank);
        out.push(Row::SectionHeader("unstationed models"));
        out.extend(loose);
    }
    out
}

pub fn unstationed(app: &App) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for shop in app.shops.iter().filter(|s| s.protocol != Protocol::Demo) {
        for m in &shop.models {
            if app
                .stations
                .iter()
                .any(|st| st.name != crate::station::DEMO && &st.model == m)
            {
                continue;
            }
            match out.iter_mut().find(|(model, _)| model == m) {
                Some((_, shops)) => shops.push(shop.name.clone()),
                None => out.push((m.clone(), vec![shop.name.clone()])),
            }
        }
    }
    out
}

pub fn shops_label(app: &App, model: &str) -> String {
    let listing = crate::shop::listing(&app.shops, model);
    if !listing.is_empty() {
        return listing
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
    }
    crate::shop::remembered(&app.shops, &app.memory, model)
        .map(|s| s.name.clone())
        .unwrap_or_else(|| "?".into())
}

pub fn active_shop_label(app: &App) -> String {
    if app.active_known() {
        app.active_shop.name.clone()
    } else {
        "?".into()
    }
}

pub fn selectable_indices(rows: &[Row]) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, r)| !matches!(r, Row::SectionHeader(_) | Row::Blank | Row::ActiveStation))
        .map(|(i, _)| i)
        .collect()
}

pub fn toggle(app: &mut App) {
    match app.popup.mode {
        Mode::Closed => {
            app.memory.extend(crate::shop_memory::load());
            app.popup.mode = Mode::Browse;
            app.popup.tab = Tab::Station;
            app.popup.selected = first_selectable(app);
        }
        Mode::Browse | Mode::SaveAs | Mode::DialEdit => {
            close(app);
        }
    }
}

pub fn close(app: &mut App) {
    app.popup.mode = Mode::Closed;
    app.popup.tab = Tab::Station;
    app.popup.dial_idx = None;
    app.popup.selected = 0;
    app.popup.search.clear();
}

pub fn cancel_field(app: &mut App) {
    if matches!(app.popup.mode, Mode::SaveAs | Mode::DialEdit) {
        app.popup.mode = Mode::Browse;
        app.popup.dial_idx = None;
    }
}

pub fn pick(app: &mut App, row: usize) {
    if app.popup.mode != Mode::Browse || !selectable_indices(&rows(app)).contains(&row) {
        return;
    }
    app.popup.selected = row;
    activate(app);
}

fn first_selectable(app: &App) -> usize {
    let r = rows(app);
    selectable_indices(&r).first().copied().unwrap_or(0)
}

pub fn move_selection(app: &mut App, delta: i32) {
    let r = rows(app);
    let sel = selectable_indices(&r);
    if sel.is_empty() {
        return;
    }
    let pos = sel
        .iter()
        .position(|&i| i == app.popup.selected)
        .unwrap_or(0);
    let new_pos = ((pos as i32 + delta).rem_euclid(sel.len() as i32)) as usize;
    app.popup.selected = sel[new_pos];
}

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
        Some(Row::PrefVoice) => toggle_voice(app),
        Some(Row::PrefView) => crate::control::toggle_view_mode(app),
        Some(Row::PrefAccent) => cycle_accent(app, delta),
        Some(Row::PrefNametags) => cycle_nametags(app, delta),
        Some(Row::PrefFont) => cycle_typeface(app, delta),
        _ => {}
    }
}

pub fn activate(app: &mut App) {
    let r = rows(app);
    let row = r.get(app.popup.selected).cloned();
    activate_row(app, row.unwrap_or(Row::Blank));
}

fn activate_row(app: &mut App, row: Row) {
    match row {
        Row::SavedStation(idx) => {
            if let Some(st) = app.stations.get(idx).cloned() {
                let origin = Some(st.name.clone());
                load_station(app, st, origin);
            }
        }
        Row::Unstationed(model) => {
            let st = Station {
                name: crate::station::UNTITLED.into(),
                model,
                dials: app.active_station.dials,
                voice: app.active_station.voice.clone(),
            };
            load_station(app, st, None);
        }
        Row::UpdateAction => {
            commit_update(app);
        }
        Row::SaveAsAction => {
            app.popup.mode = Mode::SaveAs;
        }
        Row::Dial(idx) => {
            enter_dial_edit(app, idx);
        }
        Row::Model => {
            adjust(app, 1);
        }
        Row::PrefVoice => toggle_voice(app),
        Row::PrefView => crate::control::toggle_view_mode(app),
        Row::PrefAccent => cycle_accent(app, 1),
        Row::PrefNametags => cycle_nametags(app, 1),
        Row::PrefFont => cycle_typeface(app, 1),
        _ => {}
    }
}

fn cycle_accent(app: &mut App, delta: i32) {
    let n = crate::config::ACCENTS.len() as i32;
    let i = crate::config::accent_index(app.accent) as i32;
    let (name, color) = crate::config::ACCENTS[(i + delta).rem_euclid(n) as usize];
    app.accent = color;
    let _ = crate::config::write_accent(color);
    app.note(format!("accent: {name}"));
}

fn cycle_nametags(app: &mut App, delta: i32) {
    let modes = crate::config::NAMETAG_MODES;
    let n = modes.len() as i32;
    let cur = modes
        .iter()
        .position(|(_, m)| *m == app.nametags)
        .unwrap_or(0);
    let next = ((cur as i32 + delta).rem_euclid(n)) as usize;
    let (name, mode) = modes[next];
    app.nametags = mode;
    let _ = crate::config::write_nametags(mode);
    app.note(format!("nametags: {name}"));
}

fn cycle_typeface(app: &mut App, delta: i32) {
    let faces = crate::config::TYPEFACES;
    let n = faces.len() as i32;
    let cur = faces.iter().position(|(_, f)| *f == app.typeface).unwrap_or(0) as i32;
    let (name, face) = faces[(cur + delta).rem_euclid(n) as usize];
    app.typeface = face;
    let _ = crate::config::write_typeface(face);
    app.note(format!("font: {name}"));
}

pub fn toggle_voice(app: &mut App) {
    if app.voice_on {
        app.voice_on = false;
        app.shutdown_voice();
        app.note("voice off");
    } else if crate::voice::available() {
        app.voice_on = true;
        app.unmute_voice();
        app.note("voice on");
    } else {
        app.note("voice unavailable: no say/spd-say on PATH");
    }
}

fn enter_dial_edit(app: &mut App, idx: usize) {
    if idx < dial_metas().len() {
        app.popup.dial_idx = Some(idx);
        app.popup.mode = Mode::DialEdit;
    }
}

pub fn commit_dial_edit(app: &mut App, text: &str) {
    let idx = match app.popup.dial_idx {
        Some(i) => i,
        None => return,
    };
    let text = text.trim().to_string();
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
        match meta.name {
            "tinker_clip" => app.active_station.dials.tinker_clip = val,
            "tinker_depth" => app.active_station.dials.tinker_depth = val,
            _ => {}
        }
        app.note(format!("{} = {}", meta.name, val.label()));
    }
    app.popup.mode = Mode::Browse;
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
    if let Some(pct) = s.strip_suffix('%')
        && let Ok(p) = pct.trim().parse::<u8>()
        && p <= 100
    {
        return Some(crate::station::TinkerVal::Percent(p));
    }
    if let Ok(n) = s.parse::<usize>() {
        return Some(crate::station::TinkerVal::Count(n));
    }
    None
}

pub fn commit_save_as(app: &mut App, text: &str) {
    let name = text.trim().to_string();
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
    if !crate::config::can_write() {
        app.note("save failed: no config folder connected");
        return;
    }
    if let Err(e) = crate::station_save::save_new(&new_station) {
        app.note(format!("save failed: {}", e));
        return;
    }
    app.stations.push(new_station);
    app.active_station.name = name.clone();
    app.active_origin = Some(name.clone());
    app.note(format!("saved station '{}'", name));
    app.popup.mode = Mode::Browse;
}

pub fn commit_update(app: &mut App) {
    let Some(origin) = app.active_origin.clone() else {
        app.note("nothing to update; this is an untitled session");
        return;
    };
    if !crate::config::can_write() {
        app.note("update failed: no config folder connected");
        return;
    }
    let updated = Station {
        name: origin.clone(),
        model: app.active_station.model.clone(),
        dials: app.active_station.dials,
        voice: app.active_station.voice.clone(),
    };
    if let Err(e) = crate::station_save::update(&updated) {
        app.note(format!("update failed: {}", e));
        return;
    }
    if let Some(saved) = app.stations.iter_mut().find(|s| s.name == origin) {
        *saved = updated;
    }
    app.note(format!("updated station '{}'", origin));
}

fn load_station(app: &mut App, st: Station, origin: Option<String>) {
    match crate::shop::known(&app.shops, &app.memory, &st.model).cloned() {
        Some(shop) => apply_station(app, st, origin, shop),
        None => {
            app.note(format!("looking for {} on the shops…", st.model));
            app.shop_check = Some((st, origin));
        }
    }
}

fn apply_station(app: &mut App, st: Station, origin: Option<String>, shop: Shop) {
    let label = match &origin {
        Some(name) => format!("loaded station '{name}'"),
        None => format!("loaded {}", st.model),
    };
    app.active_origin = origin;
    app.active_station = st;
    app.active_shop = shop;
    app.last_response_id = None;
    app.forget_wire();
    if app.active_known() {
        app.note(label);
    } else {
        app.note_undiscovered();
    }
}

pub fn finish_shop_check(app: &mut App, shops: Vec<Shop>, st: Station, origin: Option<String>) {
    for fresh in shops {
        if app.active_shop.name == fresh.name {
            app.active_shop.models = fresh.models.clone();
        }
        if let Some(s) = app.shops.iter_mut().find(|s| s.name == fresh.name) {
            s.models = fresh.models;
        }
    }
    let current = app.active_shop.name.clone();
    match crate::shop::choose(&app.shops, &app.memory, &st.model, &current) {
        Some(shop) => apply_station(app, st, origin, shop),
        None => app.note(format!(
            "can't load '{}': no shop's models list has {}",
            st.name, st.model
        )),
    }
}

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
        app.forget_wire();
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

pub fn brainy_label(v: Option<Brainy>) -> &'static str {
    match v {
        None => "—",
        Some(b) => b.label(),
    }
}

const TINKER_CLIP_PRESETS: &[crate::station::TinkerVal] = &[
    crate::station::TinkerVal::All,
    crate::station::TinkerVal::Count(0),
    crate::station::TinkerVal::Count(300),
    crate::station::TinkerVal::Count(1000),
    crate::station::TinkerVal::Percent(50),
];

const TINKER_DEPTH_PRESETS: &[crate::station::TinkerVal] = &[
    crate::station::TinkerVal::All,
    crate::station::TinkerVal::Count(2),
    crate::station::TinkerVal::Count(4),
    crate::station::TinkerVal::Count(8),
    crate::station::TinkerVal::Count(16),
    crate::station::TinkerVal::Count(32),
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

pub fn cycle_brainy_dials(dials: &mut Dials, delta: i32) {
    let states: &[Option<Brainy>] = &[
        None,
        Some(Brainy::Hush),
        Some(Brainy::Murmur),
        Some(Brainy::Chatty),
        Some(Brainy::Gabby),
    ];
    let cur = states.iter().position(|s| *s == dials.brainy).unwrap_or(0);
    let n = states.len() as i32;
    let next = ((cur as i32 + delta).rem_euclid(n)) as usize;
    dials.brainy = states[next];
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

pub fn cycle_tinker_depth_dials(dials: &mut Dials, delta: i32) {
    let cur = TINKER_DEPTH_PRESETS
        .iter()
        .position(|s| *s == dials.tinker_depth)
        .unwrap_or(0);
    let n = TINKER_DEPTH_PRESETS.len() as i32;
    let next = ((cur as i32 + delta).rem_euclid(n)) as usize;
    dials.tinker_depth = TINKER_DEPTH_PRESETS[next];
}

pub fn switch_tab(app: &mut App) {
    let at = TABS.iter().position(|t| *t == app.popup.tab).unwrap_or(0);
    enter_tab(app, TABS[(at + 1) % TABS.len()]);
}

pub fn jump_tab(app: &mut App, idx: usize) {
    if let Some(tab) = TABS.get(idx) {
        enter_tab(app, *tab);
    }
}

fn enter_tab(app: &mut App, tab: Tab) {
    app.popup.tab = tab;
    app.popup.mode = Mode::Browse;
    app.popup.dial_idx = None;
    app.popup.search.clear();
    app.popup.selected = first_selectable(app);
}

pub fn type_search(app: &mut App, c: char) {
    if app.popup.mode != Mode::Browse || app.popup.tab != Tab::Station {
        return;
    }
    app.popup.search.push(c);
    retarget_search(app);
}

pub fn set_search(app: &mut App, text: String) {
    if app.popup.mode != Mode::Browse || app.popup.tab != Tab::Station {
        return;
    }
    app.popup.search = text;
    retarget_search(app);
}

pub fn backspace_search(app: &mut App) {
    if app.popup.search.is_empty() {
        return;
    }
    app.popup.search.pop();
    retarget_search(app);
}

fn retarget_search(app: &mut App) {
    if app.popup.search.is_empty() {
        return;
    }
    for (i, row) in station_rows(app).iter().enumerate() {
        if matches!(row, Row::SavedStation(_) | Row::Unstationed(_)) {
            app.popup.selected = i;
            return;
        }
    }
}

pub fn open_help(app: &mut App) {
    enter_tab(app, Tab::Help);
}

pub fn shops_lines(app: &App) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for (i, s) in app.shops.iter().enumerate() {
        let active = s.name == app.active_shop.name;
        let tag = if active { "▶ " } else { "  " };
        let name_style = if active {
            Style::default().fg(app.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.accent)
        };
        out.push(Line::from(vec![
            Span::styled(format!("{tag}{}", s.name), name_style),
            Span::styled(
                format!("   {}/{}", protocol_label(s.protocol), window_label(s.window)),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
        let url = if s.url.is_empty() {
            "local (no network)".to_string()
        } else {
            s.url.clone()
        };
        out.push(Line::from(Span::styled(
            format!("    url     {url}"),
            Style::default().fg(Color::DarkGray),
        )));
        out.push(Line::from(Span::styled(
            format!("    models  {}", s.models.join(", ")),
            Style::default().fg(Color::DarkGray),
        )));
        let key = if s.key.is_empty() {
            "(none)"
        } else {
            "(set)"
        };
        out.push(Line::from(Span::styled(
            format!("    key     {key}"),
            Style::default().fg(Color::DarkGray),
        )));
        if i + 1 < app.shops.len() {
            out.push(Line::from(""));
        }
    }
    if app.shops.is_empty() {
        out.push(Line::from(Span::styled(
            "  no shops configured",
            Style::default().fg(Color::DarkGray),
        )));
    }
    out
}

fn protocol_label(p: Protocol) -> &'static str {
    match p {
        Protocol::Demo => "demo",
        Protocol::ChatCompletions => "chat",
        Protocol::Responses => "responses",
    }
}

fn window_label(w: WindowMode) -> &'static str {
    match w {
        WindowMode::Full => "full",
        WindowMode::Warm => "warm",
    }
}
