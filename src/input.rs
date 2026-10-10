use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Default)]
pub struct Input {
    pub text: String,
    pub col: usize,
}

impl Input {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn grapheme_count(&self) -> usize {
        self.text.graphemes(true).count()
    }

    fn grapheme_byte(&self, idx: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .nth(idx)
            .map_or(self.text.len(), |(b, _)| b)
    }

    pub fn insert_char(&mut self, c: char) {
        let mut tmp = [0u8; 4];
        let s: &str = c.encode_utf8(&mut tmp);
        self.insert_str(s);
    }

    pub fn insert_str(&mut self, s: &str) {
        let b = self.grapheme_byte(self.col);
        self.text.insert_str(b, s);
        self.col += s.graphemes(true).count();
    }

    pub fn insert_paste(&mut self, s: &str) {
        let mut clean = String::with_capacity(s.len());
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    clean.push(' ');
                }
                '\n' | '\t' => clean.push(' '),
                c if c.is_control() => {}
                c => clean.push(c),
            }
        }
        self.insert_str(&clean);
    }

    pub fn backspace(&mut self) {
        if self.col == 0 {
            return;
        }
        let prev = self.grapheme_byte(self.col - 1);
        let here = self.grapheme_byte(self.col);
        self.text.replace_range(prev..here, "");
        self.col -= 1;
    }

    pub fn delete_forward(&mut self) {
        if self.col >= self.grapheme_count() {
            return;
        }
        let here = self.grapheme_byte(self.col);
        let next = self.grapheme_byte(self.col + 1);
        self.text.replace_range(here..next, "");
    }

    pub fn move_left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        }
    }

    pub fn move_right(&mut self) {
        if self.col < self.grapheme_count() {
            self.col += 1;
        }
    }

    pub fn home(&mut self) {
        self.col = 0;
    }

    pub fn end(&mut self) {
        self.col = self.grapheme_count();
    }

    pub fn kill_to_end(&mut self) {
        let here = self.grapheme_byte(self.col);
        self.text.truncate(here);
    }

    pub fn kill_to_start(&mut self) {
        let here = self.grapheme_byte(self.col);
        self.text.replace_range(..here, "");
        self.col = 0;
    }

    pub fn kill_prev_word(&mut self) {
        let graphemes: Vec<&str> = self.text.graphemes(true).collect();
        let mut target = self.col;
        while target > 0 && graphemes[target - 1].chars().all(|c| c.is_whitespace()) {
            target -= 1;
        }
        while target > 0 && graphemes[target - 1].chars().any(|c| !c.is_whitespace()) {
            target -= 1;
        }
        while self.col > target {
            self.backspace();
        }
    }

    pub fn display_col(&self) -> u16 {
        let width: usize = self
            .text
            .graphemes(true)
            .take(self.col)
            .map(|g| UnicodeWidthStr::width(g).max(1))
            .sum();
        width.min(u16::MAX as usize) as u16
    }

    pub fn take(&mut self) -> String {
        let out = std::mem::take(&mut self.text);
        self.col = 0;
        out
    }

    pub fn scroll_offset(&self, visible_width: usize) -> usize {
        let caret = self.display_col() as usize;
        caret.saturating_sub(visible_width.saturating_sub(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_typing_and_backspace() {
        let mut i = Input::new();
        for c in "hello".chars() {
            i.insert_char(c);
        }
        assert_eq!(i.text, "hello");
        assert_eq!(i.col, 5);
        i.backspace();
        assert_eq!(i.text, "hell");
        assert_eq!(i.col, 4);
    }

    #[test]
    fn caret_clamps_at_ends() {
        let mut i = Input::new();
        i.insert_str("abc");
        i.move_right();
        i.move_right();
        assert_eq!(i.col, 3);
        i.home();
        i.move_left();
        assert_eq!(i.col, 0);
    }

    #[test]
    fn kill_lines() {
        let mut i = Input::new();
        i.insert_str("alpha beta");
        i.col = 5;
        i.kill_to_end();
        assert_eq!(i.text, "alpha");
        i.kill_to_start();
        assert_eq!(i.text, "");
    }

    #[test]
    fn take_resets() {
        let mut i = Input::new();
        i.insert_str("ship it");
        let out = i.take();
        assert_eq!(out, "ship it");
        assert_eq!(i.text, "");
        assert_eq!(i.col, 0);
    }

    #[test]
    fn scroll_keeps_caret_at_right_edge() {
        let mut i = Input::new();
        i.insert_str("a");
        assert_eq!(i.scroll_offset(10), 0);
        i.insert_str("bcdefghijklmnop");
        assert_eq!(i.scroll_offset(10), 7);
        i.home();
        assert_eq!(i.scroll_offset(10), 0);
        i.end();
        assert_eq!(i.scroll_offset(10), 7);
    }

    #[test]
    fn paste_inserts_bulk_text() {
        let mut i = Input::new();
        i.insert_paste("hello world");
        assert_eq!(i.text, "hello world");
        assert_eq!(i.col, 11);
    }

    #[test]
    fn paste_flattens_newlines_and_tabs() {
        let mut i = Input::new();
        i.insert_paste("a\r\nb\tc\nd");
        assert_eq!(i.text, "a b c d");
        assert_eq!(i.col, 7);
    }

    #[test]
    fn paste_at_middle_keeps_caret_after_insert() {
        let mut i = Input::new();
        i.insert_str("ab");
        i.home();
        i.move_right();
        i.insert_paste("XY");
        assert_eq!(i.text, "aXYb");
        assert_eq!(i.col, 3);
    }

    #[test]
    fn paste_drops_other_controls() {
        let mut i = Input::new();
        i.insert_paste("a\u{7}b");
        assert_eq!(i.text, "ab");
    }
}
