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
