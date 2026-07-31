use std::ops::Range;
use unicode_width::UnicodeWidthChar;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Composer {
    text: Vec<char>,
    cursor: usize,
    selection_anchor: Option<usize>,
    preferred_column: Option<usize>,
}

impl Composer {
    pub fn new(value: impl Into<String>) -> Self {
        let text: Vec<char> = value.into().chars().collect();
        let cursor = text.len();
        Self {
            text,
            cursor,
            selection_anchor: None,
            preferred_column: None,
        }
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
    pub fn selection_range(&self) -> Option<Range<usize>> {
        let anchor = self.selection_anchor?;
        if anchor == self.cursor {
            None
        } else {
            Some(anchor.min(self.cursor)..anchor.max(self.cursor))
        }
    }
    pub fn selected_text(&self) -> Option<String> {
        self.selection_range()
            .map(|range| self.text[range].iter().collect())
    }
    pub fn clear(&mut self) -> String {
        let old = self.text();
        self.text.clear();
        self.cursor = 0;
        self.selection_anchor = None;
        self.preferred_column = None;
        old
    }
    pub fn set(&mut self, value: impl Into<String>) {
        *self = Self::new(value);
    }
    pub fn select_all(&mut self) {
        self.selection_anchor = Some(0);
        self.cursor = self.text.len();
        self.preferred_column = None;
    }
    pub fn insert(&mut self, c: char) {
        self.delete_selection();
        self.text.insert(self.cursor, c);
        self.cursor += 1;
        self.preferred_column = None;
    }
    pub fn insert_str(&mut self, value: &str) {
        self.delete_selection();
        let inserted: Vec<char> = value.chars().collect();
        let count = inserted.len();
        self.text.splice(self.cursor..self.cursor, inserted);
        self.cursor += count;
        self.preferred_column = None;
    }
    pub fn newline(&mut self) {
        self.insert('\n');
    }
    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor > 0 {
            self.cursor -= 1;
            self.text.remove(self.cursor);
        }
        self.preferred_column = None;
    }
    pub fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor < self.text.len() {
            self.text.remove(self.cursor);
        }
        self.preferred_column = None;
    }
    pub fn delete_selection(&mut self) -> bool {
        let Some(range) = self.selection_range() else {
            self.selection_anchor = None;
            return false;
        };
        self.cursor = range.start;
        self.text.drain(range);
        self.selection_anchor = None;
        self.preferred_column = None;
        true
    }
    pub fn left(&mut self, select: bool) {
        self.preferred_column = None;
        if !select && let Some(range) = self.selection_range() {
            self.cursor = range.start;
            self.selection_anchor = None;
            return;
        }
        self.prepare_selection(select);
        self.cursor = self.cursor.saturating_sub(1);
        self.finish_selection(select);
    }
    pub fn right(&mut self, select: bool) {
        self.preferred_column = None;
        if !select && let Some(range) = self.selection_range() {
            self.cursor = range.end;
            self.selection_anchor = None;
            return;
        }
        self.prepare_selection(select);
        self.cursor = (self.cursor + 1).min(self.text.len());
        self.finish_selection(select);
    }
    pub fn home(&mut self, select: bool) {
        self.preferred_column = None;
        self.prepare_selection(select);
        while self.cursor > 0 && self.text[self.cursor - 1] != '\n' {
            self.cursor -= 1;
        }
        self.finish_selection(select);
    }
    pub fn end(&mut self, select: bool) {
        self.preferred_column = None;
        self.prepare_selection(select);
        while self.cursor < self.text.len() && self.text[self.cursor] != '\n' {
            self.cursor += 1;
        }
        self.finish_selection(select);
    }
    pub fn up(&mut self, width: usize, select: bool) -> bool {
        self.move_vertical(-1, width, select)
    }
    pub fn down(&mut self, width: usize, select: bool) -> bool {
        self.move_vertical(1, width, select)
    }
    fn move_vertical(&mut self, direction: i32, width: usize, select: bool) -> bool {
        let positions = self.cursor_positions(width);
        let (row, col) = positions[self.cursor];
        let target_row = if direction < 0 {
            row.checked_sub(1)
        } else {
            Some(row + 1).filter(|target| positions.iter().any(|(r, _)| r == target))
        };
        let Some(target_row) = target_row else {
            return false;
        };
        let preferred = self.preferred_column.unwrap_or(col);
        let Some((target, _)) = positions
            .iter()
            .enumerate()
            .filter(|(_, (candidate_row, _))| *candidate_row == target_row)
            .min_by_key(|(_, (_, candidate_col))| candidate_col.abs_diff(preferred))
        else {
            return false;
        };
        self.prepare_selection(select);
        self.cursor = target;
        self.finish_selection(select);
        self.preferred_column = Some(preferred);
        true
    }
    pub fn cursor_screen_position(&self, width: usize) -> (usize, usize) {
        self.cursor_positions(width)[self.cursor]
    }
    pub fn visual_row_count(&self, width: usize) -> usize {
        self.cursor_positions(width)
            .into_iter()
            .map(|(row, _)| row)
            .max()
            .unwrap_or(0)
            + 1
    }
    fn cursor_positions(&self, width: usize) -> Vec<(usize, usize)> {
        let width = width.max(1);
        let mut positions = Vec::with_capacity(self.text.len() + 1);
        let mut row = 0;
        let mut col = 0;
        positions.push((row, col));
        for c in &self.text {
            if *c == '\n' {
                row += 1;
                col = 0;
                positions.push((row, col));
                continue;
            }
            let char_width = c.width().unwrap_or(0).max(1).min(width);
            if col + char_width > width {
                row += 1;
                col = 0;
            }
            col += char_width;
            if col >= width {
                row += 1;
                col = 0;
            }
            positions.push((row, col));
        }
        positions
    }
    fn prepare_selection(&mut self, select: bool) {
        if select {
            self.selection_anchor.get_or_insert(self.cursor);
        } else {
            self.selection_anchor = None;
        }
    }
    fn finish_selection(&mut self, select: bool) {
        if !select || self.selection_anchor == Some(self.cursor) {
            self.selection_anchor = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_editing_is_character_safe() {
        let mut composer = Composer::new("a🙂b");
        composer.left(false);
        composer.backspace();
        assert_eq!(composer.text(), "ab");
    }

    #[test]
    fn selection_replaces_and_deletes_unicode_text() {
        let mut composer = Composer::new("a界🙂z");
        composer.left(true);
        composer.left(true);
        assert_eq!(composer.selected_text().as_deref(), Some("🙂z"));
        composer.insert_str("?");
        assert_eq!(composer.text(), "a界?");
        composer.select_all();
        assert_eq!(composer.selected_text().as_deref(), Some("a界?"));
        composer.delete();
        assert!(composer.text().is_empty());
    }

    #[test]
    fn horizontal_move_collapses_selection() {
        let mut composer = Composer::new("abcd");
        composer.left(true);
        composer.left(true);
        composer.left(false);
        assert_eq!(composer.cursor(), 2);
        assert!(composer.selection_range().is_none());
        composer.right(true);
        composer.right(false);
        assert_eq!(composer.cursor(), 3);
        assert!(composer.selection_range().is_none());
    }

    #[test]
    fn vertical_movement_preserves_column() {
        let mut composer = Composer::new("abcd\nxy\n1234");
        composer.home(false);
        assert!(composer.up(20, false));
        assert_eq!(composer.cursor(), 5);
        assert!(composer.up(20, false));
        assert_eq!(composer.cursor(), 0);
        assert!(!composer.up(20, false));
    }

    #[test]
    fn vertical_movement_follows_soft_wrapping() {
        let mut composer = Composer::new("abcdef");
        assert_eq!(composer.cursor_screen_position(3), (2, 0));
        assert!(composer.up(3, false));
        assert_eq!(composer.cursor(), 3);
        assert!(composer.up(3, false));
        assert_eq!(composer.cursor(), 0);
        assert!(!composer.up(3, false));
        assert_eq!(composer.visual_row_count(3), 3);
    }

    #[test]
    fn wrapped_cursor_uses_unicode_width() {
        let composer = Composer::new("ab界");
        assert_eq!(composer.cursor_screen_position(3), (1, 2));
    }
}
