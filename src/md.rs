use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use tui_markdown::{Options, StyleSheet};
use unicode_width::UnicodeWidthStr;

const AZURE: Color = Color::Rgb(125, 185, 205);

#[derive(Clone)]
struct Sheet;

impl StyleSheet for Sheet {
    fn code(&self) -> Style {
        Style::default().fg(AZURE)
    }
}

pub fn render(content: &str, append_cursor: bool, width: usize) -> Vec<Line<'static>> {
    let text = tui_markdown::from_str_with_options(content, &Options::new(Sheet));
    let mut out: Vec<Line<'static>> = text.lines.into_iter().map(line_to_static).collect();
    out = box_fences(out, width);

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

fn box_fences(inp: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    let azure = Style::default().fg(AZURE);
    let mut out = Vec::with_capacity(inp.len());
    let mut i = 0;
    while i < inp.len() {
        if let Some(lang) = fence_lang(&inp[i]) {
            let mut body = Vec::new();
            i += 1;
            while i < inp.len() && fence_lang(&inp[i]).is_none() {
                body.push(inp[i].clone());
                i += 1;
            }
            i += 1;
            out.extend(box_lines(&body, &lang, width, azure));
        } else {
            out.push(inp[i].clone());
            i += 1;
        }
    }
    out
}

fn fence_lang(line: &Line<'static>) -> Option<String> {
    line.spans
        .first()
        .and_then(|s| s.content.strip_prefix("```"))
        .map(|l| l.trim().to_string())
}

fn box_lines(body: &[Line<'static>], lang: &str, width: usize, azure: Style) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(4).max(1);
    let title_width = if lang.is_empty() {
        0
    } else {
        UnicodeWidthStr::width(lang) + 2
    };
    let top_pad = width.saturating_sub(3 + title_width);
    let mut out = Vec::with_capacity(body.len() + 2);
    let mut top = vec![Span::styled("╭─", azure)];
    if !lang.is_empty() {
        top.push(Span::styled(format!(" {lang} "), azure));
    }
    top.push(Span::styled("─".repeat(top_pad), azure));
    top.push(Span::styled("╮", azure));
    out.push(Line::from(top));

    for line in body {
        let bw: usize = line
            .spans
            .iter()
            .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        let mut spans = vec![Span::styled("│ ", azure)];
        spans.extend(line.spans.iter().cloned());
        if bw <= inner {
            spans.push(Span::raw(" ".repeat(inner - bw)));
            spans.push(Span::styled("│", azure));
        }
        out.push(Line::from(spans));
    }

    out.push(Line::from(vec![
        Span::styled("╰", azure),
        Span::styled("─".repeat(width.saturating_sub(2)), azure),
        Span::styled("╯", azure),
    ]));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_fenced_code() {
        let out = render("```rust\nfn main() {}\n```", false, 40);
        assert!(out[0].spans[0].content.starts_with("╭"));
        assert!(out[1].spans[0].content.starts_with("│"));
        assert!(out[2].spans[0].content.starts_with("╰"));
    }

    #[test]
    fn strips_lone_fence_after_block() {
        let out = render("code\n```\nbody\n```", false, 40);
        assert!(out.iter().all(|l| !l.spans.iter().any(
            |s| s.content.strip_prefix("```").is_some()
        )));
    }
}