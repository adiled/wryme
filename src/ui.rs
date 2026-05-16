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

    // ---- messages (middle, newest first) ----
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

fn push_message(out: &mut Vec<Line<'static>>, msg: &Message, area_width: u16) {
    let (role_color, role_text) = match msg.role {
        Role::User => (Color::Green, "you"),
        Role::Assistant => (Color::Magenta, "assistant"),
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
        let no_newlines_yet = !msg.content.contains('\n');
        let last_idx = msg.content.split('\n').count().saturating_sub(1);

        for (i, raw) in msg.content.split('\n').enumerate() {
            let is_last = i == last_idx;
            if is_last && cursor_in_reply && no_newlines_yet {
                // Center-spawn: the most recent delta lives at the right of
                // the line, and its left edge sits at screen-center. As more
                // deltas arrive the prev portion grows leftward into the
                // padding, shoving the spawn point left toward column 0.
                // Once the prev portion is at least half the screen wide,
                // padding hits zero and we render flush-left like normal.
                let split = msg.last_delta_byte.min(raw.len());
                let prev = &raw[..split];
                let latest = &raw[split..];
                let prev_width = UnicodeWidthStr::width(prev);
                let half = (area_width as usize) / 2;
                let padding = half.saturating_sub(prev_width);

                let mut spans: Vec<Span> = Vec::with_capacity(4);
                if padding > 0 {
                    spans.push(Span::raw(" ".repeat(padding)));
                }
                spans.push(Span::raw(prev.to_string()));
                spans.push(Span::raw(latest.to_string()));
                spans.push(Span::styled("▌", Style::default().fg(Color::DarkGray)));
                out.push(Line::from(spans));
            } else if is_last && cursor_in_reply {
                out.push(Line::from(vec![
                    Span::raw(raw.to_string()),
                    Span::styled("▌", Style::default().fg(Color::DarkGray)),
                ]));
            } else {
                out.push(Line::from(raw.to_string()));
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
