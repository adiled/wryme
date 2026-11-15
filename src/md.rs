use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use tui_markdown::{Options, StyleSheet};

pub const AZURE: Color = Color::Rgb(125, 185, 205);

#[derive(Clone)]
struct Sheet;

impl StyleSheet for Sheet {
    fn code(&self) -> Style {
        Style::default().fg(AZURE)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Prose,
    Head,
    Code,
    Table,
}

impl Kind {
    pub const ALL: &[Kind] = &[Kind::Prose, Kind::Head, Kind::Code, Kind::Table];
}

#[derive(Clone, Debug)]
pub struct Row {
    pub kind: Kind,
    pub line: Line<'static>,
}

impl Row {
    pub fn prose(line: Line<'static>) -> Self {
        Row { kind: Kind::Prose, line }
    }
}

pub fn render(content: &str, append_cursor: bool) -> Vec<Row> {
    let content = content.replace('\t', "    ");
    let text = tui_markdown::from_str_with_options(&content, &Options::new(Sheet));
    let mut out = rows(text.lines.into_iter().map(line_to_static));
    if append_cursor {
        match out.last_mut() {
            Some(last) => last.line.spans.push(cursor()),
            None => out.push(Row::prose(Line::from(cursor()))),
        }
    }
    out
}

pub fn cursor() -> Span<'static> {
    Span::styled("▌", Style::default().fg(Color::DarkGray))
}

fn rows(mut lines: impl Iterator<Item = Line<'static>>) -> Vec<Row> {
    let mut out = Vec::new();
    while let Some(line) = lines.next() {
        let Some(lang) = fence_lang(&line) else {
            let kind = if table(&line) { Kind::Table } else { Kind::Prose };
            out.push(Row { kind, line });
            continue;
        };
        out.push(Row { kind: Kind::Head, line: Line::from(Span::styled(lang, Style::default().fg(AZURE))) });
        let body = lines.by_ref().take_while(|l| fence_lang(l).is_none());
        out.extend(body.map(|line| Row { kind: Kind::Code, line }));
    }
    out
}

fn table(line: &Line<'_>) -> bool {
    line.spans
        .first()
        .is_some_and(|s| s.content.starts_with(['┌', '├', '└', '│']))
}

fn fence_lang(line: &Line<'static>) -> Option<String> {
    line.spans
        .first()
        .and_then(|s| s.content.strip_prefix("```"))
        .map(|l| l.trim().to_string())
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
    fn fenced_code_is_a_head_then_bare_code() {
        let out = render("```rust\nfn main() {}\n```\nafter", false);
        assert_eq!(out[0].kind, Kind::Head);
        assert_eq!(out[0].line.to_string(), "rust");
        assert_eq!(out[1].kind, Kind::Code);
        assert_eq!(out[1].line.to_string(), "fn main() {}");
        assert!(out.iter().all(|r| !r.line.to_string().contains("```")));
        assert_eq!(out.last().map(|r| r.kind), Some(Kind::Prose));
    }

    #[test]
    fn expands_tabs_in_code() {
        let out = render("```go\nfunc f() {\n\tif x {\n\t\tfmt.Println(1)\n\t}\n}\n```", false);
        assert!(out.iter().all(|r| !r.line.to_string().contains('\t')));
        assert!(out.iter().any(|r| r.line.to_string().contains("    if x {")));
    }

    #[test]
    fn lines_know_what_they_are() {
        let out = render("say\n\n```rust\nlet x = 1;\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |", false);
        let kinds: Vec<Kind> = out.iter().map(|r| r.kind).collect();
        assert_eq!(kinds[0], Kind::Prose);
        assert!(kinds.contains(&Kind::Head));
        assert!(kinds.contains(&Kind::Code));
        assert!(kinds.contains(&Kind::Table));
    }
}
