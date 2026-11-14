use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde::{Serialize, Serializer};

use crate::action::Scope;
use crate::app::{App, Message, Phase, Role, ViewMode};
use crate::config::{NametagMode, Typeface};
use crate::popup::{self, Tab};
use crate::shop::Protocol;

pub const BRAIN_MARK: &str = "✶";
pub const HEART_MARK: &str = "\u{2764}\u{FE0E}";
pub const USER_MARK: &str = "\u{23F9}\u{FE0E}";
pub const DOT: &str = " • ";
pub const AWAITING: &str = "awaiting";

#[derive(Serialize)]
pub struct Scene {
    pub window: String,
    #[serde(serialize_with = "ink")]
    pub accent: Color,
    pub nametags: NametagMode,
    pub font: Typeface,
    pub view: ViewMode,
    pub scope: Scope,
    pub turn: u64,
    pub marks: Marks,
    pub sync: crate::sync::Sync,
    pub bar: Bar,
    pub bubbles: Vec<Bubble>,
    pub status: Status,
    pub error: Option<String>,
    pub sheet: Option<Sheet>,
}

#[derive(Serialize)]
pub struct Marks {
    pub brain: &'static str,
    pub heart: &'static str,
}

#[derive(Serialize)]
pub struct Bar {
    pub state: &'static str,
    pub esc: &'static str,
    pub busy: bool,
    pub tool: Option<String>,
}

#[derive(Serialize)]
pub struct Bubble {
    pub id: String,
    pub user: bool,
    pub name: String,
    pub guest: Option<String>,
    #[serde(serialize_with = "ink")]
    pub ink: Color,
    pub doing: Option<&'static str>,
    pub mark: &'static str,
    pub blinks: bool,
    pub thinking_live: bool,
    pub tool: Option<String>,
    pub waiting: bool,
    pub live: bool,
    pub time: String,
    pub images: Vec<String>,
    pub orphan: bool,
    #[serde(serialize_with = "rows")]
    pub reply: Vec<Line<'static>>,
    #[serde(serialize_with = "rows")]
    pub brain: Vec<Line<'static>>,
    #[serde(serialize_with = "rows")]
    pub heart: Vec<Line<'static>>,
}

#[derive(Serialize)]
pub struct Status {
    #[serde(serialize_with = "runs")]
    pub left: Vec<Span<'static>>,
    #[serde(serialize_with = "run")]
    pub note: Span<'static>,
    #[serde(serialize_with = "runs")]
    pub right: Vec<Span<'static>>,
}

#[derive(Serialize)]
pub struct Sheet {
    pub tabs: Vec<Pill>,
    #[serde(serialize_with = "rows")]
    pub lines: Vec<Line<'static>>,
    pub picks: Vec<usize>,
    pub selected: Option<usize>,
    pub hint: &'static str,
    pub field: Option<Field>,
}

#[derive(Serialize)]
pub struct Pill {
    pub label: &'static str,
    pub on: bool,
}

#[derive(Serialize)]
pub struct Field {
    pub label: String,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Activity {
    Idle,
    Streaming,
    Writing,
    Thinking,
    Communing,
    Tinkering,
    Reminiscing,
    Hidden,
}

impl Activity {
    fn of(msg: &Message) -> Self {
        if !msg.streaming {
            return Self::Idle;
        }
        let tool = msg.current_tool.as_deref();
        let hidden = tool.is_some_and(crate::tools::is_hidden_tool);
        let bookish = tool.is_some_and(crate::tools::is_book_tool);
        match msg.phase {
            Phase::Streaming => Self::Streaming,
            Phase::Writing => Self::Writing,
            Phase::Thinking => Self::Thinking,
            Phase::Communing => Self::Communing,
            Phase::Tinkering if hidden && bookish => Self::Reminiscing,
            Phase::Tinkering if hidden => Self::Hidden,
            Phase::Tinkering => Self::Tinkering,
        }
    }

    fn doing(self) -> Option<&'static str> {
        match self {
            Self::Writing => Some("writing…"),
            Self::Thinking => Some("thinking…"),
            Self::Communing => Some("communing…"),
            Self::Tinkering => Some("tinkering…"),
            Self::Reminiscing => Some("reminiscing…"),
            Self::Idle | Self::Streaming | Self::Hidden => None,
        }
    }
}

