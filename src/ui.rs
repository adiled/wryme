use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Message, Phase, Role, ViewMode};
use crate::input::Input;
use crate::popup;
use crate::shop::Protocol;

const ERROR_BOX_MAX_ROWS: usize = 8;

pub fn draw(f: &mut Frame, app: &mut App, input: &Input) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);
    let (input_chunk, messages_chunk, status_chunk) = (chunks[0], chunks[1], chunks[2]);

    let prompt = "› ";
    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_style(if app.in_flight {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(Color::Cyan)
        })
        .title(if app.in_flight {
            if app.voice_is_active() {
                " streaming + speaking… (Esc to quiet) "
            } else {
                " streaming… (Esc to interrupt) "
            }
        } else if app.voice_is_active() {
            " speaking… (Esc to quiet) "
        } else {
            " write. Enter to send, Ctrl-C to quit "
        });

    let inner = ratatui::layout::Rect {
        x: input_chunk.x + 1,
        y: input_chunk.y + 1,
        width: input_chunk.width.saturating_sub(2),
        height: input_chunk.height.saturating_sub(2),
    };
    let visible_width = (inner.width as usize).saturating_sub(prompt.len());
    let h_scroll = input.scroll_offset(visible_width);

    f.render_widget(
        Paragraph::new(Line::from("")).block(input_block),
        input_chunk,
    );
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

    let cursor_x = text_area.x + input.display_col() - h_scroll as u16;
    let cursor_y = text_area.y;
    if cursor_x < text_area.x + text_area.width {
        f.set_cursor_position(Position {
            x: cursor_x,
            y: cursor_y,
        });
    }

    let mut lines: Vec<Line> = Vec::new();
    let msg_width = messages_chunk.width;
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

    let viewport_h = messages_chunk.height as usize;
    app.last_viewport_h = viewport_h;
    let total_rows = wrapped_row_count(&lines, messages_chunk.width);
    let n_pages = if total_rows == 0 || viewport_h == 0 {
        1
    } else {
        total_rows.div_ceil(viewport_h)
    };
    let page = app.current_page.min(n_pages.saturating_sub(1));
    app.current_page = page;

    let max_scroll = total_rows.saturating_sub(1);
    let scroll_offset = match app.view_mode {
        ViewMode::Page => page * viewport_h,
        ViewMode::Scroll => {
            app.scroll_row = app.scroll_row.min(max_scroll);
            app.scroll_row
        }
    };
    let scroll_y = scroll_offset.min(u16::MAX as usize) as u16;

    f.render_widget(messages_para.scroll((scroll_y, 0)), messages_chunk);

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
    let heart_score = app
        .reservoir
        .lock()
        .ok()
        .map(|r| r.static_figure())
        .unwrap_or(0);
    let ht = (heart_score as f64 / crate::reservoir::STATIC_BOT_SCORE as f64).clamp(0.0, 1.0);
    let heart_color = hue_lit(120.0 * (1.0 - ht));
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
        Span::styled(" \u{2764}\u{FE0E} ", heart_color),
        Span::raw(format!("{} msg", app.messages.len())),
    ];
    if app.voice_on {
        pieces.push(Span::raw(dot));
        pieces.push(Span::styled("voice", Style::default().fg(Color::Cyan)));
    }
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
    let mut trailer: Vec<Span<'static>> = Vec::new();
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
        trailer.push(Span::styled(format_k(used), Style::default().fg(color)));
    }
    if !trailer.is_empty() {
        let left_w: usize = pieces
            .iter()
            .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        let tr_w: usize = trailer
            .iter()
            .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        let bar_w = status_chunk.width as usize;
        if bar_w > left_w + tr_w + 2 {
            pieces.push(Span::raw(" ".repeat(bar_w - left_w - tr_w)));
        } else {
            pieces.push(Span::raw("  "));
        }
        pieces.extend(trailer);
    }
    let status = Paragraph::new(Line::from(pieces)).style(Style::default().fg(Color::Gray));
    f.render_widget(status, status_chunk);

    if is_error_status(&app.status) {
        draw_error_overlay(f, area, status_chunk, &app.status);
    }

    if app.popup.mode != popup::Mode::Closed {
        crate::popup_ui::draw(f, app);
    }
}

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

fn is_error_status(s: &str) -> bool {
    s.starts_with("error")
        || s.starts_with("upstream")
        || s.starts_with("stopped:")
        || s.starts_with("save failed")
        || s.starts_with("update failed")
}

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

