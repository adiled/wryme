use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::{Value, json};

use crate::app::{App, Message, Phase, Role, ViewMode};
use crate::popup::{self, Tab};
use crate::shop::Protocol;

pub const HEART_MARK: &str = "\u{2764}\u{FE0E}";
pub const DOT: &str = " • ";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
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
    pub fn of(msg: &Message) -> Self {
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

    pub fn label(self) -> Option<&'static str> {
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

pub struct Bubble {
    pub name: String,
    pub activity: Activity,
    pub tool: Option<String>,
    pub images: Vec<String>,
    pub reply: Vec<Line<'static>>,
    pub replied: bool,
    pub orphan: bool,
    pub brain: Vec<Line<'static>>,
    pub heart: Vec<Line<'static>>,
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

pub fn bubble(msg: &Message) -> Bubble {
    let replied = !msg.content.is_empty();
    let brained = !msg.brain.is_empty();
    let hearted = !msg.heart.is_empty();
    let live = msg.streaming;
    let (reply, images) = match (replied, msg.role) {
        (false, _) => (Vec::new(), Vec::new()),
        (true, Role::Assistant) => (crate::md::render(&msg.content, live), Vec::new()),
        (true, Role::User) => (plain(&msg.content, Style::default(), live), msg.images.clone()),
    };
    Bubble {
        name: match (&msg.role, &msg.guest) {
            (Role::User, Some(from)) => format!("guest@{from}"),
            (Role::User, None) => "you".to_string(),
            (Role::Assistant, _) => "assistant".to_string(),
        },
        activity: Activity::of(msg),
        tool: msg
            .current_tool
            .clone()
            .filter(|name| live && !crate::tools::is_hidden_tool(name)),
        images,
        reply,
        replied,
        orphan: live && !replied && !brained && !hearted,
        brain: thought(&msg.brain, live && !replied),
        heart: thought(&msg.heart, live && !replied && !brained),
    }
}

pub fn awaiting(app: &App) -> Option<Message> {
    (!app.awaiting.is_empty()).then(|| Message {
        role: Role::User,
        content: app.awaiting.clone(),
        images: Vec::new(),
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

pub struct Bar {
    pub state: &'static str,
    pub esc: &'static str,
    pub busy: bool,
}

pub fn bar(app: &App) -> Bar {
    let (state, esc) = match (app.in_flight, app.voice_is_active()) {
        (true, true) => ("streaming + speaking…", "esc quiet"),
        (true, false) => ("streaming…", "esc interrupt"),
        (false, true) => ("speaking…", "esc quiet"),
        (false, false) => ("", ""),
    };
    Bar {
        state,
        esc,
        busy: app.in_flight,
    }
}

pub struct Status {
    pub left: Vec<Span<'static>>,
    pub note: Span<'static>,
    pub right: Vec<Span<'static>>,
}

pub fn status(app: &App) -> Status {
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

pub struct Sheet {
    pub tabs: Vec<(&'static str, bool)>,
    pub lines: Vec<Line<'static>>,
    pub hint: &'static str,
    pub prompt: Option<String>,
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

pub fn sheet(app: &App) -> Sheet {
    let accent = app.accent;
    let tabs = [
        (Tab::Station, "Station"),
        (Tab::Shops, "Shops"),
        (Tab::Pref, "Pref"),
        (Tab::Help, "Help"),
    ]
    .iter()
    .map(|(tab, label)| (*label, app.popup.tab == *tab))
    .collect();
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut prompt = None;
    let hint = match (app.popup.tab, &app.popup.mode) {
        (Tab::Shops, _) => {
            lines = popup::shops_lines(app);
            " read-only · edit ~/.config/wryme/shops.toml · Esc / Ctrl-S close "
        }
        (Tab::Help, _) => {
            for (key, what) in popup::help_rows() {
                if key.is_empty() {
                    lines.push(Line::from(""));
                } else {
                    lines.push(Line::from(vec![
                        Span::styled(format!("  {:<22}", key), Style::default().fg(accent)),
                        Span::styled(what, Style::default()),
                    ]));
                }
            }
            " Esc close  ·  Ctrl-S close "
        }
        (_, mode) => {
            for (i, row) in popup::rows(app).iter().enumerate() {
                let style = focus(
                    i == app.popup.selected && *mode == popup::Mode::Browse,
                    accent,
                );
                lines.push(sheet_row(app, row, style));
            }
            let field = match mode {
                popup::Mode::SaveAs => Some(("name".to_string(), &app.popup.name_input)),
                popup::Mode::DialEdit => Some((
                    app.popup
                        .dial_idx
                        .and_then(|idx| popup::dial_metas().get(idx).map(|m| m.name.to_string()))
                        .unwrap_or_else(|| "value".to_string()),
                    &app.popup.dial_input,
                )),
                _ => None,
            };
            if let Some((name, input)) = field {
                let label = format!("  {name}: ");
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled(label.clone(), Style::default().fg(accent)),
                    Span::raw(input.text.clone()),
                ]));
                prompt = Some(label);
            }
            match mode {
                popup::Mode::SaveAs => " Enter save  ·  Esc cancel ",
                popup::Mode::DialEdit => " Enter save  ·  Esc cancel  ·  all | 0 | 12 | 50% ",
                _ => " ↑↓ select  ·  ←→ / Enter toggle or adjust  ·  Esc / Ctrl-S close ",
            }
        }
    };
    Sheet {
        tabs,
        lines,
        hint,
        prompt,
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
    }
}

fn tint(color: Option<Color>) -> Value {
    match color {
        None | Some(Color::Reset) => Value::Null,
        Some(c) => Value::String(c.to_string()),
    }
}

fn run(span: &Span<'_>, base: Style) -> Value {
    let style = base.patch(span.style);
    json!({
        "t": span.content,
        "fg": tint(style.fg),
        "bg": tint(style.bg),
        "b": style.add_modifier.contains(Modifier::BOLD),
        "i": style.add_modifier.contains(Modifier::ITALIC),
        "d": style.add_modifier.contains(Modifier::DIM),
    })
}

fn runs(spans: &[Span<'_>]) -> Vec<Value> {
    spans.iter().map(|s| run(s, Style::default())).collect()
}

fn rows(lines: &[Line<'_>]) -> Vec<Value> {
    lines
        .iter()
        .map(|line| {
            json!({
                "mono": crate::md::fenced(line),
                "runs": line.spans.iter().map(|s| run(s, line.style)).collect::<Vec<_>>(),
            })
        })
        .collect()
}

pub fn wire(app: &App) -> String {
    let bar = bar(app);
    let status = status(app);
    let waiting = awaiting(app);
    let bubbles: Vec<Value> = waiting
        .iter()
        .map(|m| ("awaiting".to_string(), m))
        .chain(
            app.messages
                .iter()
                .enumerate()
                .rev()
                .map(|(i, m)| (i.to_string(), m)),
        )
        .map(|(id, msg)| {
            let b = bubble(msg);
            json!({
                "id": id,
                "user": msg.role == Role::User,
                "name": b.name,
                "activity": b.activity.label(),
                "tool": b.tool,
                "waiting": msg.waiting,
                "live": msg.streaming,
                "time": msg.timestamp,
                "images": b.images,
                "reply": rows(&b.reply),
                "brain": rows(&b.brain),
                "heart": rows(&b.heart),
            })
        })
        .collect();
    let sheet = (app.popup.mode != popup::Mode::Closed).then(|| {
        let s = sheet(app);
        json!({
            "tabs": s.tabs.iter().map(|(label, on)| json!({"label": label, "on": on})).collect::<Vec<_>>(),
            "lines": rows(&s.lines),
            "hint": s.hint.trim(),
        })
    });
    json!({
        "accent": tint(Some(app.accent)),
        "bar": {"state": bar.state, "esc": bar.esc, "busy": bar.busy},
        "bubbles": bubbles,
        "status": {
            "left": runs(&status.left),
            "note": run(&status.note, Style::default()),
            "right": runs(&status.right),
        },
        "error": is_error_status(&app.status).then(|| app.status.clone()),
        "sheet": sheet,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
