// Rendering. Three regions:
//
//   ┌──────────────────────────────────────┐
//   │ > input here                         │   top: input bar
//   ├──────────────────────────────────────┤
//   │ assistant • streaming                │   middle: messages,
//   │ newest message text                  │           newest at top,
//   │                                      │           older below it
//   │ you                                  │
//   │ older question                       │
//   ├──────────────────────────────────────┤
//   │ model • N msgs • status              │   bottom: status
//   └──────────────────────────────────────┘

use ratatui::{
    layout::{Constraint, Direction, Layout, Position},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Message, Phase, Role, ViewMode};
use crate::input::Input;
use crate::popup;
use crate::shop::Protocol;

pub fn draw(f: &mut Frame, app: &mut App, input: &Input) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // input box
            Constraint::Min(1),    // messages
            Constraint::Length(1), // status
        ])
        .split(area);

    // ---- input bar (top) ----
    let prompt = "› ";
    let input_line = Line::from(vec![
        Span::styled(prompt, Style::default().fg(Color::Cyan)),
        Span::raw(&input.text),
    ]);
    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_style(if app.in_flight {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(Color::Cyan)
        })
        .title(if app.in_flight {
            " streaming… (Esc cancel) "
        } else {
            " write. Enter to send, Ctrl-C to quit "
        });
    let input_widget = Paragraph::new(input_line).block(input_block);
    f.render_widget(input_widget, chunks[0]);

    // Place the terminal cursor inside the input box.
    let cursor_x = chunks[0].x + 1 + prompt.len() as u16 + input.display_col();
    let cursor_y = chunks[0].y + 1;
    if cursor_x < chunks[0].x + chunks[0].width.saturating_sub(1) {
        f.set_cursor_position(Position {
            x: cursor_x,
            y: cursor_y,
        });
    }

    // ---- messages (middle, newest first, paged) ----
    let mut lines: Vec<Line> = Vec::new();
    let msg_width = chunks[1].width;
    for msg in app.messages.iter().rev() {
        push_message(&mut lines, msg, msg_width);
        lines.push(Line::from(""));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "no messages yet. type above and hit Enter",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )));
    }
    let messages_para = Paragraph::new(Text::from(lines.clone()))
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::NONE));

    let viewport_h = chunks[1].height as usize;
    app.last_viewport_h = viewport_h;
    let total_rows = wrapped_row_count(&lines, chunks[1].width);
    let n_pages = if total_rows == 0 || viewport_h == 0 {
        1
    } else {
        total_rows.div_ceil(viewport_h)
    };
    let page = app.current_page.min(n_pages.saturating_sub(1));

    // Clamp the scroll offset to the last legal row so the user can't page
    // off into the empty void beyond the oldest line.
    let max_scroll = total_rows.saturating_sub(1);
    let scroll_offset = match app.view_mode {
        ViewMode::Page => page * viewport_h,
        ViewMode::Scroll => app.scroll_row.min(max_scroll),
    };
    let scroll_y = scroll_offset.min(u16::MAX as usize) as u16;

    f.render_widget(messages_para.scroll((scroll_y, 0)), chunks[1]);

    // ---- status bar ----
    let dot = " • ";
    let is_demo = app.active_shop.protocol == Protocol::Demo;
    let station_color = if is_demo { Color::Yellow } else { Color::Cyan };
    let mut pieces = vec![
        Span::styled("wryme", Style::default().fg(Color::Cyan)),
        Span::raw(dot),
        Span::styled(
            format!("station: {}", app.active_station.name),
            Style::default().fg(station_color),
        ),
        Span::raw(dot),
        Span::raw(app.active_station.model.clone()),
        Span::raw(dot),
        Span::styled(
            format!("via {}", app.active_shop.name),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(dot),
        Span::raw(format!("{} msg", app.messages.len())),
    ];
    if !app.messages.is_empty() {
        pieces.push(Span::raw(dot));
        match app.view_mode {
            ViewMode::Page => {
                pieces.push(Span::styled(
                    format!("page {}/{}", page + 1, n_pages),
                    Style::default().fg(if n_pages > 1 {
                        Color::Cyan
                    } else {
                        Color::DarkGray
                    }),
                ));
            }
            ViewMode::Scroll => {
                pieces.push(Span::styled(
                    if scroll_offset == 0 {
                        "scroll (top)".to_string()
                    } else {
                        format!("scroll +{}", scroll_offset)
                    },
                    Style::default().fg(Color::Cyan),
                ));
            }
        }
    }
    pieces.push(Span::raw(dot));
    pieces.push(Span::styled(
        app.status.clone(),
        Style::default().fg(
            if app.status.starts_with("error") || app.status.starts_with("upstream") {
                Color::Red
            } else {
                Color::Gray
            },
        ),
    ));
    let status = Paragraph::new(Line::from(pieces)).style(Style::default().fg(Color::Gray));
    f.render_widget(status, chunks[2]);

    // ---- station popup overlay ----
    if app.popup.mode != popup::Mode::Closed {
        draw_popup(f, app);
    }
}

