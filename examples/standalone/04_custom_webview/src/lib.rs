#![allow(clippy::disallowed_types, dead_code, static_mut_refs)]
pub mod css_engine;
pub mod gpu;
pub mod html_parser;
#[cfg(feature = "js")]
pub mod js_bridge;
pub mod layout;
pub mod renderer;
pub mod types;

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

#[cfg(any(target_os = "android", target_os = "ios"))]
pub mod embedded_assets {
    pub const INDEX_HTML: &str = include_str!("../assets/index.html");
    pub const TODO_HTML: &str = include_str!("../assets/todo.html");
    pub const ABOUT_HTML: &str = include_str!("../assets/about.html");
    pub const CANVAS_HTML: &str = include_str!("../assets/canvas.html");
    pub const CSS3D_HTML: &str = include_str!("../assets/css3d.html");

    pub fn get(name: &str) -> Option<&'static str> {
        match name {
            "index.html" => Some(INDEX_HTML),
            "todo.html" => Some(TODO_HTML),
            "about.html" => Some(ABOUT_HTML),
            "canvas.html" => Some(CANVAS_HTML),
            "css3d.html" => Some(CSS3D_HTML),
            _ => None,
        }
    }
}

pub struct WebviewState {
    pub gpu: GpuState,
    #[cfg(feature = "js")]
    pub js: js_bridge::JsBridge,
    pub layout: LayoutTree,
    pub static_commands: Vec<DrawCommand>,
    pub ghost_commands: Vec<DrawCommand>,
    pub clear_color: [f32; 4],
    pub html_source: String,
    pub css_sources: Vec<String>,
    pub styled_base: types::StyledNode,
    pub asset_dir: PathBuf,
    #[allow(dead_code)]
    pub start_time: Instant,
    pub scroll_y: f32,
    pub mouse_down: bool,
    pub text_cache: TextMeasureCache,
    pub ghost_pos: Option<(f32, f32)>,
    pub last_touch_y: Option<f32>,
}

#[derive(Default)]
pub struct App {
    pub state: Option<WebviewState>,
}

impl App {
    pub fn rebuild_layout(&mut self) {
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
        let all_commands = generate_draw_commands(&state.layout, &canvas_ops);
        let (s, g) = Self::split_commands(all_commands);
        state.static_commands = s;
        state.ghost_commands = g;
        state.clear_color = styled.style.background_color;
        state.ghost_pos = None;
        state.gpu.static_dirty = true;
        #[cfg(feature = "js")]
        {
            state
                .js
                .update_element_rects(state.layout.collect_element_rects());
            state.js.clear_dirty();
        }
    }

    pub fn full_rebuild(&mut self) {
        let state = self.state.as_mut().unwrap();
        let dom = parse_html(&state.html_source);
        state.styled_base = apply_styles(&dom, &state.css_sources);
        self.rebuild_layout();
    }

    #[cfg(feature = "js")]
    pub fn try_patch_position(&mut self) -> bool {
        let state = self.state.as_mut().unwrap();
        let overrides = state.js.style_overrides();
        let ghost_props = match overrides.get("ghost") {
            Some(props) => props,
            None => return false,
        };
        let mut new_left = None;
        let mut new_top = None;
        for (key, val) in ghost_props {
            match key.as_str() {
                "left" => new_left = val.strip_suffix("px").and_then(|v| v.parse::<f32>().ok()),
                "top" => new_top = val.strip_suffix("px").and_then(|v| v.parse::<f32>().ok()),
                _ => {}
            }
        }
        let (new_x, new_y) = match (new_left, new_top) {
            (Some(x), Some(y)) => (x, y),
            _ => return false,
        };
        if let Some((old_x, old_y)) = state.ghost_pos {
            let dx = new_x - old_x;
            let dy = new_y - old_y;
            if dx != 0.0 || dy != 0.0 {
                for cmd in &mut state.ghost_commands {
                    match cmd {
                        DrawCommand::Rect { rect, .. } | DrawCommand::Border { rect, .. } => {
                            rect.x += dx;
                            rect.y += dy;
                        }
                        DrawCommand::Text { x, y, .. } => {
                            *x += dx;
                            *y += dy;
                        }
                        _ => {}
                    }
                }
            }
            state.ghost_pos = Some((new_x, new_y));
            state.js.clear_dirty();
            true
        } else {
            state.ghost_pos = Some((new_x, new_y));
            false
        }
    }

    pub fn split_commands(commands: Vec<DrawCommand>) -> (Vec<DrawCommand>, Vec<DrawCommand>) {
        let mut static_cmds = Vec::new();
        let mut ghost_cmds = Vec::new();
        for cmd in commands {
            let is_ghost = match &cmd {
                DrawCommand::Rect { element_id, .. }
                | DrawCommand::Border { element_id, .. }
                | DrawCommand::Text { element_id, .. } => element_id
                    .as_deref()
                    .is_some_and(|id| id == "ghost" || id.starts_with("ghost-")),
                _ => false,
            };
            if is_ghost {
                ghost_cmds.push(cmd);
            } else {
                static_cmds.push(cmd);
            }
        }
        (static_cmds, ghost_cmds)
    }

