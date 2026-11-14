use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

use crate::app::ViewMode;
use crate::config::NametagMode;
use crate::scene::{self, AWAITING, BRAIN_MARK, Bubble, HEART_MARK, Scene};
use crate::term::View;

const ERROR_BOX_MAX_ROWS: usize = 8;

pub fn draw(f: &mut Frame, scene: &Scene, view: &mut View) {
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
    let accent = scene.accent;

    let prompt = "› ";
    let bar = &scene.bar;
    let label = if bar.state.is_empty() {
        " wme. ".to_string()
    } else {
        format!(" wme. {} ", bar.state)
    };
    let esc = if bar.esc.is_empty() {
        String::new()
    } else {
        format!("{} ", bar.esc)
    };
    let mut input_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if bar.busy {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(accent)
        })
        .title(label)
        .title(Line::from(format!(" {esc}^s settings ^c exit ")).right_aligned());
    if let Some(tool) = &bar.tool {
        input_block = input_block.title_bottom(
            Line::from(Span::styled(
                format!(" {tool} "),
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ))
            .right_aligned(),
        );
    }

    let input = &view.input;
    let inner = Rect {
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
        Paragraph::new(Line::from(Span::styled(prompt, Style::default().fg(accent)))),
        Rect {
            x: inner.x,
            y: inner.y,
            width: prompt.len() as u16,
            height: inner.height,
        },
    );
    let text_area = Rect {
        x: inner.x + prompt.len() as u16,
        y: inner.y,
        width: visible_width as u16,
        height: inner.height,
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::raw(input.text.as_str()))).scroll((0, h_scroll as u16)),
        text_area,
    );

    let cursor_x = text_area.x + input.display_col() - h_scroll as u16;
    if scene.sheet.is_none() && cursor_x < text_area.x + text_area.width {
        f.set_cursor_position(Position {
            x: cursor_x,
            y: text_area.y,
        });
    }

    let mut lines: Vec<Line> = Vec::new();
    let msg_width = messages_chunk.width;
    for b in &scene.bubbles {
        push_message(&mut lines, b, msg_width, accent, scene.nametags);
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
    view.viewport_h = viewport_h;
    let total_rows = wrapped_row_count(&lines, messages_chunk.width);
    let base = assistant_base(scene, messages_chunk.width, viewport_h);
    view.page_base = base;
    let trigger = page_trigger(base, viewport_h);
    let n_pages = if total_rows == 0 || viewport_h == 0 {
        1
    } else {
        total_rows.div_ceil(viewport_h)
    };
    let mut page = view.page.min(n_pages.saturating_sub(1));
    if base > 0 && page >= 1 && viewport_h > 0 {
        let max_page = if base >= total_rows {
            1
        } else {
            (total_rows - base).div_ceil(viewport_h).max(1)
        };
        page = page.min(max_page).min(n_pages.saturating_sub(1).max(1));
    }
    view.page = page;

    let max_scroll = total_rows.saturating_sub(1);
    let scroll_offset = match scene.view {
        ViewMode::Page => {
            if base == 0 || page >= 1 {
                page_offset(base, page, 0, viewport_h)
            } else {
                view.scroll_row = view.scroll_row.min(trigger).min(max_scroll);
                view.scroll_row
            }
        }
        ViewMode::Scroll => {
            view.scroll_row = view.scroll_row.min(max_scroll);
            view.scroll_row
        }
    };
    let scroll_y = scroll_offset.min(u16::MAX as usize) as u16;

    f.render_widget(messages_para.scroll((scroll_y, 0)), messages_chunk);

    let status = &scene.status;
    let mut pieces = status.left.clone();
    if scene.bubbles.iter().any(|b| b.id != AWAITING) {
        pieces.push(Span::raw(scene::DOT));
        match scene.view {
            ViewMode::Page => {
                pieces.push(Span::styled(
                    format!("page {}/{}", page + 1, n_pages),
                    Style::default().fg(if n_pages > 1 { accent } else { Color::DarkGray }),
                ));
            }
            ViewMode::Scroll => {
                pieces.push(Span::styled(
                    if scroll_offset == 0 {
                        "scroll (top)".to_string()
                    } else {
                        format!("scroll +{}", scroll_offset)
                    },
                    Style::default().fg(accent),
                ));
            }
        }
    }
    pieces.push(Span::raw(scene::DOT));
    pieces.push(status.note.clone());
    if !status.right.is_empty() {
        let width = |spans: &[Span<'_>]| -> usize {
            spans
                .iter()
                .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                .sum()
        };
        let (left_w, tr_w) = (width(&pieces), width(&status.right));
        let bar_w = status_chunk.width as usize;
        if bar_w > left_w + tr_w + 2 {
            pieces.push(Span::raw(" ".repeat(bar_w - left_w - tr_w)));
        } else {
            pieces.push(Span::raw("  "));
        }
        pieces.extend(status.right.iter().cloned());
    }
    let status = Paragraph::new(Line::from(pieces)).style(Style::default().fg(Color::Gray));
    f.render_widget(status, status_chunk);

    if let Some(error) = &scene.error {
        draw_error_overlay(f, area, status_chunk, error);
    }

    if let Some(sheet) = &scene.sheet {
        crate::popup_ui::draw(f, sheet, view, accent);
    }

    paint_selection(f, view);
}

