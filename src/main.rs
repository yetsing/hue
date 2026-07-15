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

struct State<'a> {
    // Use an `Option` to allow the window to not be available until the
    // application is properly running.
    window: Option<Arc<Window>>,
    font: &'a [u8],
    brush: Option<TextBrush<FontRef<'a>>>,
    random_text: String,
    font_size: f32,
    section: Option<OwnedSection>,
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
                    let text = self.random_text.clone();
                    let cursor_x_offset = self.measure_text_width(&text); // 假设光标在文本末尾
                    let cursor_x_logical = screen_x + cursor_x_offset; // 假设光标在文本的第 100 个像素位置
                    let cursor_y_logical = screen_y; // 候选框通常在光标上方或下方弹出

                    // 转换为物理像素 (PhysicalPosition)glyph_brush
                    let scale_factor = self.window.as_ref().unwrap().scale_factor() as f32;
                    let physical_x = (cursor_x_logical * scale_factor) as i32;
                    let physical_y = (cursor_y_logical * scale_factor) as i32;
                    let physical_height = (font_size * scale_factor) as u32;

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
                    self.random_text.push_str(&text);
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
                    NamedKey::Delete => self.random_text.clear(),
                    NamedKey::Backspace => {
                        self.random_text.pop();
                    }
                    _ => (),
                },
                Key::Character(char) if !self.ime_active => {
                    self.random_text.push_str(char.as_str());
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
                let brush = self.brush.as_mut().unwrap();
                let ctx = self.ctx.as_ref().unwrap();
                let queue = &ctx.queue;
                let device = &ctx.device;
                let config = &ctx.config;
                let surface = &ctx.surface;

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

                    brush.draw(&mut rpass)
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