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
    layout::{Alignment, Constraint, Direction, Layout, Position},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Message, Role};
use crate::input::Input;
use crate::station::Station;

pub fn draw(f: &mut Frame, app: &App, input: &Input, station: &Station) {
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
    // Right-anchored: text is right-aligned within the box. A trailing space
    // reserves the rightmost inner column for the terminal cursor when the
    // caret is at the end. The "now" of the conversation lives at this right
    // edge: typing happens here, and the assistant reply emerges from here.
    let display_text = format!("{} ", input.text);
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
    let input_widget = Paragraph::new(display_text)
        .alignment(Alignment::Right)
        .block(input_block);
    f.render_widget(input_widget, chunks[0]);

    // Terminal cursor sits at the column corresponding to the caret's
    // position within the rendered (right-aligned) string.
    let text_w = UnicodeWidthStr::width(input.text.as_str()) as u16;
    let inner_right = chunks[0].x + chunks[0].width.saturating_sub(2);
    let inner_left = chunks[0].x + 1;
    let width_after_caret = text_w.saturating_sub(input.display_col());
    let cursor_x = inner_right.saturating_sub(width_after_caret).max(inner_left);
    let cursor_y = chunks[0].y + 1;
    f.set_cursor_position(Position {
        x: cursor_x,
        y: cursor_y,
    });

    // ---- messages (middle, newest first) ----
    let mut lines: Vec<Line> = Vec::new();
    for msg in app.messages.iter().rev() {
        push_message(&mut lines, msg);
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
    let messages = Paragraph::new(Text::from(lines))
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::NONE));
    f.render_widget(messages, chunks[1]);

    // ---- status bar ----
    let dot = " • ";
    let station_color = if station.is_demo {
        Color::Yellow
    } else {
        Color::Cyan
    };
    let pieces = vec![
        Span::styled("wryme", Style::default().fg(Color::Cyan)),
        Span::raw(dot),
        Span::styled(
            format!("station: {}", station.name),
            Style::default().fg(station_color),
        ),
        Span::raw(dot),
        Span::raw(station.model.clone()),
        Span::raw(dot),
        Span::raw(format!("{} msg", app.messages.len())),
        Span::raw(dot),
        Span::styled(
            app.status.clone(),
            Style::default().fg(if app.status.starts_with("error")
                || app.status.starts_with("upstream")
            {
                Color::Red
            } else {
                Color::Gray
            }),
        ),
    ];
    let status = Paragraph::new(Line::from(pieces)).style(Style::default().fg(Color::Gray));
    f.render_widget(status, chunks[2]);
}

fn push_message(out: &mut Vec<Line<'static>>, msg: &Message) {
    let (role_color, role_text) = match msg.role {
        Role::User => (Color::Green, "you"),
        Role::Assistant => (Color::Magenta, "assistant"),
    };
    // Assistant is right-anchored: header and reply align flush right.
    // User and brain stay flush left.
    let reply_align = match msg.role {
        Role::User => Alignment::Left,
        Role::Assistant => Alignment::Right,
    };

    let mut header = vec![
        Span::styled(
            role_text.to_string(),
            Style::default()
                .fg(role_color)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if msg.streaming {
        let label = if !msg.content.is_empty() {
            "  writing…"
        } else if !msg.brain.is_empty() {
            "  thinking…"
        } else {
            "  streaming…"
        };
        header.push(Span::styled(
            label.to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    }
    out.push(Line::from(header).alignment(reply_align));

    let has_reply = !msg.content.is_empty();
    let has_brain = !msg.brain.is_empty();
    let cursor_in_reply = msg.streaming && has_reply;
    let cursor_in_brain = msg.streaming && !has_reply && has_brain;
    let cursor_orphan = msg.streaming && !has_reply && !has_brain;

    if cursor_orphan {
        out.push(
            Line::from(Span::styled(
                "▌",
                Style::default().fg(Color::DarkGray),
            ))
            .alignment(reply_align),
        );
    }

    // Reply (newest in time, sits at the top of this message's block).
    if has_reply {
        let last_idx = msg.content.split('\n').count().saturating_sub(1);
        for (i, raw) in msg.content.split('\n').enumerate() {
            let line = if i == last_idx && cursor_in_reply {
                Line::from(vec![
                    Span::raw(raw.to_string()),
                    Span::styled("▌", Style::default().fg(Color::DarkGray)),
                ])
            } else {
                Line::from(raw.to_string())
            };
            out.push(line.alignment(reply_align));
        }
    }

    // Brain (older in time, sits beneath the reply as a footnote). Always
    // left-aligned regardless of which role owns the message, because the
    // brain belongs to the reflective margin.
    if has_brain {
        if has_reply {
            out.push(Line::from(""));
        }
        let brain_style = Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC);
        out.push(
            Line::from(Span::styled(
                "brain",
                brain_style.add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Left),
        );
        let last_idx = msg.brain.split('\n').count().saturating_sub(1);
        for (i, raw) in msg.brain.split('\n').enumerate() {
            let line = if i == last_idx && cursor_in_brain {
                Line::from(vec![
                    Span::styled(raw.to_string(), brain_style),
                    Span::styled("▌", Style::default().fg(Color::DarkGray)),
                ])
            } else {
                Line::from(Span::styled(raw.to_string(), brain_style))
            };
            out.push(line.alignment(Alignment::Left));
        }
    }
}
