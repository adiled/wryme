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

use crate::app::{App, Message, Role};
use crate::input::Input;

pub fn draw(f: &mut Frame, app: &App, input: &Input, model: &str) {
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
            " write — Enter to send, Ctrl-C to quit "
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
    for msg in app.messages.iter().rev() {
        push_message(&mut lines, msg);
        lines.push(Line::from(""));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "no messages yet — type above and hit Enter",
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
    let pieces = vec![
        Span::styled("wryme", Style::default().fg(Color::Cyan)),
        Span::raw(dot),
        Span::raw(model.to_string()),
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

    let mut header = vec![
        Span::styled(
            role_text.to_string(),
            Style::default()
                .fg(role_color)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if msg.streaming {
        header.push(Span::styled(
            "  • streaming…",
            Style::default().fg(Color::DarkGray),
        ));
    }
    out.push(Line::from(header));

    if msg.content.is_empty() && msg.streaming {
        out.push(Line::from(Span::styled(
            "▌",
            Style::default().fg(Color::DarkGray),
        )));
        return;
    }

    let last_idx = msg.content.split('\n').count().saturating_sub(1);
    for (i, raw) in msg.content.split('\n').enumerate() {
        if i == last_idx && msg.streaming {
            out.push(Line::from(vec![
                Span::raw(raw.to_string()),
                Span::styled("▌", Style::default().fg(Color::DarkGray)),
            ]));
        } else {
            out.push(Line::from(raw.to_string()));
        }
    }
}