fn paint_selection(f: &mut Frame, view: &mut View) {
    let Some(sel) = view.selection else {
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
    view.screen = (0..area.height)
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

fn thinking_block(out: &mut Vec<Line<'static>>, label: &str, body: &[Line<'static>], gap: bool) {
    if body.is_empty() {
        return;
    }
    if gap {
        out.push(Line::from(""));
    }
    out.push(Line::from(Span::styled(
        label.to_string(),
        scene::dim().add_modifier(Modifier::ITALIC | Modifier::BOLD),
    )));
    out.extend(body.iter().cloned());
}

fn push_message(
    out: &mut Vec<Line<'static>>,
    b: &Bubble,
    area_width: u16,
    accent: Color,
    nametags: NametagMode,
) -> usize {
    if nametags == NametagMode::Inlined {
        return push_message_inlined(out, b, area_width);
    }
    let start = out.len();

    let mut header: Vec<Span<'static>> = vec![Span::styled(
        b.name.clone(),
        Style::default().fg(b.ink).add_modifier(Modifier::BOLD),
    )];
    if let Some(label) = b.doing {
        header.push(Span::styled(format!("  {label}"), scene::dim()));
    }
    if b.waiting {
        header.push(Span::styled("  waiting…", scene::dim()));
    }
    let tool_span = b.tool.clone().map(|name| {
        Span::styled(name, Style::default().fg(accent).add_modifier(Modifier::BOLD))
    });
    let ts_span = Span::styled(b.time.clone(), scene::dim());

    let left_width: usize = header
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum();
    let tool_width = tool_span
        .as_ref()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()) + 2)
        .unwrap_or(0);
    let ts_width = UnicodeWidthStr::width(b.time.as_str());
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

    if b.orphan {
        out.push(Line::from(scene::cursor()));
    }
    for img in &b.images {
        out.push(Line::from(Span::styled(
            format!("📷 attached: {img}"),
            scene::dim(),
        )));
    }
    out.extend(b.reply.iter().cloned());
    let reply_rows = out.len() - start;

    thinking_block(out, "brain", &b.brain, !b.reply.is_empty());
    thinking_block(out, "heart", &b.heart, !b.reply.is_empty() || !b.brain.is_empty());
    reply_rows
}

const INLINE_LEFT: usize = 2;
const INLINE_RIGHT: usize = 6;

fn blink_on() -> bool {
    ((crate::platform::unix_secs() * 2.0) as u64).is_multiple_of(2)
}