fn mark(msg: &Message) -> (&'static str, bool) {
    if msg.role == Role::User {
        return (USER_MARK, false);
    }
    match Activity::of(msg) {
        Activity::Idle => ("⌃", false),
        Activity::Thinking => ("✦", true),
        Activity::Reminiscing => ("಄", false),
        Activity::Tinkering | Activity::Hidden | Activity::Communing => ("‣", true),
        Activity::Writing | Activity::Streaming => ("⬩", true),
    }
}

pub fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

pub fn cursor() -> Span<'static> {
    Span::styled("▌", dim())
}

fn plain(text: &str, style: Style, live: bool) -> Vec<Line<'static>> {
    let last = text.split('\n').count().saturating_sub(1);
    text.split('\n')
        .enumerate()
        .map(|(i, raw)| {
            let mut spans = vec![Span::styled(raw.to_string(), style)];
            if i == last && live {
                spans.push(cursor());
            }
            Line::from(spans)
        })
        .collect()
}

fn thought(text: &str, live: bool) -> Vec<Line<'static>> {
    if text.is_empty() {
        return Vec::new();
    }
    plain(text, dim().add_modifier(Modifier::ITALIC), live)
}

pub fn bubble(id: String, msg: &Message, thinking_live: bool) -> Bubble {
    let replied = !msg.content.is_empty();
    let brained = !msg.brain.is_empty();
    let hearted = !msg.heart.is_empty();
    let live = msg.streaming;
    let reply = match (replied, msg.role) {
        (false, _) => Vec::new(),
        (true, Role::Assistant) => crate::md::render(&msg.content, live),
        (true, Role::User) => plain(&msg.content, Style::default(), live),
    };
    let (mark, blinks) = mark(msg);
    Bubble {
        id,
        user: msg.role == Role::User,
        name: match (&msg.role, &msg.guest) {
            (Role::User, Some(from)) => format!("guest@{from}"),
            (Role::User, None) => "you".to_string(),
            (Role::Assistant, _) => "assistant".to_string(),
        },
        guest: msg.guest.clone(),
        ink: match msg.role {
            Role::User => Color::Green,
            Role::Assistant => Color::Magenta,
        },
        doing: Activity::of(msg).doing(),
        mark,
        blinks,
        thinking_live,
        tool: msg
            .current_tool
            .clone()
            .filter(|name| live && !crate::tools::is_hidden_tool(name)),
        waiting: msg.waiting,
        live,
        time: msg.timestamp.clone(),
        images: if msg.role == Role::User {
            msg.images.clone()
        } else {
            Vec::new()
        },
        orphan: live && !replied && !brained && !hearted,
        reply,
        brain: thought(&msg.brain, live && !replied),
        heart: thought(&msg.heart, live && !replied && !brained),
    }
}

fn awaiting(app: &App) -> Option<Message> {
    (!app.awaiting.is_empty() || !app.awaiting_images.is_empty()).then(|| Message {
        role: Role::User,
        content: app.awaiting.clone(),
        images: app.awaiting_images.clone(),
        guest: None,
        brain: String::new(),
        heart: String::new(),
        streaming: false,
        waiting: true,
        timestamp: String::new(),
        phase: Phase::Streaming,
        current_tool: None,
        turn_id: 0,
        tool_events: Vec::new(),
    })
}

fn thinking_turn(messages: &[Message]) -> Option<u64> {
    messages
        .iter()
        .rev()
        .find(|m| m.role == Role::Assistant && m.streaming && m.phase == Phase::Thinking)
        .map(|m| m.turn_id)
}

pub fn scope(app: &App) -> Scope {
    match app.popup.mode {
        popup::Mode::Closed => Scope::Chat,
        popup::Mode::Browse => Scope::Sheet,
        popup::Mode::SaveAs | popup::Mode::DialEdit => Scope::Field,
    }
}

