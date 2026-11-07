use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};

use crate::app::{App, ViewMode};
use crate::popup::{self, Tab};

const MODAL_WIDTH_RATIO: f32 = 0.60;
const MODAL_MIN_WIDTH: u16 = 50;
const BORDER_ROWS: u16 = 2;
const SCROLLBAR_HEIGHT_RATIO: f32 = 0.3;
const SCROLLBAR_MIN_HEIGHT: f32 = 1.0;
const SCROLLBAR_INSET: u16 = 2;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();

    let mut lines: Vec<Line<'static>> = Vec::new();
    let hint: String;
    if app.popup.tab == Tab::Shops {
        lines = popup::shops_lines(app);
        hint = " read-only · edit ~/.config/wryme/shops.toml · Esc / Ctrl-S close ".to_string();
    } else if app.popup.tab == Tab::Help {
        for (key, what) in popup::help_rows() {
            if key.is_empty() {
                lines.push(Line::from(""));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {:<22}", key), Style::default().fg(app.accent)),
                    Span::styled(what, Style::default()),
                ]));
            }
        }
        hint = " Esc close  ·  Ctrl-S close ".to_string();
    } else {
        let rows = popup::rows(app);

        for (i, row) in rows.iter().enumerate() {
            let selected = i == app.popup.selected && app.popup.mode == popup::Mode::Browse;
            match row {
                popup::Row::SectionHeader(label) => {
                    let mut spans = vec![Span::styled(
                        format!("  {}", label),
                        Style::default()
                            .fg(app.accent)
                            .add_modifier(Modifier::BOLD),
                    )];
                    if *label == "saved stations" && !app.popup.search.is_empty() {
                        spans.push(Span::styled(
                            format!("  › {}", app.popup.search),
                            Style::default().fg(Color::DarkGray),
                        ));
                    }
                    lines.push(Line::from(spans));
                }
                popup::Row::Blank => {
                    lines.push(Line::from(""));
                }
                popup::Row::Model => {
                    let style = focus_style(selected, app.accent);
                    lines.push(Line::from(vec![
                        Span::styled("  ", style),
                        Span::styled("model        ", style),
                        Span::styled(app.active_station.model.clone(), style),
                    ]));
                }
                popup::Row::Dial(idx) => {
                    let style = focus_style(selected, app.accent);
                    if let Some(meta) = popup::dial_metas().get(*idx) {
                        let label = (meta.label)(&app.active_station.dials);
                        let name = format!("{:<13}", meta.name);
                        lines.push(Line::from(vec![
                            Span::styled("  ", style),
                            Span::styled(name, style),
                            Span::styled(label, style),
                        ]));
                    }
                }
                popup::Row::ActiveStation => {
                    let st = &app.active_station;
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::raw(st.name.clone()),
                        dim(format!(" ({})", st.model)),
                        dim(format!(" • {}", popup::active_shop_label(app))),
                    ]));
                }
                popup::Row::SavedStation(idx) => {
                    let st = &app.stations[*idx];
                    let style = focus_style(selected, app.accent);
                    lines.push(Line::from(vec![
                        Span::styled("  ", style),
                        Span::styled(st.name.clone(), style),
                        dim(format!(" ({})", st.model)),
                        dim(format!(" • {}", popup::shops_label(app, &st.model))),
                    ]));
                }
                popup::Row::Unstationed(model) => {
                    let style = focus_style(selected, app.accent);
                    lines.push(Line::from(vec![
                        Span::styled("  ", style),
                        Span::styled(model.clone(), style),
                        dim(format!(" • {}", popup::shops_label(app, model))),
                    ]));
                }
                popup::Row::UpdateAction => {
                    let style = focus_style(selected, app.accent);
                    let origin = app.active_origin.clone().unwrap_or_else(|| "?".into());
                    lines.push(Line::from(vec![
                        Span::styled("  ", style),
                        Span::styled(format!("update '{}'", origin), style),
                    ]));
                }
                popup::Row::SaveAsAction => {
                    let style = focus_style(selected, app.accent);
                    lines.push(Line::from(vec![
                        Span::styled("  ", style),
                        Span::styled("save active as new…", style),
                    ]));
                }
                popup::Row::PrefVoice => {
                    let style = focus_style(selected, app.accent);
                    lines.push(Line::from(vec![
                        Span::styled("  ", style),
                        Span::styled("voice", style),
                        Span::styled(
                            format!("  {}", if app.voice_on { "on" } else { "off" }),
                            style,
                        ),
                    ]));
                }
                popup::Row::PrefView => {
                    let style = focus_style(selected, app.accent);
                    let val = match app.view_mode {
                        ViewMode::Page => "page",
                        ViewMode::Scroll => "scroll",
                    };
                    lines.push(Line::from(vec![
                        Span::styled("  ", style),
                        Span::styled("view mode", style),
                        Span::styled(format!("  {val}"), style),
                    ]));
                }
                popup::Row::PrefAccent => {
                    let style = focus_style(selected, app.accent);
                    let (name, color) =
                        crate::config::ACCENTS[crate::config::accent_index(app.accent)];
                    lines.push(Line::from(vec![
                        Span::styled("  ", style),
                        Span::styled("accent", style),
                        Span::styled(
                            format!("  {name}"),
                            Style::default().fg(color).add_modifier(Modifier::BOLD),
                        ),
                    ]));
                }
                popup::Row::PrefNametags => {
                    let style = focus_style(selected, app.accent);
                    let name = app.nametags.name();
                    lines.push(Line::from(vec![
                        Span::styled("  ", style),
                        Span::styled("nametags", style),
                        Span::styled(
                            format!("  {name}"),
                            Style::default().fg(app.accent).add_modifier(Modifier::BOLD),
                        ),
                    ]));
                }
            }
        }

        if app.popup.mode == popup::Mode::SaveAs {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("  name: ", Style::default().fg(app.accent)),
                Span::raw(app.popup.name_input.text.clone()),
            ]));
        }
        if app.popup.mode == popup::Mode::DialEdit {
            lines.push(Line::from(""));
            if let Some(idx) = app.popup.dial_idx
                && let Some(meta) = popup::dial_metas().get(idx)
            {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {}: ", meta.name),
                        Style::default().fg(app.accent),
                    ),
                    Span::raw(app.popup.dial_input.text.clone()),
                ]));
            }
        }

        hint = if app.popup.mode == popup::Mode::SaveAs {
            " Enter save  ·  Esc cancel ".to_string()
        } else if app.popup.mode == popup::Mode::DialEdit {
            " Enter save  ·  Esc cancel  ·  all | 0 | 12 | 50% ".to_string()
        } else {
            " ↑↓ select  ·  ←→ / Enter toggle or adjust  ·  Esc / Ctrl-S close ".to_string()
        };
    }

    let modal_w = (area.width as f32 * MODAL_WIDTH_RATIO)
        .max(MODAL_MIN_WIDTH as f32)
        .min(area.width as f32) as u16;
    let modal_h = ((area.height as f32 * 0.70) as u16)
        .max(10)
        .min(area.height.saturating_sub(4));
    let modal_x = area.x + (area.width - modal_w) / 2;
    let modal_y = area.y + (area.height.saturating_sub(modal_h)) / 2;
    let modal_area = Rect {
        x: modal_x,
        y: modal_y,
        width: modal_w,
        height: modal_h,
    };

    let body_h = modal_h.saturating_sub(BORDER_ROWS) as usize;
    let max_scroll = lines.len().saturating_sub(body_h);
    if app.popup.scroll > max_scroll {
        app.popup.scroll = max_scroll;
    }
    if app.popup.mode == popup::Mode::Browse
        && matches!(app.popup.tab, Tab::Station | Tab::Pref)
    {
        let sel = app.popup.selected;
        let last = app.popup.scroll + body_h;
        app.popup.scroll = if sel < app.popup.scroll {
            sel
        } else if sel >= last {
            sel.saturating_sub(body_h).saturating_add(1)
        } else {
            app.popup.scroll
        };
        app.popup.scroll = app.popup.scroll.min(lines.len().saturating_sub(body_h));
    }
    let scroll = app.popup.scroll;

    let visible: Vec<Line<'static>> = lines.iter().skip(scroll).take(body_h).cloned().collect();

    f.render_widget(Clear, modal_area);

    let tab_spans: Vec<Span<'static>> = [
            (Tab::Station, " Station "),
            (Tab::Shops, " Shops "),
            (Tab::Pref, " Pref "),
            (Tab::Help, " Help "),
        ]
        .iter()
        .map(|(t, label)| {
            let style = if app.popup.tab == *t {
                Style::default()
                    .fg(Color::Black)
                    .bg(app.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Span::styled(*label, style)
        })
        .collect();
    let title = Line::from(tab_spans);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(app.accent))
        .title(title.right_aligned())
        .title_bottom(Line::from(Span::styled(
            hint,
            Style::default().fg(Color::DarkGray),
        ))
        .right_aligned());
    let widget = Paragraph::new(Text::from(visible))
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(widget, modal_area);

    if lines.len() > body_h {
        let ratio = scroll as f32 / max_scroll.max(1) as f32;
        let bar_h = (body_h as f32 * SCROLLBAR_HEIGHT_RATIO).max(SCROLLBAR_MIN_HEIGHT) as u16;
        let bar_y =
            modal_area.y + 1 + (ratio * (body_h.saturating_sub(bar_h as usize) as f32)) as u16;
        let bar_rect = Rect {
            x: modal_area.x + modal_area.width.saturating_sub(SCROLLBAR_INSET),
            y: bar_y,
            width: 1,
            height: bar_h.min(body_h as u16),
        };
        f.render_widget(
            Block::default().style(Style::default().bg(app.accent)),
            bar_rect,
        );
    }

    if app.popup.mode == popup::Mode::SaveAs {
        let name_line_idx = lines.len().saturating_sub(1);
        let name_y = modal_area.y + 1 + (name_line_idx.saturating_sub(scroll)) as u16;
        let prompt_len = "  name: ".len() as u16;
        let caret = app.popup.name_input.display_col();
        f.set_cursor_position(Position {
            x: modal_area.x + prompt_len + caret,
            y: name_y,
        });
    }
    if app.popup.mode == popup::Mode::DialEdit {
        let line_count = lines.len();
        let name_line_idx = line_count.saturating_sub(1);
        let name_y = modal_area.y + 1 + (name_line_idx.saturating_sub(scroll)) as u16;
        let prompt = if let Some(idx) = app.popup.dial_idx {
            if let Some(meta) = popup::dial_metas().get(idx) {
                format!("  {}: ", meta.name)
            } else {
                "  value: ".to_string()
            }
        } else {
            "  value: ".to_string()
        };
        let prompt_len = prompt.len() as u16;
        let caret = app.popup.dial_input.display_col();
        f.set_cursor_position(Position {
            x: modal_area.x + prompt_len + caret,
            y: name_y,
        });
    }
}

