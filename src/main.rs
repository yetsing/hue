#[path = "ctx.rs"]
mod ctx;
mod fileexplorer;
mod textarea;
mod undo;

use core::panic;
use ctx::Ctx;
use glyph_brush::OwnedSection;
use glyph_brush::ab_glyph::FontRef;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::{fmt, println};
use wgpu_text::glyph_brush::{BuiltInLineBreaker, Layout, Section, Text};
use wgpu_text::{BrushBuilder, TextBrush};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{Ime, KeyEvent, MouseScrollDelta};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::Window;
use winit::{
    event::{ElementState, WindowEvent},
    event_loop::{self},
};

const CARET_BLINK_DELAY_MS: u128 = 500;
const CARET_BLINK_PERIOD_MS: u128 = 500;
const CARET_VERTEX_FLOATS: usize = 6 * 6;
const CARET_VERTEX_BUFFER_SIZE: u64 = (CARET_VERTEX_FLOATS * std::mem::size_of::<f32>()) as u64;
const CARET_HEIGHT_RATIO: f32 = 0.50;
const CARET_TOP_OFFSET_RATIO: f32 = 0.06;
const CARET_WGSL: &str = r#"
struct VsIn {
    @location(0) pos: vec2<f32>,
    @location(1) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.pos = vec4<f32>(in.pos, 0.0, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum VimMode {
    #[default]
    Directory,
    Command,
    CommandLine,
    Insert,
    Visual,
}

impl fmt::Display for VimMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            VimMode::Directory => "Directory",
            VimMode::Command => "Command",
            VimMode::CommandLine => "CommandLine",
            VimMode::Insert => "Insert",
            VimMode::Visual => "Visual",
        };
        return write!(f, "{}", s);
    }
}

#[derive(Default)]
struct VimState {
    mode: VimMode,
    num_str: String,
    cmd_str: String,
}

struct State<'a> {
    // Use an `Option` to allow the window to not be available until the
    // application is properly running.
    window: Option<Arc<Window>>,
    font: &'a [u8],
    brush: Option<TextBrush<FontRef<'a>>>,
    text_area: textarea::TextArea,
    font_size: f32,
    section: Option<OwnedSection>,
    undo_history: undo::UndoHistory,

    has_copy_newline: bool,
    copy_lines: Vec<String>,

    scroll_row: usize,
    view_rows: usize,

    text_input: Option<textarea::TextArea>,
    text_input_height_ratio: f32,
    text_input_prompt: String,

    caret_pipeline: Option<wgpu::RenderPipeline>,
    caret_vertex_buffer: Option<wgpu::Buffer>,
    cursor_blink_start: Instant,

    is_ime_active: bool,
    vim_state: VimState,
    modifier: ModifiersState,

    current_dir: PathBuf,
    fileinfos: Vec<fileexplorer::FileInfo>,
    filepath: PathBuf,
    file_select_offset: usize,

    target_framerate: Duration,
    delta_time: Instant,
    fps_update_time: Instant,
    fps: i32,
    scale_factor: f32,

    // 是否关闭窗口，用来给命令行模式的 `:q` 命令使用
    is_closed: bool,

    // wgpu
    ctx: Option<Ctx>,
}

