// A single-line, grapheme-aware input field. Lives at the top of the screen.
// Multiline submission can come later — for now, Enter submits, Shift-Enter
// (if your terminal sends it) inserts a literal newline character.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub struct Input {
    pub text: String,
    /// Caret position in grapheme clusters, not bytes.
    pub col: usize,
}

impl Input {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            col: 0,
        }
    }

    pub fn grapheme_count(&self) -> usize {
        self.text.graphemes(true).count()
    }

    fn grapheme_byte(&self, idx: usize) -> usize {
        let mut count = 0;
        for (b, _) in self.text.grapheme_indices(true) {
            if count == idx {
                return b;
            }
            count += 1;
        }
        self.text.len()
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

    /// Kill from caret to end of line (Ctrl-K).
    pub fn kill_to_end(&mut self) {
        let here = self.grapheme_byte(self.col);
        self.text.truncate(here);
    }

    /// Kill from start of line to caret (Ctrl-U).
    pub fn kill_to_start(&mut self) {
        let here = self.grapheme_byte(self.col);
        self.text.replace_range(..here, "");
        self.col = 0;
    }

    /// Delete the previous word (Ctrl-W). Eats trailing whitespace first,
    /// then eats one run of non-whitespace.
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

    /// Display-column position of the caret. Used by the renderer to place
    /// the terminal cursor.
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
}