pub fn build(app: &App) -> Scene {
    let live = thinking_turn(&app.messages);
    let bubbles = awaiting(app)
        .map(|m| bubble(AWAITING.to_string(), &m, false))
        .into_iter()
        .chain(
            app.messages
                .iter()
                .enumerate()
                .rev()
                .map(|(i, m)| bubble(i.to_string(), m, live == Some(m.turn_id))),
        )
        .collect();
    Scene {
        window: crate::windows::tag(&app.window_id),
        accent: app.accent,
        nametags: app.nametags,
        font: app.typeface,
        view: app.view_mode,
        scope: scope(app),
        turn: app.turn(),
        marks: Marks {
            brain: BRAIN_MARK,
            heart: HEART_MARK,
        },
        sync: app.sync.clone(),
        bar: bar(app),
        bubbles,
        status: status(app),
        error: is_error_status(&app.status).then(|| app.status.clone()),
        sheet: (app.popup.mode != popup::Mode::Closed).then(|| sheet(app)),
    }
}

fn bar(app: &App) -> Bar {
    let (state, esc) = match (app.in_flight, app.voice_is_active()) {
        (true, true) => ("streaming + speaking…", "esc quiet"),
        (true, false) => ("streaming…", "esc interrupt"),
        (false, true) => ("speaking…", "esc quiet"),
        (false, false) => ("", ""),
    };
    let tool = (app.nametags == NametagMode::Inlined)
        .then(|| {
            app.messages
                .iter()
                .rev()
                .find(|m| m.role == Role::Assistant && m.streaming)?
                .current_tool
                .clone()
                .filter(|name| !crate::tools::is_hidden_tool(name))
        })
        .flatten();
    Bar {
        state,
        esc,
        busy: app.in_flight,
        tool,
    }
}

fn status(app: &App) -> Status {
    let is_demo = app.active_shop.protocol == Protocol::Demo;
    let dirty = app.is_dirty();
    let station_label = match (&app.active_origin, dirty) {
        (Some(origin), true) => format!("tuned from {}", origin),
        (Some(origin), false) => origin.clone(),
        (None, _) => app.active_station.name.clone(),
    };
    let station_color = if is_demo {
        Color::Yellow
    } else if dirty {
        Color::Magenta
    } else {
        app.accent
    };
    let mut left = vec![
        Span::styled(
            crate::windows::tag(&app.window_id),
            Style::default().fg(app.accent),
        ),
        Span::raw(DOT),
        Span::styled(
            format!("station: {}", station_label),
            Style::default().fg(station_color),
        ),
        Span::styled(format!(" via {}", app.active_shop.name), dim()),
    ];
    #[cfg(feature = "reservoir")]
    {
        let heart_score = app
            .reservoir
            .lock()
            .ok()
            .map(|r| r.static_figure())
            .unwrap_or(0);
        let heart_color = hue_lit(120.0 * (1.0 - heart_tilt(heart_score)));
        left.push(Span::styled(format!(" {HEART_MARK} "), heart_color));
    }
    #[cfg(not(feature = "reservoir"))]
    {
        left.push(Span::raw(DOT));
    }
    left.push(Span::raw(format!("{} msg", app.messages.len())));
    if app.voice_on {
        left.push(Span::raw(DOT));
        left.push(Span::styled("voice", Style::default().fg(app.accent)));
    }
    let note = Span::styled(
        app.status.clone(),
        Style::default().fg(if is_error_status(&app.status) {
            Color::Red
        } else {
            Color::Gray
        }),
    );
    let used = app.usage_ctx + app.usage_out;
    let mut right: Vec<Span<'static>> = Vec::new();
    if used > 0 {
        let fill = app
            .reservoir
            .lock()
            .ok()
            .and_then(|r| r.dip(&app.active_station.name))
            .unwrap_or(0.0);
        let color = if fill >= 0.5 {
            let t = ((fill - 0.5) / 0.5).clamp(0.0, 1.0);
            Color::Rgb(255, (255.0 * (1.0 - t)) as u8, 0)
        } else {
            Color::DarkGray
        };
        right.push(Span::styled(format_k(used), Style::default().fg(color)));
        if let Some(pct) = cache_pct(app.usage_cached, app.usage_ctx) {
            let cache_color = if pct >= 50 {
                Color::DarkGray
            } else if pct > 0 {
                Color::Yellow
            } else {
                Color::Red
            };
            right.push(Span::styled(
                format!(" cache {pct}%"),
                Style::default().fg(cache_color),
            ));
        }
    }
    Status { left, note, right }
}

