use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use crate::app::{App, ViewMode};
use crate::popup::{self, Tab};

const MODAL_WIDTH_RATIO: f32 = 0.60;
const MODAL_MIN_WIDTH: u16 = 50;
const BORDER_ROWS: u16 = 2;
const SCROLLBAR_HEIGHT_RATIO: f32 = 0.3;
const SCROLLBAR_MIN_HEIGHT: f32 = 1.0;
const SCROLLBAR_INSET: u16 = 2;
const LINES_AFTER_INPUT: usize = 3;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();

    let mut lines: Vec<Line<'static>> = Vec::new();
    if app.popup.tab == Tab::Shops {
        lines = popup::shops_lines(app);
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  read-only · edit ~/.config/wryme/shops.toml · Tab switch · Esc / Ctrl-S close",
            Style::default().fg(Color::DarkGray),
        )));
    } else if app.popup.tab == Tab::Help {
        for (key, what) in popup::help_rows() {
            if key.is_empty() {
                lines.push(Line::from(""));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {:<22}", key), Style::default().fg(Color::Cyan)),
                    Span::styled(what, Style::default()),
                ]));
            }
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Esc / Tab back to Station  ·  Ctrl-S close",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        let rows = popup::rows(app);

        for (i, row) in rows.iter().enumerate() {
            let selected = i == app.popup.selected && app.popup.mode == popup::Mode::Browse;
            let marker = if selected { "› " } else { "  " };
            match row {
                popup::Row::SectionHeader(label) => {
                    lines.push(Line::from(Span::styled(
                        format!("  {}", label),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    )));
                }
                popup::Row::Blank => {
                    lines.push(Line::from(""));
                }
                popup::Row::Model => {
                    let style = focus_style(selected);
                    lines.push(Line::from(vec![
                        Span::styled(marker, style),
                        Span::styled("model        ", style),
                        Span::styled(app.active_station.model.clone(), style),
                    ]));
                }
                popup::Row::Dial(idx) => {
                    let style = focus_style(selected);
                    if let Some(meta) = popup::dial_metas().get(*idx) {
                        let label = (meta.label)(&app.active_station.dials);
                        let name = format!("{:<13}", meta.name);
                        lines.push(Line::from(vec![
                            Span::styled(marker, style),
                            Span::styled(name, style),
                            Span::styled(label, style),
                        ]));
                    }
                }
                popup::Row::SavedStation(idx) => {
                    let st = &app.stations[*idx];
                    let style = focus_style(selected);
                    lines.push(Line::from(vec![
                        Span::styled(marker, style),
                        Span::styled(st.name.clone(), style),
                        Span::styled(
                            format!("  ({})", st.model),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]));
                }
                popup::Row::UpdateAction => {
                    let style = focus_style(selected);
                    let origin = app.active_origin.clone().unwrap_or_else(|| "?".into());
                    lines.push(Line::from(vec![
                        Span::styled(marker, style),
                        Span::styled(format!("update '{}'", origin), style),
                    ]));
                }
                popup::Row::SaveAsAction => {
                    let style = focus_style(selected);
                    lines.push(Line::from(vec![
                        Span::styled(marker, style),
                        Span::styled("save active as new…", style),
                    ]));
                }
                popup::Row::PrefVoice => {
                    let style = focus_style(selected);
                    lines.push(Line::from(vec![
                        Span::styled(marker, style),
                        Span::styled("voice", style),
                        Span::styled(
                            format!("  {}", if app.voice_on { "on" } else { "off" }),
                            style,
                        ),
                    ]));
                }
                popup::Row::PrefView => {
                    let style = focus_style(selected);
                    let val = match app.view_mode {
                        ViewMode::Page => "page",
                        ViewMode::Scroll => "scroll",
                    };
                    lines.push(Line::from(vec![
                        Span::styled(marker, style),
                        Span::styled("view mode", style),
                        Span::styled(format!("  {val}"), style),
                    ]));
                }
            }
        }

        if app.popup.mode == popup::Mode::SaveAs {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("  name: ", Style::default().fg(Color::Cyan)),
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
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::raw(app.popup.dial_input.text.clone()),
                ]));
            }
        }

        let hint = if app.popup.mode == popup::Mode::SaveAs {
            "  Enter save  ·  Esc cancel"
        } else if app.popup.mode == popup::Mode::DialEdit {
            "  Enter save  ·  Esc cancel  ·  all | 0 | 12 | 50%"
        } else {
            "  ↑↓ select  ·  ←→ / Enter toggle or adjust  ·  Tab: Shops / Pref / Help  ·  F1 Help  ·  PgUp/PgDn scroll  ·  Esc / Ctrl-S close"
        };
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            hint,
            Style::default().fg(Color::DarkGray),
        )));
    }

    let modal_w = (area.width as f32 * MODAL_WIDTH_RATIO)
        .max(MODAL_MIN_WIDTH as f32)
        .min(area.width as f32) as u16;
    let modal_h = (lines.len() as u16 + 4).clamp(10, area.height.min(area.height));
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
    let scroll = app.popup.scroll;

    let visible: Vec<Line<'static>> = lines.iter().skip(scroll).take(body_h).cloned().collect();

    f.render_widget(Clear, modal_area);

    let station_tab = if app.popup.tab == Tab::Station {
        "▶ Station"
    } else {
        " Station"
    };
    let shops_tab = if app.popup.tab == Tab::Shops {
        "▶ Shops"
    } else {
        " Shops"
    };
    let pref_tab = if app.popup.tab == Tab::Pref {
        "▶ Pref"
    } else {
        " Pref"
    };
    let help_tab = if app.popup.tab == Tab::Help {
        "▶ Help"
    } else {
        " Help"
    };
    let title = format!(
        "{} {} {} {}",
        station_tab, shops_tab, pref_tab, help_tab
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(title);
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
            Block::default().style(Style::default().bg(Color::Cyan)),
            bar_rect,
        );
    }

    if app.popup.mode == popup::Mode::SaveAs {
        let line_count = lines.len();
        let name_line_idx = line_count.saturating_sub(LINES_AFTER_INPUT);
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
        let name_line_idx = line_count.saturating_sub(LINES_AFTER_INPUT);
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

fn focus_style(selected: bool) -> Style {
    if selected {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}
