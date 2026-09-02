// Popup rendering. Reads popup state from App and draws the modal
// overlay on top of the main UI. State and actions live in popup.rs.
// The split: popup.rs is "what the popup IS"; popup_ui.rs is "what the
// popup LOOKS like."

use ratatui::{
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::app::App;
use crate::popup::{self, Tab};

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();

    // Build the body lines for the active tab. This is the *full* body
    // (may be taller than the terminal); the scroll offset picks which
    // window of it is visible inside the modal.
    let mut lines: Vec<Line<'static>> = Vec::new();
    if app.popup.tab == Tab::Help {
        // Help tab: static shortcut table, no selection.
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

        // One Line per row, with focus highlight on the selected one.
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
                        Span::styled("model       ", style),
                        Span::styled(app.active_station.model.clone(), style),
                    ]));
                }
                popup::Row::Boldness => {
                    let style = focus_style(selected);
                    lines.push(Line::from(vec![
                        Span::styled(marker, style),
                        Span::styled("boldness    ", style),
                        Span::styled(popup::boldness_label(app.active_station.dials.boldness), style),
                    ]));
                }
                popup::Row::Patience => {
                    let style = focus_style(selected);
                    lines.push(Line::from(vec![
                        Span::styled(marker, style),
                        Span::styled("patience    ", style),
                        Span::styled(popup::patience_label(app.active_station.dials.patience), style),
                    ]));
                }
                popup::Row::Verbosity => {
                    let style = focus_style(selected);
                    lines.push(Line::from(vec![
                        Span::styled(marker, style),
                        Span::styled("verbosity   ", style),
                        Span::styled(popup::verbosity_label(app.active_station.dials.verbosity), style),
                    ]));
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
            }
        }

        // Inline name prompt while in SaveAs.
        if app.popup.mode == popup::Mode::SaveAs {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("  name: ", Style::default().fg(Color::Cyan)),
                Span::raw(app.popup.name_input.text.clone()),
            ]));
        }

        // Hint line at the bottom.
        let hint = if app.popup.mode == popup::Mode::SaveAs {
            "  Enter save  ·  Esc cancel"
        } else {
            "  ↑↓ select  ·  ←→ adjust  ·  Enter act  ·  Tab: Help  ·  F1 Help  ·  PgUp/PgDn scroll  ·  Esc / Ctrl-S close"
        };
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            hint,
            Style::default().fg(Color::DarkGray),
        )));
    }

    // Centered modal. Width 60% (min 50 cols); height grows with content
    // but never exceeds the terminal, and is at least enough to show a
    // useful slice (so the user can scroll when content overflows).
    let modal_w = (area.width as f32 * 0.60).max(50.0).min(area.width as f32) as u16;
    let modal_h = (lines.len() as u16 + 4)
        .clamp(10, area.height.min(area.height));
    let modal_x = area.x + (area.width - modal_w) / 2;
    let modal_y = area.y + (area.height.saturating_sub(modal_h)) / 2;
    let modal_area = Rect {
        x: modal_x,
        y: modal_y,
        width: modal_w,
        height: modal_h,
    };

    // Clamp the scroll offset so the visible window always fits. The
    // window is the body area inside the borders (2 rows).
    let body_h = (modal_h.saturating_sub(2)) as usize;
    let max_scroll = lines.len().saturating_sub(body_h);
    if app.popup.scroll > max_scroll {
        app.popup.scroll = max_scroll;
    }
    let scroll = app.popup.scroll;

    // Slice the visible window of lines.
    let visible: Vec<Line<'static>> = lines
        .iter()
        .skip(scroll)
        .take(body_h)
        .cloned()
        .collect();

    // Clear underneath so the modal does not show through.
    f.render_widget(Clear, modal_area);

    // BIOS-style horizontal tab bar: Station and Help, the active one
    // marked with ▶.
    let station_tab = if app.popup.tab == Tab::Station {
        "▶ Station"
    } else {
        " Station"
    };
    let help_tab = if app.popup.tab == Tab::Help {
        "▶ Help"
    } else {
        " Help"
    };
    let title = format!("{}  {}", station_tab, help_tab);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(title);
    let widget = Paragraph::new(Text::from(visible))
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(widget, modal_area);

    // Scroll indicator on the right edge of the body, only when content
    // overflows the modal (scroll > 0 or content taller than the window).
    if lines.len() > body_h {
        let ratio = scroll as f32 / max_scroll.max(1) as f32;
        let bar_h = (body_h as f32 * 0.3).max(1.0) as u16;
        let bar_y = modal_area.y + 1 + (ratio * (body_h.saturating_sub(bar_h as usize) as f32)) as u16;
        let bar_rect = Rect {
            x: modal_area.x + modal_area.width.saturating_sub(2),
            y: bar_y,
            width: 1,
            height: bar_h.min(body_h as u16),
        };
        f.render_widget(
            Block::default().style(Style::default().bg(Color::Cyan)),
            bar_rect,
        );
    }

    // In SaveAs, place the terminal cursor inside the name field.
    if app.popup.mode == popup::Mode::SaveAs {
        // Name line is at index line_count - 3 in the full body; its
        // rendered row is that index minus the scroll offset.
        let line_count = lines.len();
        let name_line_idx = line_count.saturating_sub(3);
        let name_y = modal_area.y + 1 + (name_line_idx.saturating_sub(scroll)) as u16;
        let prompt_len = "  name: ".len() as u16;
        let caret = app.popup.name_input.display_col();
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