fn draw_error_overlay(f: &mut Frame, area: Rect, status_chunk: Rect, msg: &str) {
    let box_w = (area.width / 2).clamp(24, area.width.max(24)) as usize;
    let inner_w = box_w.saturating_sub(4).max(10);
    let mut rows = wrap_words(msg, inner_w);
    if rows.is_empty() {
        rows.push(String::new());
    }
    rows.truncate(ERROR_BOX_MAX_ROWS);
    let box_h = (rows.len() + 2) as u16;
    let x = area.width.saturating_sub(box_w as u16);
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

fn wrap_words(s: &str, width: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let push = |out: &mut Vec<String>, cur: &mut String| {
        if !cur.is_empty() {
            out.push(std::mem::take(cur));
        }
    };
    for word in s.split_whitespace() {
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

fn thinking_block(out: &mut Vec<Line<'static>>, label: &str, text: &str, cursor: bool, gap: bool) {
    if gap {
        out.push(Line::from(""));
    }
    let body = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::ITALIC);
    out.push(Line::from(Span::styled(
        label.to_string(),
        body.add_modifier(Modifier::BOLD),
    )));
    let last_idx = text.split('\n').count().saturating_sub(1);
    for (i, raw) in text.split('\n').enumerate() {
        if i == last_idx && cursor {
            out.push(Line::from(vec![
                Span::styled(raw.to_string(), body),
                Span::styled("▌", Style::default().fg(Color::DarkGray)),
            ]));
        } else {
            out.push(Line::from(Span::styled(raw.to_string(), body)));
        }
    }
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
        let label = match (msg.phase, hidden, bookish) {
            (Phase::Writing, ..) => Some("  writing…"),
            (Phase::Thinking, ..) => Some("  thinking…"),
            (Phase::Tinkering, false, _) => Some("  tinkering…"),
            (Phase::Tinkering, true, true) => Some("  reminiscing…"),
            (Phase::Tinkering, true, false) => None,
            (Phase::Streaming, ..) => None,
        };
        if let Some(l) = label {
            header.push(Span::styled(
                l.to_string(),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }

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
    let has_heart = !msg.heart.is_empty();
    let cursor_in_reply = msg.streaming && has_reply;
    let cursor_in_brain = msg.streaming && !has_reply && has_brain;
    let cursor_in_heart = msg.streaming && !has_reply && !has_brain && has_heart;
    let cursor_orphan = msg.streaming && !has_reply && !has_brain && !has_heart;

    if cursor_orphan {
        out.push(Line::from(Span::styled(
            "▌",
            Style::default().fg(Color::DarkGray),
        )));
    }

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

    if has_brain {
        thinking_block(out, "brain", &msg.brain, cursor_in_brain, has_reply);
    }

    if has_heart {
        thinking_block(
            out,
            "heart",
            &msg.heart,
            cursor_in_heart,
            has_reply || has_brain,
        );
    }
}

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

    fn thinking_message(brain: &str, heart: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: "the answer".into(),
            images: Vec::new(),
            brain: brain.into(),
            heart: heart.into(),
            streaming: false,
            timestamp: "00:00".into(),
            phase: Phase::Writing,
            current_tool: None,
            turn_id: 1,
            tool_events: Vec::new(),
        }
    }

    fn rendered(msg: &Message) -> Vec<String> {
        let mut out = Vec::new();
        push_message(&mut out, msg, 80);
        out.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn heart_sits_below_brain() {
        let lines = rendered(&thinking_message("a retelling", "the raw trace"));
        let brain = lines.iter().position(|l| l.contains("brain"));
        let heart = lines.iter().position(|l| l.contains("heart"));
        assert!(brain.is_some(), "brain label missing: {lines:?}");
        assert!(heart.is_some(), "heart label missing: {lines:?}");
        assert!(brain < heart, "heart must follow brain: {lines:?}");
        assert!(lines.iter().any(|l| l.contains("a retelling")));
        assert!(lines.iter().any(|l| l.contains("the raw trace")));
    }

    #[test]
    fn heart_renders_without_a_brain() {
        let lines = rendered(&thinking_message("", "only the trace"));
        assert!(lines.iter().any(|l| l.contains("heart")));
        assert!(!lines.iter().any(|l| l.contains("brain")));
    }
}
