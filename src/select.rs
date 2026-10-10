pub type Screen = Vec<Vec<String>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub col: u16,
    pub row: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Point,
    pub head: Point,
}

impl Selection {
    pub fn new(anchor: Point) -> Self {
        Self {
            anchor,
            head: anchor,
        }
    }

    pub fn extend(&mut self, head: Point) {
        self.head = head;
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    pub fn bounds(&self) -> (Point, Point) {
        let a = (self.anchor.row, self.anchor.col);
        let h = (self.head.row, self.head.col);
        if a <= h {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    pub fn contains(&self, p: &Point) -> bool {
        let (s, e) = self.bounds();
        (p.row, p.col) >= (s.row, s.col) && (p.row, p.col) <= (e.row, e.col)
    }

    pub fn move_head(&mut self, dc: i16, dr: i16) {
        let col = (self.head.col as i32 + dc as i32).clamp(0, i32::from(u16::MAX)) as u16;
        let row = (self.head.row as i32 + dr as i32).clamp(0, i32::from(u16::MAX)) as u16;
        self.head = Point { col, row };
    }

    pub fn extract(&self, screen: &Screen) -> String {
        let (s, e) = self.bounds();
        let mut out = String::new();
        for row in s.row..=e.row {
            let line = match screen.get(row as usize) {
                Some(l) => l,
                None => continue,
            };
            if row == s.row && row == e.row {
                out.push_str(slice_line(line, s.col, e.col).trim_end());
            } else if row == s.row {
                out.push_str(slice_line(line, s.col, line.len().saturating_sub(1) as u16).trim_end());
            } else if row == e.row {
                out.push_str(slice_line(line, 0, e.col).trim_end());
            } else {
                out.push_str(join_cells(line).trim_end());
            }
            if row != e.row {
                out.push('\n');
            }
        }
        out
    }
}

fn slice_line(line: &[String], from: u16, to: u16) -> String {
    let len = line.len().saturating_sub(1);
    let from = (from as usize).min(len);
    let to = (to as usize).min(len);
    if from > to {
        return String::new();
    }
    join_cells(&line[from..=to])
}

fn join_cells(cells: &[String]) -> String {
    let mut out = String::new();
    for c in cells {
        if !c.is_empty() {
            out.push_str(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(rows: &[&str]) -> Screen {
        rows.iter()
            .map(|r| r.chars().map(|c| c.to_string()).collect())
            .collect()
    }

    #[test]
    fn single_row_partial() {
        let s = Screen::from(vec![vec!["a".into(), "b".into(), "c".into(), "d".into()]]);
        let sel = Selection::new(Point { col: 1, row: 0 });
        let mut sel = sel;
        sel.extend(Point { col: 2, row: 0 });
        assert_eq!(sel.extract(&s), "bc");
    }

    #[test]
    fn multi_row_joins_with_newlines_and_trims_trailing() {
        let s = screen(&["ab  ", "cd  ", "ef  "]);
        let mut sel = Selection::new(Point { col: 0, row: 0 });
        sel.extend(Point { col: 1, row: 2 });
        assert_eq!(sel.extract(&s), "ab\ncd\nef");
    }

    #[test]
    fn reverse_drag_normalizes() {
        let s = screen(&["abcd", "efgh"]);
        let mut sel = Selection::new(Point { col: 2, row: 1 });
        sel.extend(Point { col: 1, row: 0 });
        assert_eq!(sel.bounds().0, Point { col: 1, row: 0 });
        assert_eq!(sel.bounds().1, Point { col: 2, row: 1 });
        assert_eq!(sel.extract(&s), "bcd\nefg");
    }

    #[test]
    fn wide_char_continuations_skipped() {
        let s = Screen::from(vec![vec![
            "a".into(),
            "界".into(),
            String::new(),
            "b".into(),
        ]]);
        let mut sel = Selection::new(Point { col: 0, row: 0 });
        sel.extend(Point { col: 3, row: 0 });
        assert_eq!(sel.extract(&s), "a界b");
    }

    #[test]
    fn empty_selection_extracts_one_cell() {
        let s = screen(&["hello"]);
        let sel = Selection::new(Point { col: 0, row: 0 });
        assert_eq!(sel.extract(&s), "h");
    }

    #[test]
    fn out_of_range_is_inert() {
        let s: Screen = Vec::new();
        let sel = Selection::new(Point { col: 0, row: 99 });
        assert_eq!(sel.extract(&s), "");
    }

    #[test]
    fn contains_respects_bounds() {
        let mut sel = Selection::new(Point { col: 1, row: 1 });
        sel.extend(Point { col: 3, row: 2 });
        assert!(sel.contains(&Point { col: 2, row: 1 }));
        assert!(sel.contains(&Point { col: 3, row: 2 }));
        assert!(!sel.contains(&Point { col: 0, row: 1 }));
        assert!(!sel.contains(&Point { col: 2, row: 3 }));
    }

    #[test]
    fn move_head_clamps_at_zero() {
        let mut sel = Selection::new(Point { col: 0, row: 0 });
        sel.move_head(-5, -5);
        assert_eq!(sel.head, Point { col: 0, row: 0 });
    }
}