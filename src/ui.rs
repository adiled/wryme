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
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
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

    // The prompt stays fixed on the left; only the text scrolls, so the
    // caret (and the letters being typed) stay pinned at the right edge
    // instead of running past it, while old text slides out the left side.
    let inner = ratatui::layout::Rect {
        x: chunks[0].x + 1,
        y: chunks[0].y + 1,
        width: chunks[0].width.saturating_sub(2),
        height: chunks[0].height.saturating_sub(2),
    };
    let visible_width = (inner.width as usize).saturating_sub(prompt.len());
    let h_scroll = input.scroll_offset(visible_width);

    // Draw the border + title.
    f.render_widget(Paragraph::new(Line::from("")).block(input_block), chunks[0]);
    // Draw the fixed prompt at the inner-left.
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            prompt,
            Style::default().fg(Color::Cyan),
        ))),
        ratatui::layout::Rect {
            x: inner.x,
            y: inner.y,
            width: prompt.len() as u16,
            height: inner.height,
        },
    );
    // Draw the text, scrolled so the caret hugs the right edge.
    let text_area = ratatui::layout::Rect {
        x: inner.x + prompt.len() as u16,
        y: inner.y,
        width: visible_width as u16,
        height: inner.height,
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::raw(&input.text))).scroll((0, h_scroll as u16)),
        text_area,
    );

    // Place the terminal cursor inside the input box.
    let cursor_x = text_area.x + input.display_col() - h_scroll as u16;
    let cursor_y = text_area.y;
    if cursor_x < text_area.x + text_area.width {
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
    // Write the clamped page back so navigation can never accumulate
    // phantom pages past the end (issue #6: scrolling past the last page
    // then reversing used to cost the same amount of extra scrolling).
    app.current_page = page;

    // Clamp the scroll offset to the last legal row so the user can't page
    // off into the empty void beyond the oldest line.
    let max_scroll = total_rows.saturating_sub(1);
    let scroll_offset = match app.view_mode {
        ViewMode::Page => page * viewport_h,
        ViewMode::Scroll => {
            // Same clamp-back as above: keep scroll_row inside the legal
            // range so reversing direction never has to eat phantom rows.
            app.scroll_row = app.scroll_row.min(max_scroll);
            app.scroll_row
        }
    };
    let scroll_y = scroll_offset.min(u16::MAX as usize) as u16;

    f.render_widget(messages_para.scroll((scroll_y, 0)), chunks[1]);

    // ---- status bar ----
    let dot = " • ";
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
        Color::Cyan
    };
    let mut pieces = vec![
        Span::styled("wryme", Style::default().fg(Color::Cyan)),
        Span::raw(dot),
        Span::styled(
            format!("station: {}", station_label),
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
        Style::default().fg(if is_error_status(&app.status) {
            Color::Red
        } else {
            Color::Gray
        }),
    ));
    let used = app.usage_ctx + app.usage_out;
    if used > 0 {
        let label = format_k(used);
        let left_w: usize = pieces
            .iter()
            .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        let bar_w = chunks[2].width as usize;
        let uw = UnicodeWidthStr::width(label.as_str());
        if bar_w > left_w + uw + 2 {
            pieces.push(Span::raw(" ".repeat(bar_w - left_w - uw)));
        } else {
            pieces.push(Span::raw("  "));
        }
        pieces.push(Span::styled(label, Style::default().fg(Color::DarkGray)));
    }
    let status = Paragraph::new(Line::from(pieces)).style(Style::default().fg(Color::Gray));
    f.render_widget(status, chunks[2]);

    // ---- error overlay (bottom-right half, wrapped) ----
    // Long upstream errors used to bleed past the right edge on the
    // single-line status bar. When the status is an error, also pop a
    // wrapped red box over the bottom-right half so the whole message
    // reads, even if it runs multiline.
    if is_error_status(&app.status) {
        draw_error_overlay(f, area, chunks[2], &app.status);
    }

    // ---- station popup overlay ----
    if app.popup.mode != popup::Mode::Closed {
        crate::popup_ui::draw(f, app);
    }
}

/// K-terms formatting for the usage meter: 950 -> "950", 12400 ->
/// "12.4K", 2_300_000 -> "2.3M".
fn format_k(n: u64) -> String {
    if n < 1000 {
        return n.to_string();
    }
    let f = n as f64;
    if n < 1_000_000 {
        let k = f / 1000.0;
        return format!("{:.1}K", (k * 10.0).round() / 10.0);
    }
    format!("{:.1}M", ((f / 1_000_000.0) * 10.0).round() / 10.0)
}

/// True when the status bar carries an error worth the red treatment
/// (and the wrapped overlay below).
fn is_error_status(s: &str) -> bool {
    s.starts_with("error")
        || s.starts_with("upstream")
        || s.starts_with("stopped:")
        || s.starts_with("save failed")
        || s.starts_with("update failed")
}

