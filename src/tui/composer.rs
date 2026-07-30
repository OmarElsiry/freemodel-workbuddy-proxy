use unicode_width::UnicodeWidthChar;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Composer {
    text: Vec<char>,
    cursor: usize,
}

impl Composer {
    pub fn new(value: impl Into<String>) -> Self {
        let text: Vec<char> = value.into().chars().collect();
        let cursor = text.len();
        Self { text, cursor }
    }
    pub fn text(&self) -> String {
        self.text.iter().collect()
    }
    pub fn is_empty(&self) -> bool {
        self.text.iter().all(|c| c.is_whitespace())
    }
    pub fn cursor(&self) -> usize {
        self.cursor
    }
    pub fn clear(&mut self) -> String {
        let old = self.text();
        self.text.clear();
        self.cursor = 0;
        old
    }
    pub fn set(&mut self, value: impl Into<String>) {
        *self = Self::new(value);
    }
    pub fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += 1;
    }
    pub fn insert_str(&mut self, value: &str) {
        for c in value.chars() {
            self.insert(c);
        }
    }
    pub fn newline(&mut self) {
        self.insert('\n');
    }
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.text.remove(self.cursor);
        }
    }
    pub fn delete(&mut self) {
        if self.cursor < self.text.len() {
            self.text.remove(self.cursor);
        }
    }
    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }
    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.text.len());
    }
    pub fn home(&mut self) {
        while self.cursor > 0 && self.text[self.cursor - 1] != '\n' {
            self.cursor -= 1;
        }
    }
    pub fn end(&mut self) {
        while self.cursor < self.text.len() && self.text[self.cursor] != '\n' {
            self.cursor += 1;
        }
    }
    pub fn up(&mut self) {
        self.move_vertical(-1);
    }
    pub fn down(&mut self) {
        self.move_vertical(1);
    }
    fn move_vertical(&mut self, direction: i32) {
        let before: String = self.text[..self.cursor].iter().collect();
        let row = before.chars().filter(|c| *c == '\n').count();
        let col = before.rsplit('\n').next().unwrap_or("").chars().count();
        let lines: Vec<&[char]> = self.text.split(|c| *c == '\n').collect();
        let target = if direction < 0 {
            row.checked_sub(1)
        } else if row + 1 < lines.len() {
            Some(row + 1)
        } else {
            None
        };
        let Some(target) = target else { return };
        self.cursor = lines
            .iter()
            .take(target)
            .map(|line| line.len() + 1)
            .sum::<usize>()
            + col.min(lines[target].len());
    }
    pub fn cursor_screen_position(&self, width: usize) -> (usize, usize) {
        let width = width.max(1);
        let mut row = 0;
        let mut col = 0;
        for c in self.text.iter().take(self.cursor) {
            if *c == '\n' {
                row += 1;
                col = 0;
                continue;
            }
            let w = c.width().unwrap_or(0).max(1);
            if col + w > width {
                row += 1;
                col = 0;
            }
            col += w;
        }
        (row, col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_editing_is_character_safe() {
        let mut c = Composer::new("a🙂b");
        c.left();
        c.backspace();
        assert_eq!(c.text(), "ab");
    }
    #[test]
    fn vertical_movement_preserves_column() {
        let mut c = Composer::new("abcd\nxy\n1234");
        c.home();
        c.up();
        assert_eq!(c.cursor(), 5);
        c.up();
        assert_eq!(c.cursor(), 0);
    }
    #[test]
    fn wrapped_cursor_uses_unicode_width() {
        let c = Composer::new("ab界");
        assert_eq!(c.cursor_screen_position(3), (1, 2));
    }
}
