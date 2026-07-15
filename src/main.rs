#[path = "ctx.rs"]
mod ctx;

use ctx::Ctx;
use glyph_brush::ab_glyph::FontRef;
use glyph_brush::OwnedSection;
use std::sync::Arc;
use std::time::{Duration, Instant};
use wgpu_text::glyph_brush::{BuiltInLineBreaker, Layout, Section, Text};
use wgpu_text::{BrushBuilder, TextBrush};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{KeyEvent, MouseScrollDelta, Ime};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{Key, NamedKey};
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

struct State<'a> {
    // Use an `Option` to allow the window to not be available until the
    // application is properly running.
    window: Option<Arc<Window>>,
    font: &'a [u8],
    brush: Option<TextBrush<FontRef<'a>>>,
    random_text: String,
    font_size: f32,
    section: Option<OwnedSection>,
    caret_pipeline: Option<wgpu::RenderPipeline>,
    caret_vertex_buffer: Option<wgpu::Buffer>,
    cursor_index_chars: usize,
    cursor_blink_start: Instant,
    ime_active: bool,

    target_framerate: Duration,
    delta_time: Instant,
    fps_update_time: Instant,
    fps: i32,

    // wgpu
    ctx: Option<Ctx>,
}

impl State<'_> {
    /// 测量文本宽度，返回逻辑像素
    fn measure_text_width(&mut self, text: &str) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        
        // 从 Option 中获取 brush 的可变引用
        if let Some(brush) = &mut self.brush {
            // 构建用于测量的 Section
            let section = Section::default()
                .add_text(
                    Text::new(text)
                        .with_scale(self.font_size)
                        .with_color([1.0, 1.0, 1.0, 1.0]),
                )
                .with_bounds((f32::MAX, f32::MAX)); // 不限制宽度，让文本自然展开
            
            // 调用 glyph_bounds 获取边界框
            if let Some(rect) = brush.glyph_bounds(section) {
                let scale_factor = self.window.as_ref().unwrap().scale_factor() as f32;
                // glyph_bounds 返回物理像素，需要除以 scale_factor 转为逻辑像素
                return rect.width() / scale_factor;
            }
        }
        
        // 降级方案：如果测量失败，使用粗略估算
        // 中文字符宽度约为字体大小，英文字符约为字体大小的 60%
        let estimated_width = text.chars()
            .map(|c| {
                if c.is_ascii() {
                    self.font_size * 0.6
                } else {
                    self.font_size // 中文字符通常占满
                }
            })
            .sum::<f32>();
        
        estimated_width
    }

    fn char_to_byte_index(s: &str, char_index: usize) -> usize {
        if char_index == 0 {
            return 0;
        }
        s.char_indices()
            .nth(char_index)
            .map_or(s.len(), |(byte_idx, _)| byte_idx)
    }

    fn insert_text_at_cursor(&mut self, text: &str) {
        let byte_idx = Self::char_to_byte_index(&self.random_text, self.cursor_index_chars);
        self.random_text.insert_str(byte_idx, text);
        self.cursor_index_chars += text.chars().count();
        self.cursor_blink_start = Instant::now();
    }

    fn delete_char_before_cursor(&mut self) {
        if self.cursor_index_chars == 0 {
            return;
        }
        let end = Self::char_to_byte_index(&self.random_text, self.cursor_index_chars);
        let start = Self::char_to_byte_index(&self.random_text, self.cursor_index_chars - 1);
        self.random_text.replace_range(start..end, "");
        self.cursor_index_chars -= 1;
        self.cursor_blink_start = Instant::now();
    }

    fn delete_char_after_cursor(&mut self) {
        let len = self.random_text.chars().count();
        if self.cursor_index_chars >= len {
            return;
        }
        let start = Self::char_to_byte_index(&self.random_text, self.cursor_index_chars);
        let end = Self::char_to_byte_index(&self.random_text, self.cursor_index_chars + 1);
        self.random_text.replace_range(start..end, "");
        self.cursor_blink_start = Instant::now();
    }

    fn clamp_cursor(&mut self) {
        let len = self.random_text.chars().count();
        if self.cursor_index_chars > len {
            self.cursor_index_chars = len;
        }
    }

    fn build_caret_vertices(
        &self,
        cursor_x_logical: f32,
        viewport_width_px: f32,
        viewport_height_px: f32,
        alpha: f32,
    ) -> [f32; CARET_VERTEX_FLOATS] {
        let scale_factor = self.window.as_ref().unwrap().scale_factor() as f32;
        let x_px = (cursor_x_logical * scale_factor).round();
        let y_px = (self.font_size * CARET_TOP_OFFSET_RATIO * scale_factor).round();
        let w_px = 2.0;
        let h_px = (self.font_size * CARET_HEIGHT_RATIO * scale_factor).max(1.0).round();

        let l = (x_px / viewport_width_px) * 2.0 - 1.0;
        let r = ((x_px + w_px) / viewport_width_px) * 2.0 - 1.0;
        let t = 1.0 - (y_px / viewport_height_px) * 2.0;
        let b = 1.0 - ((y_px + h_px) / viewport_height_px) * 2.0;

        let color = [0.95_f32, 0.95_f32, 0.95_f32, alpha];

        [
            l, t, color[0], color[1], color[2], color[3],
            r, t, color[0], color[1], color[2], color[3],
            l, b, color[0], color[1], color[2], color[3],
            r, t, color[0], color[1], color[2], color[3],
            r, b, color[0], color[1], color[2], color[3],
            l, b, color[0], color[1], color[2], color[3],
        ]
    }

}

