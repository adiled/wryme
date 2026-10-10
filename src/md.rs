use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use tui_markdown::{Options, StyleSheet};

const AZURE: Color = Color::Rgb(125, 185, 205);

#[derive(Clone)]
struct Sheet;

impl StyleSheet for Sheet {
    fn code(&self) -> Style {
        Style::default().fg(AZURE)
    }
}

pub fn render(content: &str, append_cursor: bool) -> Vec<Line<'static>> {
    let text = tui_markdown::from_str_with_options(content, &Options::new(Sheet));
    let mut out: Vec<Line<'static>> = text.lines.into_iter().map(line_to_static).collect();
    pop_fences(&mut out);

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

fn pop_fences(out: &mut Vec<Line<'static>>) {
    for line in out.iter_mut() {
        let fenced = line
            .spans
            .first()
            .map(|s| s.content.starts_with("```"))
            .unwrap_or(false);
        if fenced {
            line.style = Style::default().fg(AZURE);
        }
    }
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