    pub fn navigate(&mut self, href: &str) {
        let state = self.state.as_mut().unwrap();
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        let html_source = match std::fs::read_to_string(&state.asset_dir.join(href)) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Navigation failed: {e}");
                return;
            }
        };
        #[cfg(any(target_os = "android", target_os = "ios"))]
        let html_source = match embedded_assets::get(href) {
            Some(s) => s.to_string(),
            None => {
                eprintln!("Navigation failed: missing embedded asset {href}");
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
pub fn apply_text_overrides(
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
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        let window_attrs = window_attrs.with_inner_size(winit::dpi::PhysicalSize::new(1280, 720));

        let window = Arc::new(event_loop.create_window(window_attrs).unwrap());

        let mut gpu = pollster::block_on(GpuState::new(
            event_loop.owned_display_handle(),
            window.clone(),
        ))
        .unwrap();

        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        let html_source = {
            let asset_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets");
            let html_file = std::env::args()
                .nth(1)
                .unwrap_or_else(|| "index.html".to_string());
            std::fs::read_to_string(asset_dir.join(&html_file)).expect("failed to read HTML file")
        };
        #[cfg(any(target_os = "android", target_os = "ios"))]
        let html_source = embedded_assets::get("index.html")
            .unwrap_or(embedded_assets::INDEX_HTML)
            .to_string();

        let css_sources = extract_styles(&html_source);

        #[cfg(feature = "js")]
        let js = {
            let script = html_parser::extract_script(&html_source);
            match js_bridge::JsBridge::new(script.as_deref()) {
                Ok(js) => js,
                Err(e) => {
                    log::error!("failed to init JS bridge: {:?}", e);
                    panic!("failed to init JS bridge: {:?}", e);
                }
            }
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
        let all_commands = generate_draw_commands(&layout_tree, &canvas_ops);
        let (static_commands, ghost_commands) = Self::split_commands(all_commands);

        let clear_color = styled.style.background_color;

        self.state = Some(WebviewState {
            gpu,
            #[cfg(feature = "js")]
            js,
            layout: layout_tree,
            static_commands,
            ghost_commands,
            clear_color,
            html_source,
            css_sources,
            styled_base,
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            asset_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets"),
            #[cfg(any(target_os = "android", target_os = "ios"))]
            asset_dir: PathBuf::new(),
            start_time: Instant::now(),
            scroll_y: 0.0,
            mouse_down: false,
            text_cache,
            ghost_pos: None,
            last_touch_y: None,
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
                        let s = self.state.as_mut().unwrap();
                        s.mouse_down = true;
                        s.last_touch_y = Some(my);

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
                            }
                        }
                        self.state.as_ref().unwrap().gpu.window.request_redraw();
                    }
                    winit::event::TouchPhase::Moved => {
                        let s = self.state.as_mut().unwrap();
                        if s.ghost_commands.is_empty() {
                            if let Some(last_y) = s.last_touch_y {
                                let dy = last_y - my;
                                let max_scroll = (s.layout.content_height()
                                    - s.gpu.size.height as f32 / s.gpu.scale_factor as f32)
                                    .max(0.0);
                                s.scroll_y = (s.scroll_y + dy).clamp(0.0, max_scroll);
                            }
                        }
                        s.last_touch_y = Some(my);

                        #[cfg(feature = "js")]
                        {
                            let s = self.state.as_mut().unwrap();
                            let y = my + s.scroll_y;
                            s.js.dispatch_pointer_move(&s.layout, mx, y);
                            if s.js.is_dirty() && !self.try_patch_position() {
                                self.rebuild_layout();
                            }
                        }
                        self.state.as_ref().unwrap().gpu.window.request_redraw();
                    }
                    winit::event::TouchPhase::Ended | winit::event::TouchPhase::Cancelled => {
                        let s = self.state.as_mut().unwrap();
                        s.mouse_down = false;
                        s.last_touch_y = None;
                        #[cfg(feature = "js")]
                        {
                            let s = self.state.as_mut().unwrap();
                            let y = my + s.scroll_y;
                            s.js.dispatch_pointer_up(&s.layout, mx, y);
                            s.ghost_pos = None;
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
                    let is_dragging = self.state.as_ref().map(|s| s.mouse_down).unwrap_or(false);
                    if self.state.as_ref().unwrap().js.is_dirty()
                        && !(is_dragging && self.try_patch_position())
                    {
                        self.rebuild_layout();
                    }
                }
                let s = self.state.as_mut().unwrap();
                s.gpu.render(
                    &s.static_commands,
                    &s.ghost_commands,
                    s.clear_color,
                    s.scroll_y,
                );
                s.gpu.window.request_redraw();
            }
            _ => {}
        }
    }
}

pub static mut CURSOR_POS: (f32, f32) = (0.0, 0.0);

pub fn run() {
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
