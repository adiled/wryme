use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
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
    let (state, esc) = if app.in_flight {
        if app.voice_is_active() {
            ("streaming + speaking…", "esc quiet ")
        } else {
            ("streaming…", "esc interrupt ")
        }
    } else if app.voice_is_active() {
        ("speaking…", "esc quiet ")
    } else {
        ("", "")
    };
    let label = if state.is_empty() {
        " wme. ".to_string()
    } else {
        format!(" wme. {state} ")
    };
    let mut input_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if app.in_flight {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(app.accent)
        })
        .title(label)
        .title(Line::from(format!(" {esc}^s settings ^c exit ")).right_aligned());
    if let Some(tool) = inlined_tool(app) {
        input_block = input_block.title_bottom(
            Line::from(Span::styled(
                format!(" {tool} "),
                Style::default().fg(app.accent).add_modifier(Modifier::BOLD),
            ))
            .right_aligned(),
        );
    }

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
            Style::default().fg(app.accent),
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
    if !app.awaiting.is_empty() {
        let phantom = Message {
            role: Role::User,
            content: app.awaiting.clone(),
            images: Vec::new(),
            brain: String::new(),
            heart: String::new(),
            streaming: false,
            waiting: true,
            timestamp: String::new(),
            phase: Phase::Streaming,
            current_tool: None,
            turn_id: 0,
            tool_events: Vec::new(),
        };
        push_message(&mut lines, &phantom, msg_width, app.accent, app.nametags, false);
        lines.push(Line::from(""));
    }
    let live = thinking_turn(&app.messages);
    for msg in app.messages.iter().rev() {
        let thinking_live = live == Some(msg.turn_id);
        push_message(&mut lines, msg, msg_width, app.accent, app.nametags, thinking_live);
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
    let base = assistant_base(app, messages_chunk.width, viewport_h);
    app.page_base = base;
    let trigger = page_trigger(base, viewport_h);
    let n_pages = if total_rows == 0 || viewport_h == 0 {
        1
    } else {
        total_rows.div_ceil(viewport_h)
    };
    let mut page = app.current_page.min(n_pages.saturating_sub(1));
    if base > 0 && page >= 1 && viewport_h > 0 {
        let max_page = if base >= total_rows {
            1
        } else {
            (total_rows - base).div_ceil(viewport_h).max(1)
        };
        page = page.min(max_page).min(n_pages.saturating_sub(1).max(1));
    }
    app.current_page = page;

    let max_scroll = total_rows.saturating_sub(1);
    let scroll_offset = match app.view_mode {
        ViewMode::Page => {
            if base == 0 || page >= 1 {
                page_offset(base, page, 0, viewport_h)
            } else {
                app.scroll_row = app.scroll_row.min(trigger).min(max_scroll);
                app.scroll_row
            }
        }
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
        app.accent
    };
    let mut pieces = vec![
        Span::styled("wryme", Style::default().fg(app.accent)),
        Span::raw(dot),
        Span::styled(
            format!("station: {}", station_label),
            Style::default().fg(station_color),
        ),
        Span::styled(
            format!(" via {}", app.active_shop.name),
            Style::default().fg(Color::DarkGray),
        ),
    ];
    #[cfg(feature = "reservoir")]
    {
        let heart_score = app
            .reservoir
            .lock()
            .ok()
            .map(|r| r.static_figure())
            .unwrap_or(0);
        let ht =
            (heart_score as f64 / crate::reservoir::STATIC_BOT_SCORE as f64).clamp(0.0, 1.0);
        let heart_color = hue_lit(120.0 * (1.0 - ht));
        pieces.push(Span::styled(format!(" {HEART_MARK} "), heart_color));
    }
    #[cfg(not(feature = "reservoir"))]
    {
        pieces.push(Span::raw(dot));
    }
    pieces.push(Span::raw(format!("{} msg", app.messages.len())));
    if app.voice_on {
        pieces.push(Span::raw(dot));
        pieces.push(Span::styled("voice", Style::default().fg(app.accent)));
    }
    if !app.messages.is_empty() {
        pieces.push(Span::raw(dot));
        match app.view_mode {
            ViewMode::Page => {
                pieces.push(Span::styled(
                    format!("page {}/{}", page + 1, n_pages),
                    Style::default().fg(if n_pages > 1 {
                        app.accent
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
                    Style::default().fg(app.accent),
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

    paint_selection(f, app);
}

fn paint_selection(f: &mut Frame, app: &mut App) {
    let Some(sel) = app.selection else {
        return;
    };
    if sel.is_empty() {
        return;
    }
    let buf = f.buffer_mut();
    let area = buf.area;
    if area.width == 0 || area.height == 0 {
        return;
    }
    app.screen = (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect()
        })
        .collect();
    let (s, e) = sel.bounds();
    let last_row = e.row.min(area.height - 1);
    for y in s.row..=last_row {
        let col_start = if y == s.row { s.col } else { 0 };
        let col_end = if y == e.row { e.col } else { area.width - 1 };
        for x in col_start..=col_end.min(area.width - 1) {
            buf[(x, y)].modifier |= Modifier::REVERSED;
        }
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

#[cfg(feature = "reservoir")]
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
        .border_type(BorderType::Rounded)
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

fn push_message(
    out: &mut Vec<Line<'static>>,
    msg: &Message,
    area_width: u16,
    accent: Color,
    nametags: crate::config::NametagMode,
    thinking_live: bool,
) -> usize {
    let role_color = match msg.role {
        Role::User => Color::Green,
        Role::Assistant => Color::Magenta,
    };
    if nametags == crate::config::NametagMode::Inlined {
        return push_message_inlined(out, msg, area_width, role_color, thinking_live);
    }

    let role_text = match msg.role {
        Role::User => "you",
        Role::Assistant => "assistant",
    };
    let start = out.len();

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
            (Phase::Communing, ..) => Some("  communing…"),
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

    if msg.waiting {
        header.push(Span::styled(
            "  waiting…".to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    }

    let tool_span: Option<Span<'static>> = if msg.streaming {
        msg.current_tool
            .as_ref()
            .filter(|name| !crate::tools::is_hidden_tool(name))
            .map(|name| {
                Span::styled(
                    name.clone(),
                    Style::default()
                        .fg(accent)
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

    let reply_rows = out.len() - start;

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
    reply_rows
}

const INLINE_LEFT: usize = 2;
const INLINE_RIGHT: usize = 6;
const BRAIN_MARK: &str = "✶";
const HEART_MARK: &str = "\u{2764}\u{FE0E}";
const USER_MARK: &str = "\u{23F9}\u{FE0E}";

fn inline_icon(msg: &Message) -> (&'static str, bool) {
    if msg.role == Role::User {
        return (USER_MARK, false);
    }
    if !msg.streaming {
        return ("⌃", false);
    }
    let tool = msg.current_tool.as_deref();
    let hidden = tool.is_some_and(crate::tools::is_hidden_tool);
    let bookish = tool.is_some_and(crate::tools::is_book_tool);
    match msg.phase {
        Phase::Thinking => ("✦", true),
        Phase::Tinkering if hidden && bookish => ("಄", false),
        Phase::Tinkering | Phase::Communing => ("‣", true),
        Phase::Writing | Phase::Streaming => ("⬩", true),
    }
}

fn blink_on() -> bool {
    ((crate::platform::unix_secs() * 2.0) as u64).is_multiple_of(2)
}

fn inlined_tool(app: &App) -> Option<&str> {
    if app.nametags != crate::config::NametagMode::Inlined {
        return None;
    }
    app.messages
        .iter()
        .rev()
        .find(|m| m.role == Role::Assistant && m.streaming)?
        .current_tool
        .as_deref()
        .filter(|name| !crate::tools::is_hidden_tool(name))
}

fn thinking_turn(messages: &[Message]) -> Option<u64> {
    messages
        .iter()
        .rev()
        .find(|m| m.role == Role::Assistant && m.streaming && m.phase == Phase::Thinking)
        .map(|m| m.turn_id)
}

fn push_message_inlined(
    out: &mut Vec<Line<'static>>,
    msg: &Message,
    area_width: u16,
    role_color: Color,
    thinking_live: bool,
) -> usize {
    let width = area_width as usize;
    let right = if msg.waiting { "waiting…" } else { msg.timestamp.as_str() };
    let right_w = INLINE_RIGHT.max(UnicodeWidthStr::width(right) + 1);
    let text_w = width.saturating_sub(INLINE_LEFT + right_w).max(1);
    let dim = Style::default().fg(Color::DarkGray);
    let cursor = || Span::styled("▌", dim);

    let has_reply = !msg.content.is_empty();
    let has_brain = !msg.brain.is_empty();
    let has_heart = !msg.heart.is_empty();
    let cursor_in_reply = msg.streaming && has_reply;
    let cursor_in_brain = msg.streaming && !has_reply && has_brain;
    let cursor_in_heart = msg.streaming && !has_reply && !has_brain && has_heart;
    let cursor_orphan = msg.streaming && !has_reply && !has_brain && !has_heart;

    let mut rows: Vec<Line<'static>> = Vec::new();
    if cursor_orphan {
        rows.push(Line::from(cursor()));
    }
    if has_reply {
        match msg.role {
            Role::Assistant => {
                for line in crate::md::render(&msg.content, cursor_in_reply) {
                    rows.extend(wrap_line(line, text_w));
                }
            }
            Role::User => {
                for img in &msg.images {
                    let line = Line::from(Span::styled(format!("📷 attached: {img}"), dim));
                    rows.extend(wrap_line(line, text_w));
                }
                let last_idx = msg.content.split('\n').count().saturating_sub(1);
                for (i, raw) in msg.content.split('\n').enumerate() {
                    let mut spans = vec![Span::raw(raw.to_string())];
                    if i == last_idx && cursor_in_reply {
                        spans.push(cursor());
                    }
                    rows.extend(wrap_line(Line::from(spans), text_w));
                }
            }
        }
    }
    let reply_rows = rows.len().max(1);

    let thinking = dim.add_modifier(Modifier::ITALIC);
    let mut labels: Vec<(usize, &str)> = Vec::new();
    for (text, cur, label) in [
        (&msg.brain, cursor_in_brain, BRAIN_MARK),
        (&msg.heart, cursor_in_heart, HEART_MARK),
    ] {
        if text.is_empty() {
            continue;
        }
        if !rows.is_empty() {
            rows.push(Line::from(""));
        }
        labels.push((rows.len(), label));
        let last_idx = text.split('\n').count().saturating_sub(1);
        for (i, raw) in text.split('\n').enumerate() {
            let mut spans = vec![Span::styled(raw.to_string(), thinking)];
            if i == last_idx && cur {
                spans.push(cursor());
            }
            rows.extend(wrap_line(Line::from(spans), text_w));
        }
    }
    if rows.is_empty() {
        rows.push(Line::from(""));
    }

    let lit = blink_on();
    let (icon, blinks) = inline_icon(msg);
    let icon_w = UnicodeWidthStr::width(icon);
    let icon = if blinks && !lit {
        " ".repeat(icon_w)
    } else {
        icon.to_string()
    };
    let marks_lit = lit || !thinking_live;
    for (i, row) in rows.into_iter().enumerate() {
        let row_w = row.width();
        let label = labels.iter().find(|(at, _)| *at == i).map(|(_, l)| *l);
        let (mark, mark_w, mark_style) = match label {
            Some(label) => {
                let label_w = UnicodeWidthStr::width(label);
                let label = if marks_lit { label.to_string() } else { " ".repeat(label_w) };
                (label, label_w, thinking.add_modifier(Modifier::BOLD))
            }
            None if i == 0 => (
                icon.clone(),
                icon_w,
                Style::default().fg(role_color).add_modifier(Modifier::BOLD),
            ),
            None => (String::new(), 0, Style::default()),
        };
        let mut spans: Vec<Span<'static>> = Vec::with_capacity(row.spans.len() + 4);
        spans.push(Span::styled(mark, mark_style));
        spans.push(Span::raw(" ".repeat(INLINE_LEFT.saturating_sub(mark_w).max(1))));
        spans.extend(row.spans);
        if i == 0 && !right.is_empty() {
            let used = INLINE_LEFT + row_w + UnicodeWidthStr::width(right);
            spans.push(Span::raw(" ".repeat(width.saturating_sub(used).max(1))));
            spans.push(Span::styled(right.to_string(), dim));
        }
        if msg.waiting {
            for span in &mut spans {
                span.style = span.style.add_modifier(Modifier::DIM);
            }
        }
        out.push(Line::from(spans));
    }
    reply_rows
}

fn wrap_line(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    use unicode_width::UnicodeWidthChar;
    let cw = |c: char| c.width().unwrap_or(0);
    let width = width.max(1);
    let base = line.style;
    let cells = line.spans.iter().flat_map(|s| {
        let style = base.patch(s.style);
        s.content.chars().map(move |c| (c, style))
    });

    let mut rows: Vec<Vec<(char, Style)>> = Vec::new();
    let mut cur: Vec<(char, Style)> = Vec::new();
    let mut cur_w = 0usize;
    for (c, style) in cells {
        if cur_w + cw(c) > width && !cur.is_empty() {
            if c == ' ' {
                rows.push(std::mem::take(&mut cur));
                cur_w = 0;
                continue;
            }
            let brk = cur
                .iter()
                .rposition(|(ch, _)| *ch == ' ')
                .filter(|&i| cur[..i].iter().any(|(ch, _)| *ch != ' '));
            let rest = match brk {
                Some(i) => {
                    let rest = cur.split_off(i + 1);
                    cur.pop();
                    rest
                }
                None => Vec::new(),
            };
            rows.push(std::mem::replace(&mut cur, rest));
            cur_w = cur.iter().map(|(ch, _)| cw(*ch)).sum();
        }
        cur.push((c, style));
        cur_w += cw(c);
    }
    rows.push(cur);

    rows.into_iter()
        .map(|row| {
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (c, style) in row {
                match spans.last_mut() {
                    Some(last) if last.style == style => last.content.to_mut().push(c),
                    _ => spans.push(Span::styled(c.to_string(), style)),
                }
            }
            Line::from(spans)
        })
        .collect()
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

fn assistant_base(app: &App, width: u16, viewport_h: usize) -> usize {
    if viewport_h == 0 {
        return 0;
    }
    let Some(newest) = app.messages.last() else {
        return 0;
    };
    if newest.role != Role::Assistant || newest.content.is_empty() {
        return 0;
    }
    let mut prefix: Vec<Line<'static>> = Vec::new();
    if !app.awaiting.is_empty() {
        let phantom = Message {
            role: Role::User,
            content: app.awaiting.clone(),
            images: Vec::new(),
            brain: String::new(),
            heart: String::new(),
            streaming: false,
            waiting: true,
            timestamp: String::new(),
            phase: Phase::Streaming,
            current_tool: None,
            turn_id: 0,
            tool_events: Vec::new(),
        };
        push_message(&mut prefix, &phantom, width, app.accent, app.nametags, false);
        prefix.push(Line::from(""));
    }
    let mut head: Vec<Line<'static>> = Vec::new();
    let reply_rows = push_message(&mut head, newest, width, app.accent, app.nametags, false);
    prefix.extend(head.into_iter().take(reply_rows));
    let end = wrapped_row_count(&prefix, width);
    if end > viewport_h { end } else { 0 }
}

fn page_trigger(base: usize, viewport_h: usize) -> usize {
    base.saturating_sub(viewport_h / 2)
}

fn page_offset(base: usize, page: usize, scroll_row: usize, viewport_h: usize) -> usize {
    if base == 0 || viewport_h == 0 {
        return page.saturating_mul(viewport_h);
    }
    if page == 0 {
        return scroll_row.min(page_trigger(base, viewport_h));
    }
    base.saturating_add(page.saturating_sub(1).saturating_mul(viewport_h))
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
            waiting: false,
            timestamp: "00:00".into(),
            phase: Phase::Writing,
            current_tool: None,
            turn_id: 1,
            tool_events: Vec::new(),
        }
    }

    fn rendered(msg: &Message) -> Vec<String> {
        let mut out = Vec::new();
        push_message(&mut out, msg, 80, Color::Cyan, crate::config::NametagMode::Standout, false);
        out.iter().map(|l| l.to_string()).collect()
    }

    fn inlined(msg: &Message, width: u16) -> (usize, Vec<String>) {
        let mut out = Vec::new();
        let reply_rows =
            push_message(&mut out, msg, width, Color::Cyan, crate::config::NametagMode::Inlined, false);
        (reply_rows, out.iter().map(|l| l.to_string()).collect())
    }

    fn user_message(content: &str) -> Message {
        Message {
            role: Role::User,
            content: content.into(),
            ..thinking_message("", "")
        }
    }

    #[test]
    fn inlined_puts_icon_left_and_time_right_on_first_row() {
        let (_, rows) = inlined(&thinking_message("", ""), 40);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].starts_with("⌃ the answer"), "{:?}", rows[0]);
        assert!(rows[0].ends_with("00:00"), "{:?}", rows[0]);
        assert_eq!(UnicodeWidthStr::width(rows[0].as_str()), 40);
    }

    #[test]
    fn inlined_wraps_text_between_the_columns() {
        let text = "the quick brown fox jumps over the lazy dog and keeps on running far away";
        let (_, rows) = inlined(&user_message(text), 30);
        assert!(rows.len() > 2);
        assert!(rows[0].starts_with(&format!("{USER_MARK} ")), "{:?}", rows[0]);
        assert!(rows[0].ends_with("00:00"), "{:?}", rows[0]);
        for row in &rows[1..] {
            assert!(row.starts_with("  "), "{row:?}");
            assert!(UnicodeWidthStr::width(row.as_str()) <= 30 - INLINE_RIGHT, "{row:?}");
        }
        let body: Vec<String> = rows
            .iter()
            .map(|r| r.chars().skip(INLINE_LEFT).collect())
            .collect();
        let words: Vec<&str> = body
            .iter()
            .flat_map(|r| r.trim_end_matches("00:00").split_whitespace())
            .collect();
        assert_eq!(words.join(" "), text);
    }

    #[test]
    fn inlined_waiting_shows_on_the_right_and_keeps_rows_in_bounds() {
        let msg = Message {
            waiting: true,
            timestamp: String::new(),
            ..user_message("queued up while the reply was still streaming in")
        };
        let (_, rows) = inlined(&msg, 30);
        assert!(rows.len() > 1);
        assert!(rows[0].ends_with("waiting…"), "{:?}", rows[0]);
        for row in &rows {
            assert!(UnicodeWidthStr::width(row.as_str()) <= 30, "{row:?}");
        }
        let mut out = Vec::new();
        push_message(&mut out, &msg, 30, Color::Cyan, crate::config::NametagMode::Inlined, false);
        assert!(
            out.iter()
                .flat_map(|l| &l.spans)
                .all(|s| s.style.add_modifier.contains(Modifier::DIM))
        );
    }

    #[test]
    fn inlined_reply_rows_stop_before_thinking() {
        let (reply_rows, rows) = inlined(&thinking_message("pondering", ""), 40);
        assert_eq!(reply_rows, 1);
        assert_eq!(rows.len(), 3);
        assert!(rows[2].contains("pondering"));
    }

    #[test]
    fn inlined_marks_brain_and_heart_blocks() {
        let (_, rows) = inlined(&thinking_message("pondering", "feeling"), 40);
        assert_eq!(rows.len(), 5, "{rows:?}");
        assert_eq!(rows[2], format!("{BRAIN_MARK} pondering"));
        assert_eq!(rows[4], format!("{HEART_MARK} feeling"));
        assert_eq!(UnicodeWidthStr::width(HEART_MARK), 1);
    }

    #[test]
    fn inlined_thinking_on_the_first_row_shows_its_own_mark() {
        let msg = Message {
            content: String::new(),
            ..thinking_message("pondering", "")
        };
        let (_, rows) = inlined(&msg, 40);
        assert!(rows[0].starts_with(&format!("{BRAIN_MARK} pondering")), "{:?}", rows[0]);
        assert!(rows[0].ends_with("00:00"));
    }

    #[test]
    fn thinking_follows_the_turn_not_the_message() {
        let holder = || thinking_message("pondering", "feeling");
        let live = Message {
            streaming: true,
            phase: Phase::Thinking,
            ..thinking_message("", "")
        };
        assert_eq!(thinking_turn(&[holder(), live]), Some(holder().turn_id));
        assert_eq!(thinking_turn(&[holder()]), None);
    }

    #[test]
    fn standout_reply_rows_include_header() {
        let mut out = Vec::new();
        let reply_rows = push_message(
            &mut out,
            &thinking_message("pondering", ""),
            80,
            Color::Cyan,
            crate::config::NametagMode::Standout,
            false,
        );
        assert_eq!(reply_rows, 2);
    }

    #[test]
    fn inline_icon_follows_what_the_assistant_is_doing() {
        let at = |phase, tool: Option<&str>| {
            inline_icon(&Message {
                streaming: true,
                phase,
                current_tool: tool.map(String::from),
                ..thinking_message("", "")
            })
        };
        assert_eq!(inline_icon(&thinking_message("", "")), ("⌃", false));
        assert_eq!(inline_icon(&user_message("hi")), (USER_MARK, false));
        assert_eq!(UnicodeWidthStr::width(USER_MARK), 1);
        assert_eq!(at(Phase::Writing, None), ("⬩", true));
        assert_eq!(at(Phase::Thinking, None), ("✦", true));
        assert_eq!(at(Phase::Tinkering, Some("shell")), ("‣", true));
        assert_eq!(at(Phase::Tinkering, Some(crate::tools::BOOK_NAME)), ("಄", false));
    }

    #[test]
    fn wrap_line_keeps_styles_across_breaks() {
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let line = Line::from(vec![Span::raw("plain words "), Span::styled("bold words", bold)]);
        let rows = wrap_line(line, 11);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].to_string(), "plain words");
        assert_eq!(rows[1].to_string(), "bold words");
        assert!(rows[1].spans.iter().all(|s| s.style == bold));
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

    #[test]
    fn page_starts_at_content_end_when_bled() {
        assert_eq!(page_trigger(50, 20), 40);
        assert_eq!(page_offset(50, 0, 0, 20), 0);
        assert_eq!(page_offset(50, 0, 30, 20), 30);
        assert_eq!(page_offset(50, 0, 40, 20), 40);
        assert_eq!(page_offset(50, 0, 90, 20), 40);
        assert_eq!(page_offset(50, 1, 0, 20), 50);
        assert_eq!(page_offset(50, 2, 0, 20), 70);
        assert_eq!(page_offset(50, 3, 0, 20), 90);
        assert_eq!(page_offset(0, 1, 0, 20), 20);
    }
}
