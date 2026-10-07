//! `Backend` over the DOM: one `<div>` per row, spans with inline styles,
//! a block cursor. Ratatui diffs cells; we turn the diff into row HTML.

use std::io;

use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use ratatui::style::{Color, Modifier};
use unicode_width::UnicodeWidthStr;
use wasm_bindgen::JsCast;
use web_sys::{Document, HtmlElement};

use super::TERM_ID;

/// Cells are kept as the value ratatui hands us; `PartialEq` on this is what
/// lets us skip a row that the diff already proved unchanged.
#[derive(Clone, PartialEq)]
struct Painted {
    sym: String,
    fg: Color,
    bg: Color,
    modifier: Modifier,
}

impl Painted {
    fn blank() -> Self {
        Self {
            sym: " ".into(),
            fg: Color::Reset,
            bg: Color::Reset,
            modifier: Modifier::empty(),
        }
    }
}

pub struct WebBackend {
    doc: Document,
    container: HtmlElement,
    rows: Vec<HtmlElement>,
    cursor_el: Option<HtmlElement>,
    grid: Vec<Painted>,
    dirty: Vec<bool>,
    cols: u16,
    n_rows: u16,
    char_w: f64,
    line_h: f64,
    cursor: Position,
    cursor_visible: bool,
}

impl WebBackend {
    pub fn new() -> io::Result<Self> {
        let doc = web_sys::window()
            .and_then(|w| w.document())
            .ok_or_else(|| io::Error::other("no document"))?;
        let el = doc
            .get_element_by_id(TERM_ID)
            .ok_or_else(|| io::Error::other("missing #wryme-term element"))?;
        let container: HtmlElement = el
            .dyn_into()
            .map_err(|_| io::Error::other("#wryme-term is not an HTMLElement"))?;
        let (char_w, line_h) = Self::measure(&doc);

        let mut backend = Self {
            doc,
            container,
            rows: Vec::new(),
            cursor_el: None,
            grid: Vec::new(),
            dirty: Vec::new(),
            cols: 0,
            n_rows: 0,
            char_w,
            line_h,
            cursor: Position::ORIGIN,
            cursor_visible: true,
        };
        let size = backend.size()?;
        backend.rebuild(size.width, size.height);
        Ok(backend)
    }

    /// Cell metrics from `#wryme-measure`, a 20-`M` ruler styled like the
    /// terminal. Cheaper and steadier than per-glyph measurement.
    fn measure(doc: &Document) -> (f64, f64) {
        if let Some(el) = doc.get_element_by_id("wryme-measure")
            && let r = el.get_bounding_client_rect()
            && r.width() > 0.0
            && r.height() > 0.0
        {
            return (r.width() / 20.0, r.height());
        }
        (9.0, 18.0)
    }

    fn px(&self, cells: u16) -> f64 {
        cells as f64 * self.char_w
    }

    fn py(&self, cells: u16) -> f64 {
        cells as f64 * self.line_h
    }

    /// Fresh grid, fresh row divs, fresh cursor div.
    fn rebuild(&mut self, cols: u16, rows: u16) {
        self.cols = cols.max(1);
        self.n_rows = rows.max(1);
        let len = self.cols as usize * self.n_rows as usize;
        self.grid = vec![Painted::blank(); len];
        self.dirty = vec![true; self.n_rows as usize];

        while let Some(child) = self.container.first_child() {
            let _ = self.container.remove_child(&child);
        }
        self.rows.clear();
        self.cursor_el = None;
        for _ in 0..self.n_rows {
            let Ok(el) = self.doc.create_element("div") else {
                break;
            };
            let Ok(row) = el.dyn_into::<HtmlElement>() else {
                break;
            };
            row.set_class_name("wme-row");
            let _ = self.container.append_child(&row);
            self.rows.push(row);
        }
        if let Ok(el) = self.doc.create_element("div")
            && let Ok(cur) = el.dyn_into::<HtmlElement>()
        {
            cur.set_class_name("wme-cursor");
            let _ = self.container.append_child(&cur);
            self.cursor_el = Some(cur);
        }
        self.apply_cursor();
    }

