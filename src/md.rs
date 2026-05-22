// Markdown rendering. Assistant replies often contain markdown that the
// previous renderer left as raw text (asterisks, backticks, fence lines).
// Here we feed the content through tui-markdown and convert the borrowed
// Text it returns into owned Lines so the result fits our static-lifetime
// line buffer in ui.rs.
//
// The streaming cursor span is appended to the very last line when the
// caller asks for it, so the caret sits at the end of the live text.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

pub fn render(content: &str, append_cursor: bool) -> Vec<Line<'static>> {
    let text = tui_markdown::from_str(content);
    let mut out: Vec<Line<'static>> = text
        .lines
        .into_iter()
        .map(line_to_static)
        .collect();

    if append_cursor {
        let cursor = Span::styled("▌", Style::default().fg(Color::DarkGray));
        if let Some(last) = out.last_mut() {
            last.spans.push(cursor);
        } else {
            out.push(Line::from(cursor));
        }
    }
    out
}

fn line_to_static(line: Line<'_>) -> Line<'static> {
    let spans: Vec<Span<'static>> = line
        .spans
        .into_iter()
        .map(|s| Span::styled(s.content.into_owned(), s.style))
        .collect();
    let mut owned = Line::from(spans);
    owned.style = line.style;
    if let Some(a) = line.alignment {
        owned = owned.alignment(a);
    }
    owned
}
