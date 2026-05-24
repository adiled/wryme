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
use crate::popup;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let rows = popup::rows(app);

    // One Line per row, with focus highlight on the selected one.
    let mut lines: Vec<Line<'static>> = Vec::new();
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
        "  ↑↓ select  ·  ←→ adjust  ·  Enter act  ·  Esc / Ctrl-S close"
    };
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        hint,
        Style::default().fg(Color::DarkGray),
    )));

    // Centered modal: 60% width (min 50 cols), height = lines + borders + a
    // breathing row.
    let modal_w = (area.width as f32 * 0.60).max(50.0).min(area.width as f32) as u16;
    let modal_h = (lines.len() as u16 + 4).min(area.height);
    let modal_x = area.x + (area.width - modal_w) / 2;
    let modal_y = area.y + (area.height.saturating_sub(modal_h)) / 2;
    let modal_area = Rect {
        x: modal_x,
        y: modal_y,
        width: modal_w,
        height: modal_h,
    };

    // Clear underneath so the modal does not show through.
    f.render_widget(Clear, modal_area);

    // Capture line count before lines is moved into the Paragraph.
    let line_count = lines.len();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" station ");
    let widget = Paragraph::new(Text::from(lines))
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(widget, modal_area);

    // In SaveAs, place the terminal cursor inside the name field.
    if app.popup.mode == popup::Mode::SaveAs {
        // Name line is at index line_count - 3 (last 3: name line,
        // blank, hint line).
        let name_line_idx = line_count.saturating_sub(3);
        let name_y = modal_area.y + 1 + name_line_idx as u16;
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