impl ApplicationHandler for State<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("wgpu-text: 'simple' example"),
                )
                .unwrap(),
        );

        window.set_ime_allowed(true);

        self.ctx = Some(Ctx::new(window.clone()));

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
            WindowEvent::Ime(ime_event) => {
                match ime_event {
                Ime::Enabled =>{
                    self.ime_active = true;
                    println!("IME enabled for Window={window_id:?}");
                    let mut screen_x = 0.0 as f32;
                    let mut screen_y = 0.0 as f32;
                    if let Some(section) = &self.section {
                        screen_x = section.screen_position.0; // Section 左上角 X 坐标 (逻辑像素)
                        screen_y = section.screen_position.1; // Section 左上角 Y 坐标 (逻辑像素)
                    }
                    let font_size = self.font_size; // 当前字体大小

                    // 这里的 cursor_x_offset 需要根据光标在文本中的位置计算字符宽度
                    // 使用 wgpu-text 的 glyph_brush 来计算会比较精确
                    let cursor_prefix = self
                        .random_text
                        .chars()
                        .take(self.cursor_index_chars)
                        .collect::<String>();
                    let cursor_x_offset = self.measure_text_width(&cursor_prefix);
                    let cursor_x_logical = screen_x + cursor_x_offset; // 假设光标在文本的第 100 个像素位置
                    let cursor_y_logical = screen_y; // 候选框通常在光标上方或下方弹出

                    // 转换为物理像素 (PhysicalPosition)glyph_brush
                    let scale_factor = self.window.as_ref().unwrap().scale_factor() as f32;
                    let physical_x = (cursor_x_logical * scale_factor) as i32;
                    let physical_y = (cursor_y_logical * scale_factor) as i32;
                    let physical_height = (font_size * CARET_HEIGHT_RATIO * scale_factor) as u32;

                    self.window.as_ref().unwrap().set_ime_cursor_area(
                        PhysicalPosition::new(physical_x, physical_y),
                        PhysicalSize::new(2, physical_height) // 宽度为2像素的光标竖线
                    );
                },
                Ime::Disabled => {
                    self.ime_active = false;
                    println!("IME disabled for Window={window_id:?}");
                },
                Ime::Preedit(text, caret_pos) => {
                    println!("Preedit: {}, with caret at {:?}", text, caret_pos);
                },
                Ime::Commit(text) => {
                    println!("Committed: {}", text);
                    self.insert_text_at_cursor(&text);
                },
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
            } => match logical_key {
                Key::Named(k) => match k {
                    NamedKey::Escape => elwt.exit(),
                    NamedKey::Delete => {
                        self.delete_char_after_cursor();
                    }
                    NamedKey::Backspace => {
                        self.delete_char_before_cursor();
                    }
                    NamedKey::Space => {
                        self.insert_text_at_cursor(" ");
                    }
                    NamedKey::ArrowLeft => {
                        if self.cursor_index_chars > 0 {
                            self.cursor_index_chars -= 1;
                            self.cursor_blink_start = Instant::now();
                        }
                    }
                    NamedKey::ArrowRight => {
                        if self.cursor_index_chars < self.random_text.chars().count() {
                            self.cursor_index_chars += 1;
                            self.cursor_blink_start = Instant::now();
                        }
                    }
                    _ => (),
                },
                Key::Character(char) if !self.ime_active => {
                    self.insert_text_at_cursor(char.as_str());
                }
                _ => (),
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
                self.clamp_cursor();

                let cursor_prefix = self
                    .random_text
                    .chars()
                    .take(self.cursor_index_chars)
                    .collect::<String>();
                let cursor_x = self.measure_text_width(&cursor_prefix);

                let elapsed_ms = self.cursor_blink_start.elapsed().as_millis();
                let caret_alpha = if elapsed_ms < CARET_BLINK_DELAY_MS {
                    1.0
                } else {
                    let blink_on =
                        ((elapsed_ms - CARET_BLINK_DELAY_MS) / CARET_BLINK_PERIOD_MS) % 2 == 0;
                    if blink_on { 1.0 } else { 0.0 }
                };

                let ctx = self.ctx.as_ref().unwrap();
                let queue = &ctx.queue;
                let device = &ctx.device;
                let config = &ctx.config;
                let surface = &ctx.surface;
                let viewport_width = config.width as f32;
                let viewport_height = config.height as f32;

                let section = Section::default()
                    .add_text(
                        Text::new(&self.random_text)
                            .with_scale(self.font_size)
                            .with_color([0.9, 0.5, 0.5, 1.0]),
                    )
                    .with_bounds((config.width as f32, config.height as f32))
                    .with_layout(
                        Layout::default()
                            .line_breaker(BuiltInLineBreaker::AnyCharLineBreaker),
                    );
                self.section = Some(section.to_owned());

                let caret_vertices =
                    self.build_caret_vertices(cursor_x, viewport_width, viewport_height, caret_alpha);

                let brush = self.brush.as_mut().unwrap();

                let caret_vertex_buffer = self.caret_vertex_buffer.as_ref().unwrap();
                let caret_vertex_bytes = unsafe {
                    std::slice::from_raw_parts(
                        caret_vertices.as_ptr() as *const u8,
                        std::mem::size_of_val(&caret_vertices),
                    )
                };
                queue.write_buffer(caret_vertex_buffer, 0, caret_vertex_bytes);

                match brush.queue(device, queue, [self.section.as_ref().unwrap()]) {
                    Ok(_) => (),
                    Err(err) => {
                        panic!("{err}");
                    }
                };

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

                let mut encoder =
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("Command Encoder"),
                    });

                {
                    let mut rpass =
                        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
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
            _ => (),
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
                window.set_title(&format!(
                    "wgpu-text: 'performance' example, FPS: {}",
                    self.fps
                ));
                self.fps = 0;
                self.fps_update_time = Instant::now();
            }
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        println!("Exiting!");
    }
}

fn main() {
    if std::env::var("RUST_LOG").is_err() {
        unsafe {
            std::env::set_var("RUST_LOG", "error");
        }
    }

    let event_loop = event_loop::EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut state = State {
        window: None,
        font: include_bytes!("fonts/SarasaMonoSC-Regular.ttf"),
        brush: None,
        random_text: "Hello, world!".to_string(),
        font_size: 28.,
        section: None,
        caret_pipeline: None,
        caret_vertex_buffer: None,
        cursor_index_chars: "Hello, world!".chars().count(),
        cursor_blink_start: Instant::now(),
        ime_active: false,

        // FPS and window updating:
        // change '60.0' if you want different FPS cap
        target_framerate: Duration::from_secs_f64(1.0 / 60.0),
        delta_time: Instant::now(),
        fps_update_time: Instant::now(),
        fps: 0,

        ctx: None,
    };

    let _ = event_loop.run_app(&mut state);
}