/// Wrapped error box over the bottom-right half of the screen, stacked
/// just above the status bar. Height fits the wrapped text (capped), so
/// short errors stay a small flag and long ones read multiline instead
/// of bleeding off the right edge.
fn draw_error_overlay(f: &mut Frame, area: Rect, status_chunk: Rect, msg: &str) {
    let box_w = (area.width / 2).clamp(24, area.width.max(24)) as usize;
    let inner_w = box_w.saturating_sub(4).max(10);
    let mut rows = wrap_words(msg, inner_w);
    if rows.is_empty() {
        rows.push(String::new());
    }
    // Cap the box so it never eats the whole window; extra lines clip.
    let max_rows = 8usize;
    rows.truncate(max_rows);
    let box_h = (rows.len() + 2) as u16;
    let x = area.width.saturating_sub(box_w as u16);
    // Stack above the status bar; clamp into the window on short screens.
    let y = status_chunk.y.saturating_sub(box_h).max(area.y);
    let err_area = Rect {
        x,
        y,
        width: box_w as u16,
        height: box_h.min(status_chunk.y.saturating_sub(area.y).max(3)),
    };
    if err_area.width < 12 || err_area.height < 3 {
        return;
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red))
        .title(Span::styled(" error ", Style::default().fg(Color::Red)));
    let text = Text::from(
        rows.into_iter()
            .map(|r| Line::from(Span::styled(r, Style::default().fg(Color::Red))))
            .collect::<Vec<_>>(),
    );
    f.render_widget(Clear, err_area);
    f.render_widget(Paragraph::new(text).block(block), err_area);
}

/// Greedy word wrap for the error box. Long words hard-break so a wall
/// of URL never overflows the box.
fn wrap_words(s: &str, width: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let push = |out: &mut Vec<String>, cur: &mut String| {
        if !cur.is_empty() {
            out.push(std::mem::take(cur));
        }
    };
    for word in s.split_whitespace() {
        // Hard-break words wider than the box first.
        let mut w = word;
        while unicode_width::UnicodeWidthStr::width(w) > width {
            let cut = cut_at_width(w, width.saturating_sub(1));
            if !cur.is_empty() {
                push(&mut out, &mut cur);
            }
            out.push(format!("{cut}-"));
            w = &w[cut.len()..];
        }
        let sep = if cur.is_empty() { 0 } else { 1 };
        if unicode_width::UnicodeWidthStr::width(cur.as_str())
            + sep
            + unicode_width::UnicodeWidthStr::width(w)
            > width
        {
            push(&mut out, &mut cur);
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(w);
    }
    push(&mut out, &mut cur);
    out
}

/// Byte index where the next `width` display columns end.
fn cut_at_width(s: &str, width: usize) -> &str {
    let mut w = 0usize;
    let mut end = 0usize;
    for (i, c) in s.char_indices() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw > width {
            break;
        }
        w += cw;
        end = i + c.len_utf8();
    }
    &s[..end]
}

fn push_message(out: &mut Vec<Line<'static>>, msg: &Message, area_width: u16) {
    let (role_color, role_text) = match msg.role {
        Role::User => (Color::Green, "you"),
        Role::Assistant => (Color::Magenta, "assistant"),
    };

    let mut header: Vec<Span<'static>> = vec![Span::styled(
        role_text.to_string(),
        Style::default().fg(role_color).add_modifier(Modifier::BOLD),
    )];
    if msg.streaming {
        // Hidden tools (the bookkeeper and the phantom async checker) are
        // treated visually, not as tools: the bookkeeper shows a quiet
        // "reminiscing…" and no tool name; the checker is fully invisible.
        // The app phase is still `Tinkering` — this is purely presentation.
        let hidden = msg
            .current_tool
            .as_ref()
            .map(|n| crate::tools::is_hidden_tool(n))
            .unwrap_or(false);
        let bookish = msg
            .current_tool
            .as_ref()
            .map(|n| crate::tools::is_book_tool(n))
            .unwrap_or(false);
        let label = match msg.phase {
            Phase::Writing => Some("  writing…"),
            Phase::Thinking => Some("  thinking…"),
            Phase::Tinkering => {
                if hidden {
                    if bookish {
                        Some("  reminiscing…")
                    } else {
                        None
                    }
                } else {
                    Some("  tinkering…")
                }
            }
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
    // Hidden tools (bookkeeper / phantom checker) never show a name.
    let tool_span: Option<Span<'static>> = if msg.streaming {
        msg.current_tool
            .as_ref()
            .filter(|name| !crate::tools::is_hidden_tool(name))
            .map(|name| {
                Span::styled(
                    name.clone(),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
            })
    } else {
        None
    };
    let ts_span = Span::styled(msg.timestamp.clone(), Style::default().fg(Color::DarkGray));

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
                for img in &msg.images {
                    out.push(Line::from(Span::styled(
                        format!("📷 attached: {img}"),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_box_wraps_inside_width() {
        let rows = wrap_words("upstream 400: this is a long error message", 12);
        assert!(rows.len() > 1);
        for r in &rows {
            assert!(UnicodeWidthStr::width(r.as_str()) <= 12, "overflow: {r}");
        }
        // Joined words survive intact, space-separated.
        assert_eq!(rows.join(" "), "upstream 400: this is a long error message");
    }

    #[test]
    fn error_box_hard_breaks_long_words() {
        let rows = wrap_words("https://example.com/very/long/path", 10);
        assert!(rows.len() > 1);
        for r in &rows {
            assert!(UnicodeWidthStr::width(r.trim_end_matches('-').to_string().as_str()) <= 10);
        }
    }

    #[test]
    fn usage_formats_in_k_terms() {
        assert_eq!(format_k(0), "0");
        assert_eq!(format_k(950), "950");
        assert_eq!(format_k(1000), "1.0K");
        assert_eq!(format_k(12400), "12.4K");
        assert_eq!(format_k(2_300_000), "2.3M");
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
