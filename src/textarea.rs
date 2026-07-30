use chrono::offset;

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
    frozen_pos: (usize, usize),
}

impl TextArea {
    pub(crate) fn new() -> Self {
        TextArea {
            lines: vec![Line::new(String::new())],
            cursor_row: 0,
            cursor_col: 0,
            preferred_col: 0,
            frozen_pos: (0, 0),
        }
    }

    pub(crate) fn from_string(s: &str) -> Self {
        let mut lines: Vec<Line> = s.lines().map(|line| Line::new(line.to_string())).collect();
        if lines.is_empty() {
            lines.push(Line::new(String::new()));
        }
        TextArea {
            lines,
            cursor_row: 0,
            cursor_col: 0,
            preferred_col: 0,
            frozen_pos: (0, 0),
        }
    }

    pub(crate) fn from_prompt(s: &str) -> Self {
        let lines: Vec<Line> = s.lines().map(|line| Line::new(line.to_string())).collect();
        let row = lines.len() - 1;
        let col = lines.last().map_or(0, |line| line.length());
        TextArea {
            lines,
            cursor_row: row,
            cursor_col: col,
            preferred_col: col,
            frozen_pos: (row, col),
        }
    }

    /// Inserts text at the current cursor position and updates the cursor position accordingly.
    /// Text can be multiple characters, but it does not handle newlines.
    pub(crate) fn insert_text_at_cursor(&mut self, text: &str) {
        self.insert_text_at(self.cursor_col, text);
    }

    pub(crate) fn insert_text_at(&mut self, col: usize, text: &str) {
        if let Some(line) = self.lines.get_mut(self.cursor_row) {
            line.insert_text(text, col);
            self.cursor_col = col + text.chars().count();
            self.preferred_col = self.cursor_col; // Update preferred column after insertion
        }
    }

    pub(crate) fn insert_multiline_text_above_cursor(&mut self, text: &str) {
        let lines: Vec<Line> = text
            .lines()
            .map(|line| Line::new(line.to_string()))
            .collect();
        let insert_pos = self.cursor_row;
        self.lines.splice(insert_pos..insert_pos, lines);
        self.cursor_col = 0;
        self.preferred_col = 0; // Update preferred column after insertion
    }

