#![allow(clippy::disallowed_types, dead_code, static_mut_refs)]

mod css_engine;
mod gpu;
mod html_parser;
#[cfg(feature = "js")]
mod js_bridge;
mod layout;
mod renderer;
mod types;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowId},
};

use css_engine::{apply_style_overrides, apply_styles};
use gpu::GpuState;
use html_parser::{extract_styles, parse_html};
use layout::{build_layout, LayoutTree, TextMeasureCache};
use renderer::generate_draw_commands;
use types::DrawCommand;

#[cfg(target_os = "android")]
mod embedded_assets {
    pub const INDEX_HTML: &str = include_str!("../assets/index.html");
    pub const TODO_HTML: &str = include_str!("../assets/todo.html");
    pub const ABOUT_HTML: &str = include_str!("../assets/about.html");
    pub const CANVAS_HTML: &str = include_str!("../assets/canvas.html");

    pub fn get(name: &str) -> Option<&'static str> {
        match name {
            "index.html" => Some(INDEX_HTML),
            "todo.html" => Some(TODO_HTML),
            "about.html" => Some(ABOUT_HTML),
            "canvas.html" => Some(CANVAS_HTML),
            _ => None,
        }
    }
}

struct WebviewState {
    gpu: GpuState,
    #[cfg(feature = "js")]
    js: js_bridge::JsBridge,
    layout: LayoutTree,
    commands: Vec<DrawCommand>,
    clear_color: [f32; 4],
    html_source: String,
    css_sources: Vec<String>,
    styled_base: types::StyledNode,
    asset_dir: PathBuf,
    #[allow(dead_code)]
    start_time: Instant,
    scroll_y: f32,
    mouse_down: bool,
    text_cache: TextMeasureCache,
}

#[derive(Default)]
struct App {
    state: Option<WebviewState>,
}

impl App {
    fn rebuild_layout(&mut self) {
        let state = self.state.as_mut().unwrap();
        let mut styled = state.styled_base.clone();

        #[cfg(feature = "js")]
        {
            let overrides = state.js.text_overrides().clone();
            apply_text_overrides(&mut styled, &overrides);
            let style_ov = state.js.style_overrides();
            apply_style_overrides(&mut styled, &style_ov);
        }

        #[cfg(feature = "js")]
        let canvas_ops = state.js.canvas_ops();
        #[cfg(not(feature = "js"))]
        let canvas_ops = std::collections::HashMap::new();

        let size = state.gpu.size;
        let scale = state.gpu.scale_factor as f32;
        state.layout = build_layout(
            &styled,
            size.width as f32 / scale,
            size.height as f32 / scale,
            &mut state.text_cache,
            &mut |text, font_size, max_width| state.gpu.measure_text(text, font_size, max_width),
        );
        state.commands = generate_draw_commands(&state.layout, &canvas_ops);
        state.clear_color = styled.style.background_color;
        #[cfg(feature = "js")]
        {
            state
                .js
                .update_element_rects(state.layout.collect_element_rects());
            state.js.clear_dirty();
        }
    }

    fn full_rebuild(&mut self) {
        let state = self.state.as_mut().unwrap();
        let dom = parse_html(&state.html_source);
        state.styled_base = apply_styles(&dom, &state.css_sources);
        self.rebuild_layout();
    }

    fn navigate(&mut self, href: &str) {
        let state = self.state.as_mut().unwrap();
        let path = state.asset_dir.join(href);
        let html_source = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Navigation failed: {e}");
                return;
            }
        };

        let css_sources = extract_styles(&html_source);
        #[cfg(feature = "js")]
        {
            let script = html_parser::extract_script(&html_source);
            state.js =
                js_bridge::JsBridge::new(script.as_deref()).expect("failed to init JS bridge");
        }

        state.html_source = html_source;
        state.css_sources = css_sources;
        state.scroll_y = 0.0;

        self.full_rebuild();
        self.state.as_ref().unwrap().gpu.window.request_redraw();
    }
}