    fn apply_cursor(&mut self) {
        let Some(el) = self.cursor_el.clone() else {
            return;
        };
        let style = el.style();
        let _ = style.set_property("display", if self.cursor_visible { "block" } else { "none" });
        let _ = style.set_property("left", &format!("{}px", self.px(self.cursor.x)));
        let _ = style.set_property("top", &format!("{}px", self.py(self.cursor.y)));
    }

    fn blank_range(&mut self, from: usize, to: usize) {
        let to = to.min(self.grid.len());
        if from >= to {
            return;
        }
        for cell in &mut self.grid[from..to] {
            *cell = Painted::blank();
        }
        let cols = self.cols as usize;
        let start_row = from / cols;
        let end_row = (to.saturating_sub(1)) / cols;
        for y in start_row..=end_row.min(self.n_rows as usize - 1) {
            if let Some(d) = self.dirty.get_mut(y) {
                *d = true;
            }
        }
    }

    /// Row HTML: runs of same-styled cells, skipping wide-char continuations
    /// so the row can never drift out of column.
    fn row_html(&self, y: u16) -> String {
        let cols = self.cols as usize;
        let base = y as usize * cols;
        let mut out = String::new();
        let mut i = 0usize;
        while i < cols {
            let cell = &self.grid[base + i];
            if cell.sym.is_empty() {
                i += 1;
                continue;
            }
            let w = UnicodeWidthStr::width(cell.sym.as_str()).max(1);
            let mut run = escape(&cell.sym);
            let mut style = style_attr(cell);
            let mut j = i + w;
            while j < cols {
                let n = &self.grid[base + j];
                if n.sym.is_empty() {
                    j += 1;
                    continue;
                }
                if style_attr(n) != style {
                    break;
                }
                let nw = UnicodeWidthStr::width(n.sym.as_str()).max(1);
                run.push_str(&escape(&n.sym));
                j += nw;
            }
            if style.is_empty() {
                out.push_str(&run);
            } else {
                style.insert_str(0, "style=\"");
                style.push('"');
                out.push_str("<span ");
                out.push_str(&style);
                out.push('>');
                out.push_str(&run);
                out.push_str("</span>");
            }
            i = j;
        }
        out
    }
}