    pub(crate) fn insert_multiline_text_below_cursor(&mut self, text: &str) {
        let lines: Vec<Line> = text
            .lines()
            .map(|line| Line::new(line.to_string()))
            .collect();
        let insert_pos = self.cursor_row + 1;
        self.lines.splice(insert_pos..insert_pos, lines);
        self.cursor_row += 1;
        self.cursor_col = 0;
        self.preferred_col = 0; // Update preferred column after insertion
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

    pub(crate) fn delete_char_before_cursor(&mut self, can_delete_newline: bool) {
        if (self.cursor_row, self.cursor_col) <= self.frozen_pos {
            return;
        }

        if can_delete_newline && self.cursor_col == 0 {
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

    pub(crate) fn delete_char_after_cursor(&mut self, can_delete_newline: bool) {
        let line_count = self.lines.len();
        if self.cursor_row >= line_count {
            return;
        }

        let line_len = self.lines[self.cursor_row].length();
        if self.cursor_col < line_len {
            if let Some(line) = self.lines.get_mut(self.cursor_row) {
                line.delete_text(self.cursor_col, 1);
            }
        } else if can_delete_newline
            && self.cursor_col == line_len
            && self.cursor_row + 1 < line_count
        {
            // Merge with the next line
            let next_line = self.lines.remove(self.cursor_row + 1);
            if let Some(line) = self.lines.get_mut(self.cursor_row) {
                line.text.push_str(&next_line.text);
            }
        }
    }

    pub(crate) fn delete_lines_at_cursor(&mut self, n: usize) {
        if n >= self.lines.len() {
            self.lines.clear();
            self.lines.push(Line::new(String::new()));
            self.cursor_row = 0;
            self.cursor_col = 0;
            self.preferred_col = 0; // Update preferred column after deletion
            return;
        }
        if self.cursor_row < self.lines.len() {
            let actual_n = usize::min(n, self.lines.len() - self.cursor_row);
            for _ in 0..actual_n {
                self.lines.remove(self.cursor_row);
            }
            self.clamp1_cursor();
            self.preferred_col = self.cursor_col; // Update preferred column after deletion
        }
    }

    pub(crate) fn delete_to_start_of_line(&mut self) {
        if let Some(line) = self.lines.get_mut(self.cursor_row) {
            if self.cursor_col > 0 {
                line.delete_text(0, self.cursor_col);
                self.cursor_col = 0;
                self.preferred_col = 0; // Update preferred column after deletion
            }
        }
    }

    pub(crate) fn delete_to_end_of_line(&mut self) {
        if let Some(line) = self.lines.get_mut(self.cursor_row) {
            let line_len = line.length();
            if self.cursor_col < line_len {
                line.delete_text(self.cursor_col, line_len - self.cursor_col);
            }
            self.clamp1_cursor();
        }
    }

    pub(crate) fn new_line_above_cursor(&mut self) {
        if self.cursor_row < self.lines.len() {
            let new_line = Line::new(String::new());
            self.lines.insert(self.cursor_row, new_line);
            self.cursor_col = 0;
            self.preferred_col = 0; // Update preferred column after inserting a new line
        }
    }

    pub(crate) fn new_line_below_cursor(&mut self) {
        if self.cursor_row < self.lines.len() {
            let new_line = Line::new(String::new());
            self.lines.insert(self.cursor_row + 1, new_line);
            self.cursor_row += 1;
            self.cursor_col = 0;
            self.preferred_col = 0; // Update preferred column after inserting a new line
        }
    }

    pub(crate) fn append_line(&mut self, text: &str) {
        self.lines.push(Line::new(text.to_string()));
    }

    pub(crate) fn string(&self) -> String {
        self.lines
            .iter()
            .map(|line| line.text.clone())
            .collect::<Vec<String>>()
            .join("\n")
    }

    pub(crate) fn string_with_row_offset(&self, row_offset: usize) -> String {
        self.lines
            .iter()
            .skip(row_offset)
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

    pub(crate) fn measure_lines_before_cursor_string(&self, row_offset: usize) -> String {
        self.lines
            .iter()
            .skip(row_offset)
            .take(self.cursor_row.saturating_sub(row_offset))
            .map(|line| {
                if line.text.is_empty() {
                    "Mg" // 空行替换为占位符
                } else {
                    line.text.as_str()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub(crate) fn lines_with(&self, offset: usize, count: usize) -> String {
        self.lines
            .iter()
            .skip(offset)
            .take(count)
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].length() == 0
    }

    pub(crate) fn line_count(&self) -> usize {
        self.lines.len()
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

    pub(crate) fn clamp1_cursor(&mut self) {
        if self.cursor_row >= self.lines.len() {
            self.cursor_row = self.lines.len().saturating_sub(1);
        }
        let length = self.lines[self.cursor_row].length();
        let max_col = length.saturating_sub(1);
        if self.cursor_col > max_col {
            self.cursor_col = max_col;
        }
    }

    pub(crate) fn move_left_cursor(&mut self) {
        if self.cursor_col > self.frozen_pos.1 {
            self.cursor_col -= 1;
            self.preferred_col = self.cursor_col; // Update preferred column when moving left
        }
    }

    pub(crate) fn move_right1_cursor(&mut self) {
        if self.cursor_row < self.lines.len() {
            let line_len = self.lines[self.cursor_row].length();
            if line_len > 1 && self.cursor_col < line_len - 1 {
                self.cursor_col += 1;
                self.preferred_col = self.cursor_col; // Update preferred column when moving right
            }
        }
    }

    pub(crate) fn move_right_cursor(&mut self) {
        if self.cursor_row < self.lines.len() {
            let line_len = self.lines[self.cursor_row].length();
            if line_len > 0 && self.cursor_col < line_len {
                self.cursor_col += 1;
                self.preferred_col = self.cursor_col; // Update preferred column when moving right
            }
        }
    }

    pub(crate) fn move_up_cursor(&mut self) {
        if self.cursor_row > self.frozen_pos.0 {
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

    pub(crate) fn goto_cursor(&mut self, row: usize, col: usize) {
        self.cursor_row = usize::min(row, self.lines.len().saturating_sub(1));
        let line_len = self.lines[self.cursor_row].length();
        self.cursor_col = usize::min(col, line_len.saturating_sub(1));
        self.preferred_col = self.cursor_col; // Update preferred column when going to a specific position
    }

    pub(crate) fn first_non_blank_of_line_cursor(&mut self) {
        if self.cursor_row < self.lines.len() {
            let line = &self.lines[self.cursor_row].text;
            let first_non_blank_col = line.chars().position(|c| !c.is_whitespace()).unwrap_or(0);
            self.cursor_col = first_non_blank_col;
            self.preferred_col = self.cursor_col; // Update preferred column when moving to first non-blank
        }
    }

    pub(crate) fn start_of_line_cursor(&mut self) {
        self.cursor_col = 0;
        self.preferred_col = self.cursor_col; // Update preferred column when moving to start of line
    }

    pub(crate) fn end_of_line_cursor(&mut self) {
        if self.cursor_row < self.lines.len() {
            let line_len = self.lines[self.cursor_row].length();
            self.cursor_col = line_len;
            self.preferred_col = self.cursor_col; // Update preferred column when moving to end of line
        }
    }

    pub(crate) fn end_offset_of_line_cursor(&mut self, offset: usize) {
        if self.cursor_row < self.lines.len() {
            let line_len = self.lines[self.cursor_row].length();
            self.cursor_col = line_len.saturating_sub(offset);
            self.preferred_col = self.cursor_col; // Update preferred column when moving to end of line
        }
    }
}
