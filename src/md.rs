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
    let content = content.replace('\t', "    ");
    let text = tui_markdown::from_str_with_options(&content, &Options::new(Sheet));
    let mut out: Vec<Line<'static>> = text.lines.into_iter().map(line_to_static).collect();
    out = gutter_fences(out);

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

fn gutter_fences(inp: Vec<Line<'static>>) -> Vec<Line<'static>> {
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
            out.extend(gutter_block(&body, &lang, azure));
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

fn gutter_block(body: &[Line<'static>], lang: &str, azure: Style) -> Vec<Line<'static>> {
    let mut out = Vec::with_capacity(body.len() + 1);
    let mut top = vec![Span::styled("╭ ", azure)];
    if !lang.is_empty() {
        top.push(Span::styled(lang.to_string(), azure));
    }
    out.push(Line::from(top));
    for line in body {
        let mut spans = vec![Span::styled("│ ", azure)];
        spans.extend(line.spans.iter().cloned());
        out.push(Line::from(spans));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gutters_fenced_code() {
        let out = render("```rust\nfn main() {}\n```", false);
        assert_eq!(out[0].spans[0].content, "╭ ");
        assert_eq!(out[0].spans[1].content, "rust");
        for l in &out[1..] {
            assert_eq!(l.spans[0].content, "│ ");
        }
        assert!(out.iter().all(|l| {
            !l.spans
                .iter()
                .any(|s| s.content.starts_with("```") || s.content.contains('╰'))
        }));
    }

    #[test]
    fn expands_tabs_in_code() {
        let out = render("```go\nfunc f() {\n\tif x {\n\t\tfmt.Println(1)\n\t}\n}\n```", false);
        for l in &out[1..] {
            for s in &l.spans[1..] {
                assert!(!s.content.contains('\t'));
            }
        }
        assert!(out.iter().any(|l| l.to_string().contains("    if x {")));
    }
}