fn draw_popup(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let rows = popup::rows(app);

    // Build a Line per row, including the focused-row highlight and the
    // current value for adjustable rows.
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
                    Span::styled(format!("  ({})", st.model), Style::default().fg(Color::DarkGray)),
                ]));
            }
            popup::Row::SaveAction => {
                let style = focus_style(selected);
                lines.push(Line::from(vec![
                    Span::styled(marker, style),
                    Span::styled("save active as…", style),
                ]));
            }
        }
    }

    // If we are in SaveAs mode, append an inline prompt row.
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

    // Compute centered modal area: 60% width, height = lines + 2 borders + 1
    // breathing row at top.
    let modal_w = (area.width as f32 * 0.60).max(50.0).min(area.width as f32) as u16;
    let modal_h = (lines.len() as u16 + 4).min(area.height);
    let modal_x = area.x + (area.width - modal_w) / 2;
    let modal_y = area.y + (area.height.saturating_sub(modal_h)) / 2;
    let modal_area = ratatui::layout::Rect {
        x: modal_x,
        y: modal_y,
        width: modal_w,
        height: modal_h,
    };

    // Clear underneath so the modal does not show through.
    f.render_widget(ratatui::widgets::Clear, modal_area);

    // Capture the line count before lines is moved into the Paragraph.
    let line_count = lines.len();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" station ");
    let widget = Paragraph::new(Text::from(lines))
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(widget, modal_area);

    // If we are in SaveAs, put the terminal cursor in the name field.
    if app.popup.mode == popup::Mode::SaveAs {
        // The name line is at index line_count - 3 (last 3 entries are
        // the name line, a blank, and the hint line).
        let name_line_idx = line_count.saturating_sub(3);
        let name_y = modal_area.y + 1 + name_line_idx as u16;
        let prompt_len = "  name: ".len() as u16;
        let caret = app.popup.name_input.display_col();
        f.set_cursor_position(ratatui::layout::Position {
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

fn push_message(out: &mut Vec<Line<'static>>, msg: &Message, area_width: u16) {
    let (role_color, role_text) = match msg.role {
        Role::User => (Color::Green, "you"),
        Role::Assistant => (Color::Magenta, "assistant"),
    };

    let mut header: Vec<Span<'static>> = vec![Span::styled(
        role_text.to_string(),
        Style::default()
            .fg(role_color)
            .add_modifier(Modifier::BOLD),
    )];
    if msg.streaming {
        let label = match msg.phase {
            Phase::Writing => Some("  writing…"),
            Phase::Thinking => Some("  thinking…"),
            Phase::Tinkering => Some("  tinkering…"),
            // Initial state. No chunk has arrived yet. Suppress the
            // generic "streaming…" filler; the empty header reads as
            // "waiting" cleanly enough.
            Phase::Streaming => None,
        };
        if let Some(l) = label {
            header.push(Span::styled(
                l.to_string(),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }

    // Build the right side of the header. Tool name (if any, while streaming)
    // sits just to the left of the timestamp with two spaces between them.
    let tool_span: Option<Span<'static>> = if msg.streaming {
        msg.current_tool.as_ref().map(|name| {
            Span::styled(
                name.clone(),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )
        })
    } else {
        None
    };
    let ts_span = Span::styled(
        msg.timestamp.clone(),
        Style::default().fg(Color::DarkGray),
    );

    // Width math. Pad with spaces between the header's left content and the
    // right cluster (tool name + timestamp).
    let left_width: usize = header
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum();
    let tool_width = tool_span
        .as_ref()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()) + 2)
        .unwrap_or(0);
    let ts_width = UnicodeWidthStr::width(msg.timestamp.as_str());
    let pad = (area_width as usize)
        .saturating_sub(left_width + tool_width + ts_width)
        .max(1);
    header.push(Span::raw(" ".repeat(pad)));
    if let Some(t) = tool_span {
        header.push(t);
        header.push(Span::raw("  "));
    }
    header.push(ts_span);
    out.push(Line::from(header));

    let has_reply = !msg.content.is_empty();
    let has_brain = !msg.brain.is_empty();
    let cursor_in_reply = msg.streaming && has_reply;
    let cursor_in_brain = msg.streaming && !has_reply && has_brain;
    let cursor_orphan = msg.streaming && !has_reply && !has_brain;

    if cursor_orphan {
        out.push(Line::from(Span::styled(
            "▌",
            Style::default().fg(Color::DarkGray),
        )));
    }

    // Reply (newest in time, sits at the top of this message's block).
    if has_reply {
        match msg.role {
            Role::Assistant => {
                out.extend(crate::md::render(&msg.content, cursor_in_reply));
            }
            Role::User => {
                let last_idx = msg.content.split('\n').count().saturating_sub(1);
                for (i, raw) in msg.content.split('\n').enumerate() {
                    if i == last_idx && cursor_in_reply {
                        out.push(Line::from(vec![
                            Span::raw(raw.to_string()),
                            Span::styled("▌", Style::default().fg(Color::DarkGray)),
                        ]));
                    } else {
                        out.push(Line::from(raw.to_string()));
                    }
                }
            }
        }
    }

    // Brain (older in time, sits beneath the reply as a footnote).
    if has_brain {
        if has_reply {
            out.push(Line::from(""));
        }
        let brain_style = Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC);
        out.push(Line::from(Span::styled(
            "brain",
            brain_style.add_modifier(Modifier::BOLD),
        )));
        let last_idx = msg.brain.split('\n').count().saturating_sub(1);
        for (i, raw) in msg.brain.split('\n').enumerate() {
            if i == last_idx && cursor_in_brain {
                out.push(Line::from(vec![
                    Span::styled(raw.to_string(), brain_style),
                    Span::styled("▌", Style::default().fg(Color::DarkGray)),
                ]));
            } else {
                out.push(Line::from(Span::styled(raw.to_string(), brain_style)));
            }
        }
    }
}

/// Approximate visual row count after wrapping. Sums each Line's display
/// width and rounds up by area width. Not exact (ratatui's word-boundary
/// wrap may add a row here or there) but close enough to count pages.
fn wrapped_row_count(lines: &[Line<'_>], area_width: u16) -> usize {
    let aw = (area_width as usize).max(1);
    let mut total = 0usize;
    for line in lines {
        let w: usize = line
            .spans
            .iter()
            .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        total += if w == 0 { 1 } else { w.div_ceil(aw) };
    }
    total
}