impl State<'_> {
    fn initialize(&mut self) {
        if self.text_input_height_ratio <= 0.0 || self.text_input_height_ratio >= 1.0 {
            panic!(
                "Invalid text_input_height_ratio: {}. It must be between 0.0 and 1.0 (exclusive).",
                self.text_input_height_ratio
            );
        }
        self.update_fileinfos();
    }

    fn update_fileinfos(&mut self) {
        match fileexplorer::list_directory(self.current_dir.to_str().unwrap()) {
            Ok(files) => {
                self.fileinfos = files;
            }
            Err(err) => {
                eprintln!("Error listing directory: {}", err);
                self.fileinfos.clear();
            }
        };

        self.file_select_offset = 5;
        let prompt = format!(
            "\" =========================================\n\" 目录: {}\n说明: d 表示目录 - 表示文件\n\" 帮助: hjkl:光标移动 n:新建文件\n\" =========================================\n\n",
            self.current_dir.to_str().unwrap()
        );
        self.text_area = textarea::TextArea::from_prompt(&prompt);
        self.text_area.insert_text_at_cursor("d .");
        self.text_area.append_line("d ..");
        for file in &self.fileinfos {
            self.text_area.append_line(&format!(
                "{} {:<20} {:>10} {}",
                if file.is_dir { "d" } else { "-" },
                file.name,
                file.size,
                file.format_modified_time()
            ));
        }
        self.text_area.goto_cursor(self.file_select_offset, 0);
    }

    /// 测量文本宽度，返回逻辑像素
    fn measure_text_width1(&mut self, text: &str, font_size: f32) -> f32 {
        if text.is_empty() {
            return 0.0;
        }

        // 从 Option 中获取 brush 的可变引用
        if let Some(brush) = &mut self.brush {
            // 构建用于测量的 Section
            let section = Section::default()
                .add_text(
                    Text::new(text)
                        .with_scale(font_size)
                        .with_color([1.0, 1.0, 1.0, 1.0]),
                )
                .with_bounds((f32::MAX, f32::MAX)); // 不限制宽度，让文本自然展开

            // 调用 glyph_bounds 获取边界框
            if let Some(rect) = brush.glyph_bounds(section) {
                // glyph_bounds 返回物理像素，需要除以 scale_factor 转为逻辑像素
                return rect.width() / self.scale_factor;
            }
        }

        // 降级方案：如果测量失败，使用粗略估算
        // 中文字符宽度约为字体大小，英文字符约为字体大小的 60%
        let estimated_width = text
            .chars()
            .map(|c| {
                if c.is_ascii() {
                    font_size * 0.6
                } else {
                    font_size // 中文字符通常占满
                }
            })
            .sum::<f32>();

        estimated_width
    }

    fn measure_text_height1(&mut self, text: &str, font_size: f32) -> f32 {
        if text.is_empty() {
            return 0.0;
        }

        if let Some(brush) = &mut self.brush {
            let section = Section::default()
                .add_text(
                    Text::new(text)
                        .with_scale(font_size)
                        .with_color([1.0, 1.0, 1.0, 1.0]),
                )
                .with_bounds((f32::MAX, f32::MAX))
                .with_layout(
                    Layout::default().line_breaker(BuiltInLineBreaker::UnicodeLineBreaker),
                );

            if let Some(rect) = brush.glyph_bounds(section) {
                return rect.height() / self.scale_factor;
            }
        }

        let line_count = text.lines().count() as f32;
        font_size * line_count
    }

    fn measure_text_width(&mut self, text: &str) -> f32 {
        self.measure_text_width1(text, self.font_size)
    }

    fn measure_text_height(&mut self, text: &str) -> f32 {
        self.measure_text_height1(text, self.font_size)
    }

    fn measure_line_advance(&mut self) -> f32 {
        let single_line_height = self.measure_text_height("Mg");
        let two_line_height = self.measure_text_height("Mg\nMg");
        let line_advance = (two_line_height - single_line_height).abs();

        if line_advance > 0.0 {
            line_advance
        } else {
            self.font_size
        }
    }

    fn build_caret_vertices(
        &self,
        cursor_x_logical: f32,
        cursor_y_logical: f32,
        viewport_width_px: f32,
        viewport_height_px: f32,
        alpha: f32,
    ) -> [f32; CARET_VERTEX_FLOATS] {
        let scale_factor = self.scale_factor;
        let x_px = (cursor_x_logical * scale_factor).round();
        let y_px =
            ((cursor_y_logical + self.font_size * CARET_TOP_OFFSET_RATIO) * scale_factor).round();
        let w_px = 2.0;
        let h_px = (self.font_size * CARET_HEIGHT_RATIO * scale_factor)
            .max(1.0)
            .round();

        let l = (x_px / viewport_width_px) * 2.0 - 1.0;
        let r = ((x_px + w_px) / viewport_width_px) * 2.0 - 1.0;
        let t = 1.0 - (y_px / viewport_height_px) * 2.0;
        let b = 1.0 - ((y_px + h_px) / viewport_height_px) * 2.0;

        let color = [0.95_f32, 0.95_f32, 0.95_f32, alpha];

        [
            l, t, color[0], color[1], color[2], color[3], r, t, color[0], color[1], color[2],
            color[3], l, b, color[0], color[1], color[2], color[3], r, t, color[0], color[1],
            color[2], color[3], r, b, color[0], color[1], color[2], color[3], l, b, color[0],
            color[1], color[2], color[3],
        ]
    }

    /// 获取文本区域的光标逻辑位置（逻辑像素），这个位置是相对于文本区域的左上角的，而不是屏幕/应用的左上角
    fn cursor_logical_position(&mut self) -> (f32, f32) {
        // 第一步：仅从 text_input/text_area 中提取纯数据（String/usize）
        //         match 产生的 &self 借用在此语句结束时立即释放
        let (cursor_prefix, mut preceding_lines) = match self.text_input.as_ref() {
            Some(text_input) => (
                text_input.cursor_prefix_string(),
                text_input.measure_lines_before_cursor_string(0),
            ),
            None => (
                self.text_area.cursor_prefix_string(),
                self.text_area
                    .measure_lines_before_cursor_string(self.scroll_row),
            ),
        };

        // 第二步：此时 self 上没有任何活跃借用，可以自由调用 &mut self 方法
        let cursor_x = self.measure_text_width(&cursor_prefix);
        let cursor_y = self.measure_text_height(&preceding_lines);

        (cursor_x, cursor_y)
    }

    fn save_file(&self) -> bool {
        match File::create(&self.filepath) {
            Ok(mut file) => {
                let content = self.text_area.string();
                match file.write_all(content.as_bytes()) {
                    Ok(_) => true,
                    Err(e) => {
                        eprintln!("Write file error: {}", e);
                        false
                    }
                }
            }
            Err(e) => {
                eprintln!("Open file error: {}", e);
                false
            }
        }
    }

    fn scroll_row(&mut self) {
        let (cursor_row, _) = self.text_area.cursor_position();
        if cursor_row < self.scroll_row {
            let old_scroll_row = self.scroll_row;
            self.scroll_row = cursor_row;
            if let Some(window) = self.window.clone().as_mut()
                && old_scroll_row != self.scroll_row
            {
                window.request_redraw();
            }
            return;
        }
        if self.view_rows > 0 && cursor_row.saturating_sub(self.scroll_row) + 2 >= self.view_rows {
            let old_scroll_row = self.scroll_row;
            self.scroll_row = cursor_row + 2 - self.view_rows + 1;
            if let Some(window) = self.window.clone().as_mut()
                && old_scroll_row != self.scroll_row
            {
                window.request_redraw();
            }
            return;
        }
    }

    fn copy_lines(&mut self, offset: usize, count: usize) {
        self.copy_lines = self.text_area.lines_with(offset, count);
        self.has_copy_newline = true;
    }

    fn handle_key_in_directory_mode(&mut self, key: Key) {
        if self.vim_state.mode != VimMode::Directory {
            return;
        }
        match key {
            Key::Named(k) => match self.text_input.as_mut() {
                Some(text_input) => match k {
                    NamedKey::Enter => {
                        let input_text = text_input.string();
                        let filename = &input_text[self.text_input_prompt.len()..];
                        let filepath = self.current_dir.join(filename);
                        match File::create_new(&filepath) {
                            Ok(_) => {
                                self.text_input = None;
                                self.vim_state.mode = VimMode::Command;
                                self.text_area = textarea::TextArea::new();
                                self.filepath = filepath;
                            }
                            Err(e) => {
                                eprintln!("Create new file error: {}", e);
                            }
                        }
                    }
                    NamedKey::Escape => {
                        self.text_input = None;
                        self.cursor_blink_start = Instant::now();
                    }
                    NamedKey::Backspace => {
                        text_input.delete_char_before_cursor(true);
                        self.cursor_blink_start = Instant::now();
                    }
                    _ => {}
                },
                None => match k {
                    NamedKey::Enter => {
                        let (cursor_row, _) = self.text_area.cursor_position();
                        if cursor_row == self.file_select_offset {
                            // Handle "d ." (current directory) no action needed
                        } else if cursor_row == self.file_select_offset + 1 {
                            // Handle "d .." (parent directory)
                            if let Some(parent) = self.current_dir.parent() {
                                self.current_dir = parent.to_path_buf();
                                self.update_fileinfos();
                            }
                        } else {
                            if let Some(selected_file) =
                                self.fileinfos.get(cursor_row - self.file_select_offset - 2)
                            {
                                if selected_file.is_dir {
                                    self.current_dir.push(&selected_file.name);
                                    self.update_fileinfos();
                                } else {
                                    println!("Selected file: {}", selected_file.name);
                                    // Read the file content and write it to the text area
                                    let file_path = self.current_dir.join(&selected_file.name);
                                    match std::fs::read_to_string(&file_path) {
                                        Ok(content) => {
                                            self.vim_state.mode = VimMode::Command;
                                            self.text_area =
                                                textarea::TextArea::from_string(&content);
                                            self.filepath = file_path;
                                        }
                                        Err(err) => {
                                            eprintln!(
                                                "Error reading file {}: {}",
                                                file_path.display(),
                                                err
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                },
            },
            Key::Character(char) if !self.is_ime_active => match self.text_input.as_mut() {
                Some(text_input) => text_input.insert_text_at_cursor(char.as_str()),
                None => match char.as_str() {
                    "G" => {
                        let line_count = self.text_area.line_count();
                        self.text_area.goto_cursor(line_count - 1, 0);
                    }
                    "h" => {
                        self.text_area.move_left_cursor();
                        self.cursor_blink_start = Instant::now();
                    }
                    "j" => {
                        self.text_area.move_down_cursor();
                        self.cursor_blink_start = Instant::now();
                    }
                    "k" => {
                        self.text_area.move_up_cursor();
                        self.cursor_blink_start = Instant::now();
                    }
                    "l" => {
                        self.text_area.move_right_cursor();
                        self.cursor_blink_start = Instant::now();
                    }
                    "n" => {
                        self.text_input =
                            Some(textarea::TextArea::from_prompt(&self.text_input_prompt));
                        self.cursor_blink_start = Instant::now();
                    }
                    _ => {}
                },
            },

            _ => {}
        }
    }

    fn handle_key_in_command_mode(&mut self, key: Key) {
        if self.vim_state.mode != VimMode::Command {
            return;
        }
        match key {
            Key::Named(k) => match k {
                NamedKey::Escape => {
                    self.vim_state.num_str.clear();
                    self.vim_state.cmd_str.clear();
                }
                _ => {}
            },
            Key::Character(char) if !self.is_ime_active => {
                if !self.vim_state.cmd_str.is_empty() {
                    self.vim_state.cmd_str.push_str(char.as_str());
                    match self.vim_state.cmd_str.as_str() {
                        "dd" => {
                            self.vim_state.cmd_str.clear();
                            let line_count: usize;
                            if self.vim_state.num_str.is_empty() {
                                line_count = 1;
                            } else {
                                line_count = self.vim_state.num_str.parse::<usize>().unwrap_or(1);
                                self.vim_state.num_str.clear();
                            }
                            self.copy_lines(self.text_area.cursor_position().0, line_count);
                            self.text_area.delete_lines_at_cursor(line_count);
                            let cmd = undo::DeleteLinesCmd::new(
                                self.text_area.cursor_position().0,
                                0,
                                self.copy_lines.clone(),
                            );
                            self.undo_history.record(Box::new(cmd));
                            self.undo_history.start_new_block();
                        }
                        "d$" => {
                            self.vim_state.cmd_str.clear();
                            self.text_area.delete_to_end_of_line();
                        }
                        "d^" => {
                            self.vim_state.cmd_str.clear();
                            self.text_area.delete_to_start_of_line();
                        }
                        "gg" => {
                            self.vim_state.cmd_str.clear();
                            self.text_area.goto_cursor(0, 0);
                            self.scroll_row();
                        }
                        "yy" => {
                            self.vim_state.cmd_str.clear();
                            let line_count: usize;
                            if self.vim_state.num_str.is_empty() {
                                line_count = 1;
                            } else {
                                line_count = self.vim_state.num_str.parse::<usize>().unwrap_or(1);
                                self.vim_state.num_str.clear();
                            }
                            let (cursor_row, _) = self.text_area.cursor_position();
                            self.copy_lines(cursor_row, line_count);
                        }
                        _ => {}
                    }
                    return;
                }
                match char.as_str() {
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "0" => {
                        self.vim_state.num_str.push_str(char.as_str());
                    }
                    "$" => {
                        self.text_area.end_offset_of_line_cursor(1);
                    }
                    "^" => {
                        self.text_area.first_non_blank_of_line_cursor();
                    }
                    "a" => {
                        self.text_area.move_right_cursor();
                        self.vim_state.mode = VimMode::Insert;
                        println!("Switched to Insert mode");
                    }
                    "A" => {
                        self.text_area.end_of_line_cursor();
                        self.vim_state.mode = VimMode::Insert;
                        println!("Switched to Insert mode");
                    }
                    "d" => {
                        self.vim_state.cmd_str.push_str(char.as_str());
                    }
                    "D" => {
                        self.text_area.delete_to_end_of_line();
                    }
                    "g" => {
                        self.vim_state.cmd_str.push_str(char.as_str());
                    }
                    "G" => {
                        let last_line = self.text_area.line_count().saturating_sub(1);
                        let mut line_num: usize;
                        if self.vim_state.num_str.is_empty() {
                            line_num = last_line;
                        } else {
                            line_num = self.vim_state.num_str.parse::<usize>().unwrap_or(0);
                            line_num = line_num.min(last_line);
                            self.vim_state.num_str.clear();
                        }
                        self.text_area.goto_cursor(line_num, 0);
                        self.scroll_row();
                    }
                    "h" => {
                        self.text_area.move_left_cursor();
                        self.cursor_blink_start = Instant::now();
                    }
                    "H" => {
                        let (_, cursor_col) = self.text_area.cursor_position();
                        self.text_area.goto_cursor(self.scroll_row, cursor_col);
                    }
                    "i" => {
                        self.vim_state.mode = VimMode::Insert;
                        println!("Switched to Insert mode");
                    }
                    "I" => {
                        self.text_area.start_of_line_cursor();
                        self.vim_state.mode = VimMode::Insert;
                        println!("Switched to Insert mode");
                    }
                    "j" => {
                        self.text_area.move_down_cursor();
                        self.cursor_blink_start = Instant::now();
                        self.scroll_row();
                    }
                    "k" => {
                        self.text_area.move_up_cursor();
                        self.cursor_blink_start = Instant::now();
                        self.scroll_row();
                        // let (cursor_row, _) = self.text_area.cursor_position();
                        // if self.scroll_row > 0 && cursor_row < self.scroll_row {
                        //     self.scroll_row -= 1;
                        // }
                    }
                    "l" => {
                        self.text_area.move_right1_cursor();
                        self.cursor_blink_start = Instant::now();
                    }
                    "L" => {
                        let (_, cursor_col) = self.text_area.cursor_position();
                        self.text_area
                            .goto_cursor(self.scroll_row + self.view_rows - 1, cursor_col);
                    }
                    "M" => {
                        let (_, cursor_col) = self.text_area.cursor_position();
                        let mut row = self.scroll_row + self.view_rows / 2;
                        row = row.min(self.text_area.line_count() / 2);
                        self.text_area.goto_cursor(row, cursor_col);
                    }
                    "o" => {
                        let (cursor_row, _) = self.text_area.cursor_position();
                        let cursor_end = self.text_area.line_length(cursor_row).unwrap_or(0);
                        self.text_area.new_line_below_cursor();
                        self.vim_state.mode = VimMode::Insert;
                        println!("Switched to Insert mode");
                        let cmd = undo::InsertNewlineCmd::new(cursor_row, cursor_end);
                        self.undo_history.record(Box::new(cmd));
                    }
                    "O" => {
                        let (cursor_row, _) = self.text_area.cursor_position();
                        self.text_area.new_line_above_cursor();
                        self.vim_state.mode = VimMode::Insert;
                        println!("Switched to Insert mode");
                        let cmd = undo::InsertNewlineCmd::new(cursor_row, 0);
                        self.undo_history.record(Box::new(cmd));
                    }
                    "p" => {
                        if !self.copy_lines.is_empty() {
                            let (cursor_row, cursor_col) = self.text_area.cursor_position();
                            if self.has_copy_newline {
                                let lines = self.copy_lines.clone();
                                self.text_area.insert_lines_at(
                                    cursor_row + 1,
                                    std::mem::take(&mut self.copy_lines),
                                );
                                let cmd = undo::InsertLinesCmd::new(cursor_row + 1, 0, lines);
                                self.undo_history.record(Box::new(cmd));
                                self.undo_history.start_new_block();
                            } else {
                                self.text_area.insert_text_at(
                                    cursor_row,
                                    cursor_col + 1,
                                    &self.copy_lines[0],
                                );
                            }
                        }
                    }
                    "P" => {
                        if !self.copy_lines.is_empty() {
                            let (cursor_row, cursor_col) = self.text_area.cursor_position();
                            if self.has_copy_newline {
                                let lines = self.copy_lines.clone();
                                self.text_area.insert_lines_at(
                                    cursor_row,
                                    std::mem::take(&mut self.copy_lines),
                                );
                                let cmd = undo::InsertLinesCmd::new(cursor_row, 0, lines);
                                self.undo_history.record(Box::new(cmd));
                                self.undo_history.start_new_block();
                            } else {
                                self.text_area.insert_text_at(
                                    cursor_row,
                                    cursor_col,
                                    &self.copy_lines[0],
                                );
                            }
                        }
                    }
                    "u" => {
                        self.undo_history.undo(&mut self.text_area);
                        self.text_area.clamp1_cursor();
                    }
                    "v" => {
                        self.vim_state.mode = VimMode::Visual;
                        println!("Switched to Visual mode");
                    }
                    "x" => {
                        let (cursor_row, cursor_col) = self.text_area.cursor_position();
                        let s = self.text_area.line_string_slice(cursor_row, cursor_col, 1);
                        self.text_area.delete_char_after_cursor(false);
                        if let Some(s) = s {
                            let cmd = undo::DeleteTextCmd::new(cursor_row, cursor_col, s);
                            self.undo_history.record(Box::new(cmd));
                            self.undo_history.start_new_block();
                        }
                    }
                    "X" => {
                        let (cursor_row, cursor_col) = self.text_area.cursor_position();
                        let s = match cursor_col {
                            0 => None,
                            _ => self
                                .text_area
                                .line_string_slice(cursor_row, cursor_col - 1, 1),
                        };
                        self.text_area.delete_char_before_cursor(false);
                        if let Some(s) = s {
                            let cmd = undo::DeleteTextCmd::new(cursor_row, cursor_col - 1, s);
                            self.undo_history.record(Box::new(cmd));
                            self.undo_history.start_new_block();
                        }
                    }
                    "y" => {
                        self.vim_state.cmd_str.push_str(char.as_str());
                    }
                    "Y" => {
                        let line_count: usize;
                        if self.vim_state.num_str.is_empty() {
                            line_count = 1;
                        } else {
                            line_count = self.vim_state.num_str.parse::<usize>().unwrap_or(1);
                            self.vim_state.num_str.clear();
                        }
                        let (cursor_row, _) = self.text_area.cursor_position();
                        self.copy_lines(cursor_row, line_count);
                    }
                    ":" => {
                        self.vim_state.mode = VimMode::CommandLine;
                        self.text_input = Some(textarea::TextArea::new());
                        if let Some(text_input) = self.text_input.as_mut() {
                            text_input.insert_text_at_cursor(":");
                        }
                        println!("Switched to Command-Line mode");
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn handle_key_in_command_line_mode(&mut self, key: Key) {
        if self.vim_state.mode != VimMode::CommandLine {
            return;
        }
        let text_input = self.text_input.as_mut().unwrap();
        match key {
            Key::Named(k) => match k {
                NamedKey::Enter => {
                    let input_text = text_input.string();
                    let cmd = &input_text[1..]; // 移除前面的冒号
                    match cmd {
                        ",d" => {
                            self.vim_state.mode = VimMode::Directory;
                            self.text_input = None;
                            self.update_fileinfos();
                            return;
                        }
                        "w" => {
                            self.save_file();
                        }
                        "wq" => {
                            if self.save_file() {
                                self.is_closed = true;
                            }
                        }
                        "q" => {
                            self.is_closed = true;
                        }
                        "qa" => {
                            self.is_closed = true;
                        }
                        _ => {}
                    };
                    self.text_input = None;
                    self.vim_state.mode = VimMode::Command;
                }
                NamedKey::Escape => {
                    self.text_input = None;
                    self.cursor_blink_start = Instant::now();
                }
                NamedKey::Backspace => {
                    text_input.delete_char_before_cursor(true);
                    self.cursor_blink_start = Instant::now();
                    if text_input.is_empty() {
                        self.text_input = None;
                        self.vim_state.mode = VimMode::Command;
                    }
                }
                _ => {}
            },
            Key::Character(char) => {
                text_input.insert_text_at_cursor(char.as_str());
            }
            _ => {}
        };
    }

    fn handle_key_in_insert_mode(&mut self, key: Key) {
        if self.vim_state.mode != VimMode::Insert {
            return;
        }
        match key {
            Key::Named(k) => match k {
                NamedKey::Escape => {
                    self.esc_insert_mode();
                }
                NamedKey::Delete => {
                    let (cursor_row, cursor_col) = self.text_area.cursor_position();
                    let line_length = self.text_area.line_length(cursor_row).unwrap_or(0);
                    let line_count = self.text_area.line_count();
                    let s = if cursor_col >= line_length {
                        None
                    } else {
                        self.text_area.line_string_slice(cursor_row, cursor_col, 1)
                    };
                    self.text_area.delete_char_after_cursor(true);
                    self.cursor_blink_start = Instant::now();
                    if let Some(s) = s {
                        let cmd = undo::DeleteText1Cmd::new(cursor_row, cursor_col, s);
                        self.undo_history.record(Box::new(cmd));
                    } else if cursor_col >= line_length && cursor_row < line_count - 1 {
                        // Handle delete at the end of a line (merge lines)
                        let cmd = undo::DeleteNewlineCmd::new(cursor_row, cursor_col);
                        self.undo_history.record(Box::new(cmd));
                    }
                }
                NamedKey::Backspace => {
                    let (cursor_row, cursor_col) = self.text_area.cursor_position();
                    let s = match cursor_col {
                        0 => None,
                        _ => self
                            .text_area
                            .line_string_slice(cursor_row, cursor_col - 1, 1),
                    };
                    let previous_line_length = if cursor_row == 0 {
                        0
                    } else {
                        self.text_area.line_length(cursor_row - 1).unwrap_or(0)
                    };
                    self.text_area.delete_char_before_cursor(true);
                    self.cursor_blink_start = Instant::now();
                    if let Some(s) = s {
                        let cmd = undo::DeleteText1Cmd::new(cursor_row, cursor_col - 1, s);
                        self.undo_history.record(Box::new(cmd));
                    } else if cursor_col == 0 && cursor_row > 0 {
                        // Handle backspace at the beginning of a line (merge lines)
                        let cmd = undo::DeleteNewlineCmd::new(cursor_row - 1, previous_line_length);
                        self.undo_history.record(Box::new(cmd));
                    }
                }
                NamedKey::Space => {
                    self.text_area.insert_text_at_cursor(" ");
                    self.cursor_blink_start = Instant::now();
                }
                NamedKey::Enter => {
                    let (cursor_row, cursor_col) = self.text_area.cursor_position();
                    self.text_area.insert_newline_at_cursor();
                    self.cursor_blink_start = Instant::now();
                    self.scroll_row();
                    let cmd = undo::InsertNewlineCmd::new(cursor_row, cursor_col);
                    self.undo_history.record(Box::new(cmd));
                }
                _ => {}
            },
            Key::Character(char) if !self.is_ime_active => {
                // Handle Ctrl+[ to switch to Command mode （终端里面 Ctrl + [ 对应 Esc ）
                if self.modifier.control_key() && char.as_str() == "[" {
                    self.esc_insert_mode();
                    return;
                }
                let (cursor_row, cursor_col) = self.text_area.cursor_position();
                self.text_area.insert_text_at_cursor(char.as_str());
                self.cursor_blink_start = Instant::now();
                let cmd =
                    undo::InsertTextCmd::new(cursor_row, cursor_col, char.as_str().to_string());
                self.undo_history.record(Box::new(cmd));
            }
            _ => (),
        }
    }

    fn handle_key_in_visual_mode(&mut self, _key: Key) {
        if self.vim_state.mode != VimMode::Visual {
            return;
        }
    }

    fn esc_insert_mode(&mut self) {
        self.vim_state.mode = VimMode::Command;
        // vim 命令模式下，光标应该在当前行的最后一个字符上，而不是行尾之后
        self.text_area.clamp1_cursor();
        self.undo_history.start_new_block();
        println!("Switched to Command mode");
    }

    fn redraw(&mut self) {
        self.text_area.clamp_cursor();
        let text_height = self.measure_line_advance();
        let (cursor_x, cursor_y) = self.cursor_logical_position();

        let elapsed_ms = self.cursor_blink_start.elapsed().as_millis();
        let caret_alpha = if elapsed_ms < CARET_BLINK_DELAY_MS
            || self.vim_state.mode != VimMode::Insert
        {
            1.0
        } else {
            let blink_on = ((elapsed_ms - CARET_BLINK_DELAY_MS) / CARET_BLINK_PERIOD_MS) % 2 == 0;
            if blink_on { 1.0 } else { 0.0 }
        };

        let line_count = self.text_area.line_count();
        let line_digit_count = line_count.to_string().len();
        let line_count_str = format!(" {} ", line_count);
        let line_count_width = self.measure_text_width(&line_count_str) * self.scale_factor;

        let ctx = self.ctx.as_ref().unwrap();
        let queue = &ctx.queue;
        let device = &ctx.device;
        let config = &ctx.config;
        let surface = &ctx.surface;
        let viewport_width = config.width as f32;
        let viewport_height = config.height as f32;

        self.view_rows = ((config.height as f32 * self.text_input_height_ratio)
            / text_height
            / self.scale_factor)
            .floor() as usize;

        let mut line_num_text = String::new();
        let line_num_start = self.scroll_row;
        let line_num_end = line_count.min(self.scroll_row + self.view_rows + 1);
        for i in line_num_start..line_num_end {
            let line_num = format!(" {:>width$} \n", i + 1, width = line_digit_count);
            line_num_text.push_str(&line_num);
        }
        let line_num_section = Section::default()
            .add_text(
                Text::new(&line_num_text)
                    .with_scale(self.font_size)
                    .with_color([0.5, 0.5, 0.5, 1.0]),
            )
            .with_bounds((
                config.width as f32,
                config.height as f32 * self.text_input_height_ratio,
            ))
            .with_layout(Layout::default().line_breaker(BuiltInLineBreaker::UnicodeLineBreaker))
            .with_screen_position((0.0, 0.0));

        let text = self.text_area.string_with_row_offset(self.scroll_row);
        let mut offset_x = line_count_width;
        if cursor_x * self.scale_factor > config.width as f32 * 0.8 {
            offset_x = line_count_width + config.width as f32 * 0.8 - cursor_x * self.scale_factor;
        }
        let section = Section::default()
            .add_text(
                Text::new(&text)
                    .with_scale(self.font_size)
                    .with_color([0.9, 0.5, 0.5, 1.0]),
            )
            .with_bounds((
                f32::MAX,
                config.height as f32 * self.text_input_height_ratio,
            ))
            .with_layout(Layout::default().line_breaker(BuiltInLineBreaker::UnicodeLineBreaker))
            .with_screen_position((offset_x, 0.0));
        // self.section = Some(section.to_owned());

        let mut section_x = section.screen_position.0; // Section 左上角 X 坐标 (逻辑像素)
        let mut section_y = section.screen_position.1; // Section 左上角 Y 坐标 (逻辑像素)

        let (cursor_row, cursor_col) = self.text_area.cursor_position();
        let name = self
            .filepath
            .file_name()
            .and_then(|os| os.to_str())
            .unwrap_or("Untitled");
        let status_text = format!(
            "{} | Ln {}, Col {} | Mode: {} | {:.1} {:.1} | {} ({})",
            name,
            cursor_row,
            cursor_col,
            self.vim_state.mode,
            cursor_x,
            cursor_y,
            self.scroll_row,
            self.view_rows,
        );
        let status_section = Section::default()
            .add_text(
                Text::new(&status_text)
                    .with_scale(self.font_size)
                    .with_color([0.2, 0.5, 0.8, 1.0]),
            )
            .with_bounds((config.width as f32, config.height as f32))
            .with_layout(Layout::default().line_breaker(BuiltInLineBreaker::AnyCharLineBreaker))
            .with_screen_position((0.0, (config.height as f32) - (text_height + 15.0)));

        let input_text = self
            .text_input
            .as_ref()
            .map(|text_input| text_input.string());
        let input_section = match input_text.as_ref() {
            Some(input_text) => {
                let input_section = Section::default()
                    .add_text(
                        Text::new(input_text)
                            .with_scale(self.font_size)
                            .with_color([0.5, 0.9, 0.5, 1.0]),
                    )
                    .with_bounds((config.width as f32, config.height as f32))
                    .with_layout(
                        Layout::default().line_breaker(BuiltInLineBreaker::UnicodeLineBreaker),
                    )
                    .with_screen_position((
                        0.0,
                        (config.height as f32) - 2.0 * (text_height + 15.0),
                    ));
                section_x = input_section.screen_position.0;
                section_y = input_section.screen_position.1;
                Some(input_section)
            }
            None => None,
        };

        let caret_vertices = self.build_caret_vertices(
            cursor_x + section_x / self.scale_factor,
            cursor_y + section_y / self.scale_factor,
            viewport_width,
            viewport_height,
            caret_alpha,
        );

        let brush = self.brush.as_mut().unwrap();

        let caret_vertex_buffer = self.caret_vertex_buffer.as_ref().unwrap();
        let caret_vertex_bytes = unsafe {
            std::slice::from_raw_parts(
                caret_vertices.as_ptr() as *const u8,
                std::mem::size_of_val(&caret_vertices),
            )
        };
        queue.write_buffer(caret_vertex_buffer, 0, caret_vertex_bytes);

        if let Some(input_section) = input_section {
            match brush.queue(
                device,
                queue,
                [section, status_section, line_num_section, input_section],
            ) {
                Ok(_) => (),
                Err(err) => {
                    panic!("{err}");
                }
            };
        } else {
            match brush.queue(device, queue, [section, status_section, line_num_section]) {
                Ok(_) => (),
                Err(err) => {
                    panic!("{err}");
                }
            };
        }

        let frame = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Occluded => return,
            _ => {
                surface.configure(device, config);
                match surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(s) => s,
                    e => {
                        panic!("Failed to acquire next surface texture: {:?}", e)
                    }
                }
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Command Encoder"),
        });

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.2,
                            g: 0.2,
                            b: 0.3,
                            a: 1.,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            brush.draw(&mut rpass);

            if caret_alpha > 0.0 {
                rpass.set_pipeline(self.caret_pipeline.as_ref().unwrap());
                rpass.set_vertex_buffer(0, caret_vertex_buffer.slice(..));
                rpass.draw(0..6, 0..1);
            }
        }

        queue.submit([encoder.finish()]);
        queue.present(frame);
    }
}

impl ApplicationHandler for State<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes().with_title("wgpu-text: 'simple' example"),
                )
                .unwrap(),
        );

        window.set_ime_allowed(true);

        self.ctx = Some(Ctx::new(window.clone()));
        self.scale_factor = window.scale_factor() as f32;

        let ctx = self.ctx.as_ref().unwrap();
        let device = &ctx.device;
        let config = &ctx.config;

        self.brush = Some(BrushBuilder::using_font_bytes(self.font).unwrap().build(
            device,
            config.width,
            config.height,
            config.format,
        ));

        let caret_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Caret Shader"),
            source: wgpu::ShaderSource::Wgsl(CARET_WGSL.into()),
        });

        let caret_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Caret Pipeline Layout"),
                bind_group_layouts: &[],
                immediate_size: 0,
            });

        let caret_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Caret Pipeline"),
            layout: Some(&caret_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &caret_shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: (6 * std::mem::size_of::<f32>()) as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: (2 * std::mem::size_of::<f32>()) as u64,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x4,
                        },
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &caret_shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let caret_vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Caret Vertex Buffer"),
            size: CARET_VERTEX_BUFFER_SIZE,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        self.caret_pipeline = Some(caret_pipeline);
        self.caret_vertex_buffer = Some(caret_vertex_buffer);

        self.window = Some(window);
    }

    fn window_event(
        &mut self,
        elwt: &ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::Resized(new_size) => {
                let ctx = self.ctx.as_mut().unwrap();
                let queue = &ctx.queue;
                let device = &ctx.device;
                let config = &mut ctx.config;
                let surface = &ctx.surface;
                let brush = self.brush.as_mut().unwrap();

                config.width = new_size.width.max(1);
                config.height = new_size.height.max(1);
                surface.configure(device, config);

                brush.resize_view(config.width as f32, config.height as f32, queue);
                // You can also do this!
                // brush.update_matrix(wgpu_text::ortho(config.width, config.height), &queue);
            }
            WindowEvent::CloseRequested => elwt.exit(),
            WindowEvent::ModifiersChanged(new_modifiers) => {
                self.modifier = new_modifiers.state();
            }
            WindowEvent::Ime(ime_event) => {
                match ime_event {
                    Ime::Enabled => {
                        self.is_ime_active = true;
                        println!("IME enabled for Window={window_id:?}");
                        let mut screen_x = 0.0 as f32;
                        let mut screen_y = 0.0 as f32;
                        if let Some(section) = &self.section {
                            screen_x = section.screen_position.0; // Section 左上角 X 坐标 (逻辑像素)
                            screen_y = section.screen_position.1; // Section 左上角 Y 坐标 (逻辑像素)
                        }
                        let font_size = self.font_size; // 当前字体大小

                        let (cursor_x, cursor_y) = self.cursor_logical_position();
                        let cursor_x_logical = screen_x + cursor_x;
                        let cursor_y_logical = screen_y + cursor_y;

                        // 转换为物理像素 (PhysicalPosition)glyph_brush
                        let scale_factor = self.scale_factor;
                        let physical_x = (cursor_x_logical * scale_factor) as i32;
                        let physical_y = ((cursor_y_logical + font_size * CARET_TOP_OFFSET_RATIO)
                            * scale_factor) as i32;
                        let physical_height =
                            (font_size * CARET_HEIGHT_RATIO * scale_factor) as u32;

                        self.window.as_ref().unwrap().set_ime_cursor_area(
                            PhysicalPosition::new(physical_x, physical_y),
                            PhysicalSize::new(2, physical_height), // 宽度为2像素的光标竖线
                        );
                    }
                    Ime::Disabled => {
                        self.is_ime_active = false;
                        println!("IME disabled for Window={window_id:?}");
                    }
                    Ime::Preedit(text, caret_pos) => {
                        println!("Preedit: {}, with caret at {:?}", text, caret_pos);
                    }
                    Ime::Commit(text) => {
                        println!("Committed: {}", text);
                        if self.vim_state.mode == VimMode::Insert {
                            self.text_area.insert_text_at_cursor(&text);
                            self.cursor_blink_start = Instant::now();
                        }
                    }
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key,
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => match self.vim_state.mode {
                VimMode::Directory => self.handle_key_in_directory_mode(logical_key),
                VimMode::Command => self.handle_key_in_command_mode(logical_key),
                VimMode::CommandLine => self.handle_key_in_command_line_mode(logical_key),
                VimMode::Insert => self.handle_key_in_insert_mode(logical_key),
                VimMode::Visual => self.handle_key_in_visual_mode(logical_key),
            },
            WindowEvent::MouseWheel {
                delta: MouseScrollDelta::LineDelta(_, y),
                ..
            } => {
                // increase/decrease font size
                let mut size = self.font_size;
                if y > 0.0 {
                    size += (size / 4.0).max(2.0)
                } else {
                    size *= 4.0 / 5.0
                };
                self.font_size = (size.clamp(3.0, 300.0) * 2.0).round() / 2.0;
            }
            WindowEvent::RedrawRequested => {
                self.redraw();
            }
            _ => (),
        };

        if self.is_closed {
            elwt.exit();
        }
    }

    fn new_events(&mut self, _elwt: &ActiveEventLoop, _cause: winit::event::StartCause) {
        if self.target_framerate <= self.delta_time.elapsed()
            && let Some(window) = self.window.clone().as_mut()
        {
            window.request_redraw();
            self.delta_time = Instant::now();
            self.fps += 1;
            if self.fps_update_time.elapsed().as_millis() > 1000 {
                window.set_title(&format!("hue FPS: {}", self.fps));
                self.fps = 0;
                self.fps_update_time = Instant::now();
            }
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        println!("Exiting!");
    }
}

fn main() -> Result<(), winit::error::EventLoopError> {
    if std::env::var("RUST_LOG").is_err() {
        unsafe {
            std::env::set_var("RUST_LOG", "error");
        }
    }

    let event_loop = event_loop::EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Poll);

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let mut state = State {
        window: None,
        font: include_bytes!("fonts/SarasaMonoSC-Regular.ttf"),
        brush: None,
        text_area: textarea::TextArea::new(),
        font_size: 28.,
        section: None,
        undo_history: undo::UndoHistory::new(),

        has_copy_newline: false,
        copy_lines: Vec::new(),

        scroll_row: 0,
        view_rows: 0,

        text_input: None,
        text_input_height_ratio: 0.9,
        text_input_prompt: "Enter new filename: ".to_string(),

        caret_pipeline: None,
        caret_vertex_buffer: None,
        cursor_blink_start: Instant::now(),

        is_ime_active: false,
        vim_state: VimState::default(),
        modifier: ModifiersState::empty(),

        current_dir: cwd,
        fileinfos: Vec::new(),
        filepath: PathBuf::new(),
        file_select_offset: 0,

        // FPS and window updating:
        // change '60.0' if you want different FPS cap
        target_framerate: Duration::from_secs_f64(1.0 / 60.0),
        delta_time: Instant::now(),
        fps_update_time: Instant::now(),
        fps: 0,
        scale_factor: 1.0,

        is_closed: false,

        ctx: None,
    };

    state.initialize();

    event_loop.run_app(&mut state)
}
