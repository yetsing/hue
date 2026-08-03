#![allow(unused)]

use crate::textarea::TextArea;
use std::any::Any;

// 定义编辑命令 Trait
pub trait EditCommand {
    fn execute(&mut self, text_area: &mut TextArea);
    fn undo(&mut self, text_area: &mut TextArea);
    fn try_merge(&mut self, other: &dyn EditCommand) -> bool;
    fn as_any(&self) -> &dyn Any;
}

pub struct InsertTextCmd {
    row: usize,
    col: usize,
    text: String, // 插入的内容
}

impl InsertTextCmd {
    pub fn new(row: usize, col: usize, text: String) -> Self {
        InsertTextCmd { row, col, text }
    }
}

impl EditCommand for InsertTextCmd {
    fn execute(&mut self, text_area: &mut TextArea) {
        text_area.insert_text_at(self.row, self.col, &self.text);
    }
    fn undo(&mut self, text_area: &mut TextArea) {
        text_area.delete_chars_at(self.row, self.col, self.text.chars().count());
    }
    fn try_merge(&mut self, other: &dyn EditCommand) -> bool {
        if let Some(other) = other.as_any().downcast_ref::<InsertTextCmd>() {
            if self.row == other.row && self.col + self.text.chars().count() == other.col {
                self.text.push_str(&other.text);
                return true;
            }
        }
        false
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct InsertNewlineCmd {
    row: usize,
    col: usize,
}

impl InsertNewlineCmd {
    pub fn new(row: usize, col: usize) -> Self {
        InsertNewlineCmd { row, col }
    }
}

impl EditCommand for InsertNewlineCmd {
    fn execute(&mut self, text_area: &mut TextArea) {
        text_area.insert_newline_at(self.row, self.col);
    }
    fn undo(&mut self, text_area: &mut TextArea) {
        text_area.merge_lines_at(self.row);
    }
    fn try_merge(&mut self, _other: &dyn EditCommand) -> bool {
        false // 插入新行的命令不与其他命令合并
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct DeleteNewlineCmd {
    row: usize,
    col: usize,
}

impl DeleteNewlineCmd {
    pub fn new(row: usize, col: usize) -> Self {
        DeleteNewlineCmd { row, col }
    }
}

impl EditCommand for DeleteNewlineCmd {
    fn execute(&mut self, text_area: &mut TextArea) {
        text_area.merge_lines_at(self.row);
    }
    fn undo(&mut self, text_area: &mut TextArea) {
        text_area.insert_newline_at(self.row, self.col);
    }
    fn try_merge(&mut self, _other: &dyn EditCommand) -> bool {
        false // 删除新行的命令不与其他命令合并
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct DeleteTextCmd {
    row: usize,
    col: usize,
    text: String, // 被删除的内容
}

impl DeleteTextCmd {
    pub fn new(row: usize, col: usize, text: String) -> Self {
        DeleteTextCmd { row, col, text }
    }
}

impl EditCommand for DeleteTextCmd {
    fn execute(&mut self, text_area: &mut TextArea) {
        text_area.delete_chars_at(self.row, self.col, self.text.chars().count());
    }
    fn undo(&mut self, text_area: &mut TextArea) {
        text_area.insert_text_at(self.row, self.col, &self.text);
        text_area.goto_cursor(self.row, self.col);
    }
    fn try_merge(&mut self, _other: &dyn EditCommand) -> bool {
        false // 删除命令不与其他命令合并
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// 在 Insert 模式下删除文本的命令，和 DeleteTextCmd 类似，但用于 Insert 模式下的删除操作
pub struct DeleteText1Cmd {
    row: usize,
    col: usize,
    text: String, // 被删除的内容
}

impl DeleteText1Cmd {
    pub fn new(row: usize, col: usize, text: String) -> Self {
        DeleteText1Cmd { row, col, text }
    }
}

impl EditCommand for DeleteText1Cmd {
    fn execute(&mut self, text_area: &mut TextArea) {
        text_area.delete_chars_at(self.row, self.col, self.text.chars().count());
    }
    fn undo(&mut self, text_area: &mut TextArea) {
        text_area.insert_text_at(self.row, self.col, &self.text);
        text_area.goto_cursor(self.row, self.col);
    }
    fn try_merge(&mut self, other: &dyn EditCommand) -> bool {
        let other = other.as_any().downcast_ref::<DeleteText1Cmd>();
        if let Some(other) = other {
            if self.row == other.row && self.col + self.text.chars().count() == other.col {
                self.text.push_str(&other.text);
                return true;
            }
            if self.row == other.row && self.col == other.col + other.text.chars().count() {
                self.text = format!("{}{}", other.text, self.text);
                self.col = other.col;
                return true;
            }
        }
        false
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct UndoHistory {
    // 每个 Vec<Command> 代表一个 Undo Block
    undo_stack: Vec<Vec<Box<dyn EditCommand>>>,
    redo_stack: Vec<Vec<Box<dyn EditCommand>>>,
    current_block: Vec<Box<dyn EditCommand>>, // 当前正在构建的块
}

impl UndoHistory {
    pub fn new() -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            current_block: Vec::new(),
        }
    }
}

impl UndoHistory {
    // 开始新的 Undo Block（如按下 dw 或进入 Insert 模式时调用）
    pub fn start_new_block(&mut self) {
        if !self.current_block.is_empty() {
            self.undo_stack
                .push(std::mem::take(&mut self.current_block));
            self.redo_stack.clear(); // 新操作清空 redo 栈
        }
    }

    // 记录当前操作到当前块
    pub fn record(&mut self, cmd: Box<dyn EditCommand>) {
        if self.current_block.is_empty() {
            self.current_block.push(cmd);
        } else {
            let last_cmd = self.current_block.last_mut().unwrap();
            if !last_cmd.try_merge(&*cmd) {
                self.current_block.push(cmd);
            }
        }
    }

    // 执行 u 命令
    pub fn undo(&mut self, text_area: &mut TextArea) {
        // 先提交未完成的当前块
        self.start_new_block();

        if let Some(mut block) = self.undo_stack.pop() {
            // 逆序执行块内所有命令
            for cmd in block.iter_mut().rev() {
                cmd.undo(text_area);
            }
            self.redo_stack.push(block);
        }
    }
}