impl Backend for WebBackend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        // Ratatui resizes its buffers (and thus tells us a new size) before
        // this runs, so the grid can follow here without a resize listener.
        let size = self.size()?;
        if size.width != self.cols || size.height != self.n_rows {
            self.rebuild(size.width, size.height);
        }
        for (x, y, cell) in content {
            if x >= self.cols || y >= self.n_rows {
                continue;
            }
            let idx = y as usize * self.cols as usize + x as usize;
            let sym = cell.symbol().to_string();
            let next = Painted {
                sym: sym.clone(),
                fg: cell.fg,
                bg: cell.bg,
                modifier: cell.modifier,
            };
            if self.grid[idx] != next {
                self.grid[idx] = next;
                self.dirty[y as usize] = true;
            }
            // A wide glyph owns the columns after it, whatever the buffer says.
            let w = UnicodeWidthStr::width(sym.as_str()).max(1);
            for k in 1..w {
                let cx = x as usize + k;
                if cx >= self.cols as usize {
                    break;
                }
                let ci = y as usize * self.cols as usize + cx;
                if self.grid[ci].sym.is_empty() {
                    continue;
                }
                self.grid[ci] = Painted {
                    sym: String::new(),
                    fg: cell.fg,
                    bg: cell.bg,
                    modifier: cell.modifier,
                };
                self.dirty[y as usize] = true;
            }
        }
        Ok(())
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.cursor_visible = false;
        self.apply_cursor();
        Ok(())
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.cursor_visible = true;
        self.apply_cursor();
        Ok(())
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        Ok(self.cursor)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        self.cursor = position.into();
        self.apply_cursor();
        Ok(())
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        let (w, h) = (self.cols, self.n_rows);
        self.rebuild(w, h);
        Ok(())
    }

    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        let cols = self.cols as usize;
        let rows = self.n_rows as usize;
        let cur = self.cursor.y as usize * cols + self.cursor.x as usize;
        let total = cols * rows;
        match clear_type {
            ClearType::All => self.clear()?,
            ClearType::AfterCursor => self.blank_range(cur, total),
            ClearType::BeforeCursor => self.blank_range(0, cur.min(total)),
            ClearType::CurrentLine => {
                let start = self.cursor.y as usize * cols;
                self.blank_range(start, start + cols);
            }
            ClearType::UntilNewLine => {
                let start = self.cursor.y as usize * cols;
                self.blank_range(cur, start + cols);
            }
        }
        Ok(())
    }

    fn size(&self) -> Result<Size, Self::Error> {
        let rect = self.container.get_bounding_client_rect();
        let width = ((rect.width() / self.char_w).floor() as u16).max(1);
        let height = ((rect.height() / self.line_h).floor() as u16).max(1);
        Ok(Size { width, height })
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        let size = self.size()?;
        Ok(WindowSize {
            columns_rows: size,
            pixels: Size {
                width: (size.width as f64 * self.char_w) as u16,
                height: (size.height as f64 * self.line_h) as u16,
            },
        })
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        for y in 0..self.n_rows {
            let dirty = self.dirty.get(y as usize).copied().unwrap_or(false);
            if !dirty {
                continue;
            }
            let html = self.row_html(y);
            if let Some(row) = self.rows.get(y as usize) {
                row.set_inner_html(&html);
            }
            if let Some(d) = self.dirty.get_mut(y as usize) {
                *d = false;
            }
        }
        self.apply_cursor();
        Ok(())
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// Inline style for one cell. `Reset` colors inherit from the page theme.
fn style_attr(c: &Painted) -> String {
    let (mut fg, mut bg) = (c.fg, c.bg);
    if c.modifier.contains(Modifier::REVERSED) {
        std::mem::swap(&mut fg, &mut bg);
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(css) = color_css(fg) {
        parts.push(format!("color:{css}"));
    }
    if let Some(css) = color_css(bg) {
        parts.push(format!("background:{css}"));
    }
    if c.modifier.contains(Modifier::BOLD) {
        parts.push("font-weight:700".into());
    }
    if c.modifier.contains(Modifier::ITALIC) {
        parts.push("font-style:italic".into());
    }
    let mut deco: Vec<&str> = Vec::new();
    if c.modifier.contains(Modifier::UNDERLINED) {
        deco.push("underline");
    }
    if c.modifier.contains(Modifier::CROSSED_OUT) {
        deco.push("line-through");
    }
    if !deco.is_empty() {
        parts.push(format!("text-decoration:{}", deco.join(" ")));
    }
    if c.modifier.contains(Modifier::DIM) {
        parts.push("opacity:0.7".into());
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("{};", parts.join(";"))
    }
}

fn color_css(c: Color) -> Option<String> {
    let hex = match c {
        Color::Reset => return None,
        Color::Black => "#000000",
        Color::Red => "#cd3131",
        Color::Green => "#0dbc79",
        Color::Yellow => "#e5e510",
        Color::Blue => "#2472c8",
        Color::Magenta => "#bc3fbc",
        Color::Cyan => "#11a8cd",
        Color::Gray => "#e5e5e5",
        Color::DarkGray => "#666666",
        Color::LightRed => "#f14c4c",
        Color::LightGreen => "#23d18b",
        Color::LightYellow => "#f5f543",
        Color::LightBlue => "#3b8eea",
        Color::LightMagenta => "#d670d6",
        Color::LightCyan => "#29b8db",
        Color::White => "#ffffff",
        Color::Indexed(n) => return Some(ansi256(n)),
        Color::Rgb(r, g, b) => return Some(format!("rgb({r},{g},{b})")),
    };
    Some(hex.to_string())
}

fn ansi256(n: u8) -> String {
    const NAMED: [&str; 16] = [
        "#000000", "#cd3131", "#0dbc79", "#e5e510", "#2472c8", "#bc3fbc", "#11a8cd", "#e5e5e5",
        "#666666", "#f14c4c", "#23d18b", "#f5f543", "#3b8eea", "#d670d6", "#29b8db", "#ffffff",
    ];
    match n {
        0..=15 => NAMED[n as usize].to_string(),
        16..=231 => {
            let i = n - 16;
            let steps = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
            let (r, g, b) = (steps(i / 36), steps((i % 36) / 6), steps(i % 6));
            format!("rgb({r},{g},{b})")
        }
        _ => {
            let v = 8 + 10 * (n - 232);
            format!("rgb({v},{v},{v})")
        }
    }
}