fn dim(text: String) -> Span<'static> {
    Span::styled(text, Style::default().fg(Color::DarkGray))
}

fn focus_style(selected: bool, accent: Color) -> Style {
    if selected {
        Style::default()
            .fg(Color::Black)
            .bg(accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shop::{Protocol, Shop, WindowMode};
    use crate::station::Station;

    fn shop(name: &str, models: &[&str]) -> Shop {
        Shop {
            name: name.into(),
            url: format!("http://{name}"),
            key: String::new(),
            protocol: Protocol::ChatCompletions,
            window: WindowMode::Full,
            models: models.iter().map(|m| m.to_string()).collect(),
            limit: Vec::new(),
            headers: Default::default(),
        }
    }

    fn station(name: &str, model: &str) -> Station {
        Station {
            name: name.into(),
            model: model.into(),
            ..Default::default()
        }
    }

    fn screen(app: &mut App) -> Vec<String> {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 40)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn station_tab_shows_active_saved_and_unstationed_with_shops() {
        let hum = shop("hum", &["deepseek-v4-flash", "claude-sonnet-5-5"]);
        let opus = station("opus", "claude-opus-5-5");
        let mut app = App::new(
            None,
            vec![Shop::demo(), shop("dwarfstar", &["deepseek-v4-flash", "glm-5.3-flash"]), hum.clone()],
            vec![Station::demo(), opus.clone(), station("sonnet", "claude-sonnet-5-5")],
            opus,
            hum,
            Some("opus".into()),
        );
        app.memory.clear();
        app.popup.mode = popup::Mode::Browse;
        let lines = screen(&mut app);
        let has = |want: &str| lines.iter().any(|l| l.contains(want));
        for want in [
            "active station",
            "opus (claude-opus-5-5) • ?",
            "saved stations",
            "sonnet (claude-sonnet-5-5) • hum",
            "unstationed models",
            "deepseek-v4-flash • dwarfstar, hum",
            "glm-5.3-flash • dwarfstar",
        ] {
            assert!(has(want), "missing {want:?} in:\n{}", lines.join("\n"));
        }
        assert!(!has("claude-sonnet-5-5 • "), "a stationed model is listed as unstationed");
        app.memory.insert("claude-opus-5-5".into(), "hum".into());
        assert!(screen(&mut app).iter().any(|l| l.contains("opus (claude-opus-5-5) • hum")));
    }
}