fn cache_pct(cached: u64, input: u64) -> Option<u64> {
    (input > 0 && cached > 0).then(|| (cached * 100 / input).min(100))
}

fn format_k(n: u64) -> String {
    if n < 1000 {
        return n.to_string();
    }
    if n < 999_500 {
        return format!("{}K", (n + 500) / 1000);
    }
    format!("{}M", (n + 500_000) / 1_000_000)
}

pub fn is_error_status(s: &str) -> bool {
    s.starts_with("error")
        || s.starts_with("upstream")
        || s.starts_with("stopped:")
        || s.starts_with("save failed")
        || s.starts_with("update failed")
}

#[cfg(feature = "reservoir")]
fn heart_tilt(score: u32) -> f64 {
    const NEUTRAL: f64 = 50.0;
    let top = crate::reservoir::STATIC_BOT_SCORE as f64;
    ((score as f64 - NEUTRAL) / (top - NEUTRAL)).clamp(0.0, 1.0)
}

#[cfg(feature = "reservoir")]
fn hue_lit(hue: f64) -> Color {
    const LIGHTNESS: f64 = 0.45;
    const SATURATION: f64 = 0.85;
    const HUE_SECTOR_DEGREES: f64 = 60.0;

    let l = LIGHTNESS;
    let s = SATURATION;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((hue / HUE_SECTOR_DEGREES) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (hue.rem_euclid(360.0) / HUE_SECTOR_DEGREES).floor() as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    Color::Rgb(
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

fn focus(selected: bool, accent: Color) -> Style {
    if selected {
        Style::default()
            .fg(Color::Black)
            .bg(accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

fn pair(style: Style, name: &str, value: Span<'static>) -> Line<'static> {
    Line::from(vec![
        Span::styled("  ", style),
        Span::styled(name.to_string(), style),
        value,
    ])
}

fn sheet(app: &App) -> Sheet {
    let accent = app.accent;
    let tabs = popup::TABS
        .iter()
        .map(|tab| Pill {
            label: tab.label(),
            on: app.popup.tab == *tab,
        })
        .collect();
    let browsing = app.popup.mode == popup::Mode::Browse;
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut picks = Vec::new();
    let mut selected = None;
    let mut field = None;
    let hint = match app.popup.tab {
        Tab::Shops => {
            lines = popup::shops_lines(app);
            " read-only · edit ~/.config/wryme/shops.toml · Esc / Ctrl-S close "
        }
        Tab::Help => {
            for (key, what) in crate::action::help() {
                if what.is_empty() {
                    lines.push(Line::from(Span::styled(
                        format!("  {key}"),
                        Style::default().fg(accent).add_modifier(Modifier::BOLD),
                    )));
                } else {
                    lines.push(Line::from(vec![
                        Span::styled(format!("  {:<22}", key), Style::default().fg(accent)),
                        Span::raw(what),
                    ]));
                }
            }
            " Esc close  ·  Tab next tab "
        }
        Tab::Station | Tab::Pref => {
            let rows = popup::rows(app);
            picks = popup::selectable_indices(&rows);
            if browsing {
                selected = Some(app.popup.selected);
            }
            for (i, row) in rows.iter().enumerate() {
                let style = focus(browsing && i == app.popup.selected, accent);
                lines.push(sheet_row(app, row, style));
            }
            field = match app.popup.mode {
                popup::Mode::SaveAs => Some(Field {
                    label: "name".into(),
                    text: String::new(),
                }),
                popup::Mode::DialEdit => app
                    .popup
                    .dial_idx
                    .and_then(|idx| popup::dial_metas().into_iter().nth(idx))
                    .map(|meta| Field {
                        label: meta.name.to_string(),
                        text: (meta.label)(&app.active_station.dials),
                    }),
                _ => None,
            };
            match app.popup.mode {
                popup::Mode::SaveAs => " Enter save  ·  Esc cancel ",
                popup::Mode::DialEdit => " Enter save  ·  Esc cancel  ·  all | 0 | 12 | 50% ",
                _ => " ↑↓ select  ·  ←→ / Enter toggle or adjust  ·  Esc / Ctrl-S close ",
            }
        }
    };
    Sheet {
        tabs,
        lines,
        picks,
        selected,
        hint,
        field,
    }
}

fn sheet_row(app: &App, row: &popup::Row, style: Style) -> Line<'static> {
    let accent = app.accent;
    let faint = |text: String| Span::styled(text, dim());
    let strong = |text: String, color: Color| {
        Span::styled(text, Style::default().fg(color).add_modifier(Modifier::BOLD))
    };
    match row {
        popup::Row::SectionHeader(label) => {
            let mut spans = vec![strong(format!("  {}", label), accent)];
            if *label == "saved stations" && !app.popup.search.is_empty() {
                spans.push(faint(format!("  › {}", app.popup.search)));
            }
            Line::from(spans)
        }
        popup::Row::Blank => Line::from(""),
        popup::Row::Model => pair(
            style,
            "model        ",
            Span::styled(app.active_station.model.clone(), style),
        ),
        popup::Row::Dial(idx) => match popup::dial_metas().get(*idx) {
            Some(meta) => pair(
                style,
                &format!("{:<13}", meta.name),
                Span::styled((meta.label)(&app.active_station.dials), style),
            ),
            None => Line::from(""),
        },
        popup::Row::ActiveStation => {
            let st = &app.active_station;
            Line::from(vec![
                Span::raw("  "),
                Span::raw(st.name.clone()),
                faint(format!(" ({})", st.model)),
                faint(format!(" • {}", popup::active_shop_label(app))),
            ])
        }
        popup::Row::SavedStation(idx) => {
            let st = &app.stations[*idx];
            Line::from(vec![
                Span::styled("  ", style),
                Span::styled(st.name.clone(), style),
                faint(format!(" ({})", st.model)),
                faint(format!(" • {}", popup::shops_label(app, &st.model))),
            ])
        }
        popup::Row::Unstationed(model) => Line::from(vec![
            Span::styled("  ", style),
            Span::styled(model.clone(), style),
            faint(format!(" • {}", popup::shops_label(app, model))),
        ]),
        popup::Row::UpdateAction => {
            let origin = app.active_origin.clone().unwrap_or_else(|| "?".into());
            Line::from(vec![
                Span::styled("  ", style),
                Span::styled(format!("update '{}'", origin), style),
            ])
        }
        popup::Row::SaveAsAction => Line::from(vec![
            Span::styled("  ", style),
            Span::styled("save active as new…", style),
        ]),
        popup::Row::PrefVoice => pair(
            style,
            "voice",
            Span::styled(
                format!("  {}", if app.voice_on { "on" } else { "off" }),
                style,
            ),
        ),
        popup::Row::PrefView => pair(
            style,
            "view mode",
            Span::styled(
                match app.view_mode {
                    ViewMode::Page => "  page",
                    ViewMode::Scroll => "  scroll",
                },
                style,
            ),
        ),
        popup::Row::PrefAccent => {
            let (name, color) = crate::config::ACCENTS[crate::config::accent_index(accent)];
            pair(style, "accent", strong(format!("  {name}"), color))
        }
        popup::Row::PrefNametags => pair(
            style,
            "nametags",
            strong(format!("  {}", app.nametags.name()), accent),
        ),
        popup::Row::PrefSync => pair(style, "sync", strong(format!("  {}", app.sync.place), accent)),
        popup::Row::PrefFont => pair(
            style,
            "font",
            strong(format!("  {}", app.typeface.name()), accent),
        ),
    }
}

fn tint(color: Option<Color>) -> Option<String> {
    match color {
        None | Some(Color::Reset) => None,
        Some(c) => Some(c.to_string()),
    }
}

fn ink<S: Serializer>(color: &Color, s: S) -> Result<S::Ok, S::Error> {
    tint(Some(*color)).serialize(s)
}

#[derive(Serialize)]
struct Run<'a> {
    t: &'a str,
    fg: Option<String>,
    bg: Option<String>,
    b: bool,
    i: bool,
    d: bool,
    u: bool,
    x: bool,
}

impl<'a> Run<'a> {
    fn of(span: &'a Span<'_>, base: Style) -> Self {
        let style = base.patch(span.style);
        let has = |m: Modifier| style.add_modifier.contains(m);
        Run {
            t: span.content.as_ref(),
            fg: tint(style.fg),
            bg: tint(style.bg),
            b: has(Modifier::BOLD),
            i: has(Modifier::ITALIC),
            d: has(Modifier::DIM),
            u: has(Modifier::UNDERLINED),
            x: has(Modifier::CROSSED_OUT),
        }
    }
}

#[derive(Serialize)]
struct Row<'a> {
    mono: bool,
    kind: crate::md::Kind,
    runs: Vec<Run<'a>>,
}

fn rows<S: Serializer>(lines: &[Line<'static>], s: S) -> Result<S::Ok, S::Error> {
    s.collect_seq(lines.iter().map(|line| Row {
        mono: crate::md::mono(line),
        kind: crate::md::kind(line),
        runs: line.spans.iter().map(|span| Run::of(span, line.style)).collect(),
    }))
}

fn runs<S: Serializer>(spans: &[Span<'static>], s: S) -> Result<S::Ok, S::Error> {
    s.collect_seq(spans.iter().map(|span| Run::of(span, Style::default())))
}

fn run<S: Serializer>(span: &Span<'static>, s: S) -> Result<S::Ok, S::Error> {
    Run::of(span, Style::default()).serialize(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    fn message(role: Role) -> Message {
        Message {
            role,
            content: "hi".into(),
            images: Vec::new(),
            guest: None,
            brain: String::new(),
            heart: String::new(),
            streaming: false,
            waiting: false,
            timestamp: "10:00".into(),
            phase: Phase::Streaming,
            current_tool: None,
            turn_id: 1,
            tool_events: Vec::new(),
        }
    }

    #[test]
    fn usage_formats_in_k_terms() {
        assert_eq!(format_k(0), "0");
        assert_eq!(format_k(950), "950");
        assert_eq!(format_k(1000), "1K");
        assert_eq!(format_k(12400), "12K");
        assert_eq!(format_k(12500), "13K");
        assert_eq!(format_k(999_600), "1M");
        assert_eq!(format_k(2_300_000), "2M");
        assert_eq!(format_k(2_600_000), "3M");
    }

    #[test]
    fn error_status_detection() {
        assert!(is_error_status("upstream 500: x"));
        assert!(is_error_status("stopped: length"));
        assert!(is_error_status("error: y"));
        assert!(!is_error_status("saved station 'a'"));
        assert!(!is_error_status(""));
    }

    #[test]
    fn mark_follows_what_the_assistant_is_doing() {
        let at = |phase, tool: Option<&str>| {
            mark(&Message {
                streaming: true,
                phase,
                current_tool: tool.map(String::from),
                ..message(Role::Assistant)
            })
        };
        assert_eq!(mark(&message(Role::Assistant)), ("⌃", false));
        assert_eq!(mark(&message(Role::User)), (USER_MARK, false));
        assert_eq!(UnicodeWidthStr::width(USER_MARK), 1);
        assert_eq!(at(Phase::Writing, None), ("⬩", true));
        assert_eq!(at(Phase::Thinking, None), ("✦", true));
        assert_eq!(at(Phase::Tinkering, Some("shell")), ("‣", true));
        assert_eq!(at(Phase::Tinkering, Some(crate::tools::BOOK_NAME)), ("಄", false));
    }

    #[test]
    fn thinking_follows_the_turn_not_the_message() {
        let holder = || Message {
            brain: "pondering".into(),
            ..message(Role::Assistant)
        };
        let live = Message {
            streaming: true,
            phase: Phase::Thinking,
            ..message(Role::Assistant)
        };
        assert_eq!(thinking_turn(&[holder(), live]), Some(holder().turn_id));
        assert_eq!(thinking_turn(&[holder()]), None);
    }

    #[test]
    fn bubble_wire_carries_styles_and_marks() {
        let b = bubble("3".into(), &message(Role::User), false);
        let v = serde_json::to_value(&b).unwrap();
        assert_eq!(v["id"], "3");
        assert_eq!(v["ink"], "Green");
        assert_eq!(v["mark"], USER_MARK);
        assert_eq!(v["reply"][0]["runs"][0]["t"], "hi");
        assert_eq!(v["reply"][0]["mono"], false);
    }
}