#[cfg(feature = "js")]
fn apply_text_overrides(
    styled: &mut types::StyledNode,
    overrides: &std::collections::HashMap<String, String>,
) {
    if let Some(id) = &styled.dom_node.id {
        if let Some(text) = overrides.get(id) {
            styled.children.clear();
            styled.dom_node.children.clear();
            let child_style = types::ComputedStyle {
                display: types::Display::Inline,
                color: styled.style.color,
                font_size: styled.style.font_size,
                ..types::ComputedStyle::default()
            };
            styled.children.push(types::StyledNode {
                dom_node: types::DomNode {
                    tag: "#text".to_string(),
                    id: None,
                    classes: Vec::new(),
                    inline_style: String::new(),
                    href: None,
                    text: text.clone(),
                    children: Vec::new(),
                },
                style: child_style,
                children: Vec::new(),
            });
            return;
        }
    }
    for child in &mut styled.children {
        apply_text_overrides(child, overrides);
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window_attrs = Window::default_attributes().with_title("Custom Webview - wgpu");
        #[cfg(not(target_os = "android"))]
        let window_attrs = window_attrs.with_inner_size(winit::dpi::PhysicalSize::new(1280, 720));

        let window = Arc::new(event_loop.create_window(window_attrs).unwrap());

        let mut gpu = pollster::block_on(GpuState::new(
            event_loop.owned_display_handle(),
            window.clone(),
        ))
        .unwrap();

        #[cfg(not(target_os = "android"))]
        let html_source = {
            let asset_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets");
            let html_file = std::env::args()
                .nth(1)
                .unwrap_or_else(|| "index.html".to_string());
            std::fs::read_to_string(asset_dir.join(&html_file)).expect("failed to read HTML file")
        };
        #[cfg(target_os = "android")]
        let html_source = embedded_assets::get("todo.html")
            .unwrap_or(embedded_assets::INDEX_HTML)
            .to_string();

        let css_sources = extract_styles(&html_source);

        #[cfg(feature = "js")]
        let js = {
            let script = html_parser::extract_script(&html_source);
            js_bridge::JsBridge::new(script.as_deref()).expect("failed to init JS bridge")
        };

        let dom = parse_html(&html_source);
        let styled_base = apply_styles(&dom, &css_sources);
        let mut styled = styled_base.clone();

        #[cfg(feature = "js")]
        {
            let overrides = js.text_overrides().clone();
            apply_text_overrides(&mut styled, &overrides);
            let style_ov = js.style_overrides();
            apply_style_overrides(&mut styled, &style_ov);
        }

        #[cfg(feature = "js")]
        let canvas_ops = js.canvas_ops();
        #[cfg(not(feature = "js"))]
        let canvas_ops = std::collections::HashMap::new();

        let size = gpu.size;
        let scale = gpu.scale_factor as f32;
        let mut text_cache = TextMeasureCache::default();
        let layout_tree = build_layout(
            &styled,
            size.width as f32 / scale,
            size.height as f32 / scale,
            &mut text_cache,
            &mut |text, font_size, max_width| gpu.measure_text(text, font_size, max_width),
        );
        let commands = generate_draw_commands(&layout_tree, &canvas_ops);

        let clear_color = styled.style.background_color;

        self.state = Some(WebviewState {
            gpu,
            #[cfg(feature = "js")]
            js,
            layout: layout_tree,
            commands,
            clear_color,
            html_source,
            css_sources,
            styled_base,
            #[cfg(not(target_os = "android"))]
            asset_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets"),
            #[cfg(target_os = "android")]
            asset_dir: PathBuf::new(),
            start_time: Instant::now(),
            scroll_y: 0.0,
            mouse_down: false,
            text_cache,
        });

        window.request_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if self.state.is_none() {
            return;
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                self.state.as_mut().unwrap().gpu.resize(size);
                self.rebuild_layout();
                self.state.as_ref().unwrap().gpu.window.request_redraw();
            }
            WindowEvent::MouseInput {
                state: winit::event::ElementState::Pressed,
                button: winit::event::MouseButton::Left,
                ..
            } => {
                let (mx, my) = unsafe { CURSOR_POS };
                let s = self.state.as_mut().unwrap();
                s.mouse_down = true;
                let hit = renderer::hit_test(&s.layout, mx, my + s.scroll_y);
                if let Some(ref hit) = hit {
                    if let Some(ref href) = hit.href {
                        let href = href.clone();
                        self.navigate(&href);
                        return;
                    }
                }
                #[cfg(feature = "js")]
                {
                    let s = self.state.as_mut().unwrap();
                    let y = my + s.scroll_y;
                    s.js.dispatch_pointer_down(&s.layout, mx, y);
                    if s.js.is_dirty() {
                        self.rebuild_layout();
                        self.state.as_ref().unwrap().gpu.window.request_redraw();
                    }
                }
            }
            WindowEvent::MouseInput {
                state: winit::event::ElementState::Released,
                button: winit::event::MouseButton::Left,
                ..
            } => {
                self.state.as_mut().unwrap().mouse_down = false;
                #[cfg(feature = "js")]
                {
                    let s = self.state.as_mut().unwrap();
                    let (mx, my) = unsafe { CURSOR_POS };
                    let y = my + s.scroll_y;
                    s.js.dispatch_pointer_up(&s.layout, mx, y);
                    if s.js.is_dirty() {
                        self.rebuild_layout();
                        self.state.as_ref().unwrap().gpu.window.request_redraw();
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let s = self.state.as_mut().unwrap();
                let dy = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => -y * 40.0,
                    winit::event::MouseScrollDelta::PixelDelta(pos) => -pos.y as f32,
                };
                let max_scroll = (s.layout.content_height()
                    - s.gpu.size.height as f32 / s.gpu.scale_factor as f32)
                    .max(0.0);
                s.scroll_y = (s.scroll_y + dy).clamp(0.0, max_scroll);
                s.gpu.window.request_redraw();
            }
            WindowEvent::Touch(touch) => {
                let scale = self.state.as_ref().unwrap().gpu.scale_factor as f32;
                let mx = touch.location.x as f32 / scale;
                let my = touch.location.y as f32 / scale;
                unsafe {
                    CURSOR_POS = (mx, my);
                }
                match touch.phase {
                    winit::event::TouchPhase::Started => {
                        self.state.as_mut().unwrap().mouse_down = true;
                        #[cfg(feature = "js")]
                        {
                            let s = self.state.as_mut().unwrap();
                            let y = my + s.scroll_y;
                            s.js.dispatch_pointer_down(&s.layout, mx, y);
                            if s.js.is_dirty() {
                                self.rebuild_layout();
                            }
                        }
                        self.state.as_ref().unwrap().gpu.window.request_redraw();
                    }
                    winit::event::TouchPhase::Moved => {
                        #[cfg(feature = "js")]
                        {
                            let s = self.state.as_mut().unwrap();
                            let y = my + s.scroll_y;
                            s.js.dispatch_pointer_move(&s.layout, mx, y);
                            if s.js.is_dirty() {
                                self.rebuild_layout();
                            }
                        }
                        self.state.as_ref().unwrap().gpu.window.request_redraw();
                    }
                    winit::event::TouchPhase::Ended | winit::event::TouchPhase::Cancelled => {
                        self.state.as_mut().unwrap().mouse_down = false;
                        #[cfg(feature = "js")]
                        {
                            let s = self.state.as_mut().unwrap();
                            let y = my + s.scroll_y;
                            s.js.dispatch_pointer_up(&s.layout, mx, y);
                            if s.js.is_dirty() {
                                self.rebuild_layout();
                            }
                        }
                        self.state.as_ref().unwrap().gpu.window.request_redraw();
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => unsafe {
                CURSOR_POS = (position.x as f32, position.y as f32);
                if self.state.as_ref().map(|s| s.mouse_down).unwrap_or(false) {
                    self.state.as_ref().unwrap().gpu.window.request_redraw();
                }
            },
            WindowEvent::RedrawRequested => {
                #[cfg(feature = "js")]
                {
                    {
                        let s = self.state.as_mut().unwrap();
                        let now_ms = s.start_time.elapsed().as_secs_f64() * 1000.0;
                        s.js.tick(now_ms);
                    }
                    {
                        let s = self.state.as_mut().unwrap();
                        if s.mouse_down {
                            let (mx, my) = unsafe { CURSOR_POS };
                            s.js.dispatch_pointer_move(&s.layout, mx, my + s.scroll_y);
                        }
                    }
                    if self.state.as_ref().unwrap().js.is_dirty() {
                        self.rebuild_layout();
                    }
                }
                let s = self.state.as_mut().unwrap();
                s.gpu.render(&s.commands, s.clear_color, s.scroll_y);
                s.gpu.window.request_redraw();
            }
            _ => {}
        }
    }
}

static mut CURSOR_POS: (f32, f32) = (0.0, 0.0);

fn main() {
    #[cfg(not(target_os = "android"))]
    {
        env_logger::init();

        let event_loop = EventLoop::new().unwrap();
        event_loop.set_control_flow(ControlFlow::Poll);

        let mut app = App::default();
        event_loop.run_app(&mut app).unwrap();
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub fn android_main(app: winit::platform::android::activity::AndroidApp) {
    use winit::platform::android::EventLoopBuilderExtAndroid;

    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );

    std::panic::set_hook(Box::new(|info| {
        log::error!("PANIC: {info}");
    }));

    log::info!("android_main: starting");

    let event_loop = EventLoop::builder().with_android_app(app).build().unwrap();
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut application = App::default();
    event_loop.run_app(&mut application).unwrap();
}