fn push_message_inlined(out: &mut Vec<Line<'static>>, b: &Bubble, area_width: u16) -> usize {
    let width = area_width as usize;
    let right = if b.waiting { "waiting…" } else { b.time.as_str() };
    let right_w = INLINE_RIGHT.max(UnicodeWidthStr::width(right) + 1);
    let text_w = width.saturating_sub(INLINE_LEFT + right_w).max(1);
    let dim = scene::dim();

    let mut rows: Vec<Line<'static>> = Vec::new();
    if b.orphan {
        rows.push(Line::from(scene::cursor()));
    }
    for img in &b.images {
        let line = Line::from(Span::styled(format!("📷 attached: {img}"), dim));
        rows.extend(wrap_line(line, text_w));
    }
    for (i, line) in b.reply.iter().enumerate() {
        let mut line = line.clone();
        if let (0, Some(from)) = (i, &b.guest) {
            line.spans
                .insert(0, Span::styled(format!("@{from} "), Style::default().fg(b.ink)));
        }
        rows.extend(wrap_line(line, text_w));
    }
    let reply_rows = rows.len().max(1);

    let thinking = dim.add_modifier(Modifier::ITALIC);
    let mut labels: Vec<(usize, &str)> = Vec::new();
    for (body, label) in [(&b.brain, BRAIN_MARK), (&b.heart, HEART_MARK)] {
        if body.is_empty() {
            continue;
        }
        if !rows.is_empty() {
            rows.push(Line::from(""));
        }
        labels.push((rows.len(), label));
        for line in body {
            rows.extend(wrap_line(line.clone(), text_w));
        }
    }
    if rows.is_empty() {
        rows.push(Line::from(""));
    }

    let lit = blink_on();
    let icon_w = UnicodeWidthStr::width(b.mark);
    let icon = if b.blinks && !lit {
        " ".repeat(icon_w)
    } else {
        b.mark.to_string()
    };
    let marks_lit = lit || !b.thinking_live;
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
                Style::default().fg(b.ink).add_modifier(Modifier::BOLD),
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
        if b.waiting {
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

fn assistant_base(scene: &Scene, width: u16, viewport_h: usize) -> usize {
    if viewport_h == 0 {
        return 0;
    }
    let mut bubbles = scene.bubbles.iter().peekable();
    let mut prefix: Vec<Line<'static>> = Vec::new();
    if let Some(waiting) = bubbles.next_if(|b| b.id == AWAITING) {
        push_message(&mut prefix, waiting, width, scene.accent, scene.nametags);
        prefix.push(Line::from(""));
    }
    let Some(newest) = bubbles.next() else {
        return 0;
    };
    if newest.user || newest.reply.is_empty() {
        return 0;
    }
    let mut head: Vec<Line<'static>> = Vec::new();
    let reply_rows = push_message(&mut head, newest, width, scene.accent, scene.nametags);
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
    use crate::app::{Message, Phase, Role};
    use crate::scene::USER_MARK;

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

    fn thinking_message(brain: &str, heart: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: "the answer".into(),
            images: Vec::new(),
            guest: None,
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

    fn paint(msg: &Message, width: u16, mode: NametagMode) -> (usize, Vec<Line<'static>>) {
        let mut out = Vec::new();
        let b = scene::bubble("0".into(), msg, false);
        let reply_rows = push_message(&mut out, &b, width, Color::Cyan, mode);
        (reply_rows, out)
    }

    fn rendered(msg: &Message) -> Vec<String> {
        let (_, out) = paint(msg, 80, NametagMode::Standout);
        out.iter().map(|l| l.to_string()).collect()
    }

    fn inlined(msg: &Message, width: u16) -> (usize, Vec<String>) {
        let (reply_rows, out) = paint(msg, width, NametagMode::Inlined);
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
        let (_, out) = paint(&msg, 30, NametagMode::Inlined);
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
    fn standout_reply_rows_include_header() {
        let (reply_rows, _) = paint(&thinking_message("pondering", ""), 80, NametagMode::Standout);
        assert_eq!(reply_rows, 2);
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
