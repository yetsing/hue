fn char_to_byte_index(s: &str, char_index: usize) -> usize {
    if char_index == 0 {
        return 0;
    }
    s.char_indices()
        .nth(char_index)
        .map_or(s.len(), |(byte_idx, _)| byte_idx)
}

struct Line {
    text: String,
}

impl Line {
    fn new(text: String) -> Self {
        Line { text }
    }

    fn length(&self) -> usize {
        self.text.chars().count()
    }

    fn insert_text(&mut self, text: &str, col: usize) {
        let byte_idx = char_to_byte_index(&self.text, col);
        self.text.insert_str(byte_idx, text);
    }

    fn delete_text(&mut self, col: usize, len: usize) {
        let byte_idx = char_to_byte_index(&self.text, col);
        let end = char_to_byte_index(&self.text, col + len);
        self.text.replace_range(byte_idx..end, "");
    }
}

pub(crate) struct TextArea {
    lines: Vec<Line>,
    cursor_row: usize,
    cursor_col: usize,
    preferred_col: usize, // Preferred column position when moving up/down
}

impl TextArea {
    pub(crate) fn new() -> Self {
        TextArea {
            lines: vec![Line::new(String::new())],
            cursor_row: 0,
            cursor_col: 0,
            preferred_col: 0,
        }
    }

    pub(crate) fn insert_text_at_cursor(&mut self, text: &str) {
        if let Some(line) = self.lines.get_mut(self.cursor_row) {
            line.insert_text(text, self.cursor_col);
            self.cursor_col += text.chars().count();
            self.preferred_col = self.cursor_col; // Update preferred column after insertion
        }
    }

    pub(crate) fn insert_newline_at_cursor(&mut self) {
        if let Some(line) = self.lines.get_mut(self.cursor_row) {
            let remaining_text = line
                .text
                .split_off(char_to_byte_index(&line.text, self.cursor_col));
            self.lines
                .insert(self.cursor_row + 1, Line::new(remaining_text));
            self.cursor_row += 1;
            self.cursor_col = 0;
            self.preferred_col = 0;
        }
    }

    pub(crate) fn delete_char_before_cursor(&mut self) {
        if self.cursor_col == 0 && self.cursor_row == 0 {
            return;
        }

        if self.cursor_col == 0 {
            // Merge with the previous line
            let current_line = self.lines.remove(self.cursor_row);
            if let Some(prev_line) = self.lines.get_mut(self.cursor_row - 1) {
                self.cursor_col = prev_line.length();
                self.preferred_col = self.cursor_col; // Update preferred column after merging
                prev_line.text.push_str(&current_line.text);
                self.cursor_row -= 1;
            }
            return;
        }

        if let Some(line) = self.lines.get_mut(self.cursor_row) {
            line.delete_text(self.cursor_col - 1, 1);
            self.cursor_col -= 1;
            self.preferred_col = self.cursor_col; // Update preferred column after deletion
        }
    }

    pub(crate) fn delete_char_after_cursor(&mut self) {
        let line_count = self.lines.len();
        if self.cursor_row >= line_count {
            return;
        }

        let line_len = self.lines[self.cursor_row].length();
        if self.cursor_col < line_len {
            if let Some(line) = self.lines.get_mut(self.cursor_row) {
                line.delete_text(self.cursor_col, 1);
            }
        } else if self.cursor_col == line_len && self.cursor_row + 1 < line_count {
            // Merge with the next line
            let next_line = self.lines.remove(self.cursor_row + 1);
            if let Some(line) = self.lines.get_mut(self.cursor_row) {
                line.text.push_str(&next_line.text);
            }
        }
    }

    pub(crate) fn delete_line_at_cursor(&mut self) {
        if self.cursor_row < self.lines.len() {
            self.lines.remove(self.cursor_row);
            if self.cursor_row >= self.lines.len() && self.cursor_row > 0 {
                self.cursor_row -= 1;
            }
            self.cursor_col = usize::min(self.cursor_col, self.lines[self.cursor_row].length().saturating_sub(1));
            self.preferred_col = self.cursor_col; // Update preferred column after deletion
        }
    }

    pub(crate) fn string(&self) -> String {
        self.lines
            .iter()
            .map(|line| line.text.clone())
            .collect::<Vec<String>>()
            .join("\n")
    }

    pub(crate) fn cursor_prefix_string(&self) -> String {
        if let Some(line) = self.lines.get(self.cursor_row) {
            line.text.chars().take(self.cursor_col).collect()
        } else {
            String::new()
        }
    }

    pub(crate) fn lines_before_cursor_string(&self) -> String {
        self.lines
            .iter()
            .take(self.cursor_row)
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub(crate) fn trailing_empty_lines_before_cursor(&self) -> usize {
        self.lines
            .iter()
            .take(self.cursor_row)
            .rev()
            .take_while(|line| line.text.is_empty())
            .count()
    }
}

// cursor movement methods
impl TextArea {
    pub(crate) fn cursor_position(&self) -> (usize, usize) {
        (self.cursor_row, self.cursor_col)
    }

    pub(crate) fn clamp_cursor(&mut self) {
        if self.cursor_row >= self.lines.len() {
            self.cursor_row = self.lines.len().saturating_sub(1);
        }
        if self.cursor_col > self.lines[self.cursor_row].length() {
            self.cursor_col = self.lines[self.cursor_row].length();
        }
    }

    pub(crate) fn move_left_cursor(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
            self.preferred_col = self.cursor_col; // Update preferred column when moving left
        }
    }

    pub(crate) fn move_right_cursor(&mut self) {
        if self.cursor_row < self.lines.len() {
            let line_len = self.lines[self.cursor_row].length();
            if line_len > 1 && self.cursor_col < line_len - 1 {
                self.cursor_col += 1;
                self.preferred_col = self.cursor_col; // Update preferred column when moving right
            }
        }
    }

    pub(crate) fn move_up_cursor(&mut self) {
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            let line_len = self.lines[self.cursor_row].length();
            if line_len < 2 {
                self.cursor_col = 0;
            } else {
                self.cursor_col = usize::min(self.preferred_col, line_len - 1);
            }
        }
    }

    pub(crate) fn move_down_cursor(&mut self) {
        if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            let line_len = self.lines[self.cursor_row].length();
            if line_len < 2 {
                self.cursor_col = 0;
            } else {
                self.cursor_col = usize::min(self.preferred_col, line_len - 1);
            }
        }
    }
}
