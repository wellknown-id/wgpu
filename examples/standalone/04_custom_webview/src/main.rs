#![allow(clippy::disallowed_types, dead_code, static_mut_refs)]

mod css_engine;
mod gpu;
mod html_parser;
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

use css_engine::apply_styles;
use gpu::GpuState;
use html_parser::{extract_script, extract_styles, parse_html};
use js_bridge::JsBridge;
use layout::{build_layout, LayoutTree};
use renderer::generate_draw_commands;
use types::DrawCommand;

struct WebviewState {
    gpu: GpuState,
    js: JsBridge,
    layout: LayoutTree,
    commands: Vec<DrawCommand>,
    clear_color: [f32; 4],
    html_source: String,
    css_sources: Vec<String>,
    start_time: Instant,
}

#[derive(Default)]
struct App {
    state: Option<WebviewState>,
}

impl App {
    fn rebuild_layout(&mut self) {
        let state = self.state.as_mut().unwrap();
        let dom = parse_html(&state.html_source);
        let mut styled = apply_styles(&dom, &state.css_sources);

        let overrides = state.js.text_overrides().clone();
        apply_text_overrides(&mut styled, &overrides);

        let size = state.gpu.size;
        state.layout = build_layout(&styled, size.width as f32, size.height as f32);
        state.commands = generate_draw_commands(&state.layout);
        state.js.clear_dirty();
    }
}

fn apply_text_overrides(
    styled: &mut types::StyledNode,
    overrides: &std::collections::HashMap<String, String>,
) {
    if let Some(id) = &styled.dom_node.id {
        if let Some(text) = overrides.get(id) {
            styled.children.clear();
            styled.dom_node.text = text.clone();
            styled.dom_node.children.clear();
        }
    }
    for child in &mut styled.children {
        apply_text_overrides(child, overrides);
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("Custom Webview - wgpu")
                        .with_inner_size(winit::dpi::PhysicalSize::new(1280, 720)),
                )
                .unwrap(),
        );

        let gpu = pollster::block_on(GpuState::new(
            event_loop.owned_display_handle(),
            window.clone(),
        ))
        .unwrap();

        let asset_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets");
        let html_file = std::env::args()
            .nth(1)
            .unwrap_or_else(|| "index.html".to_string());
        let html_source =
            std::fs::read_to_string(asset_dir.join(&html_file)).expect("failed to read HTML file");

        let css_sources = extract_styles(&html_source);
        let script = extract_script(&html_source);

        let js = JsBridge::new(script.as_deref()).expect("failed to init JS bridge");

        let dom = parse_html(&html_source);
        let mut styled = apply_styles(&dom, &css_sources);
        let overrides = js.text_overrides().clone();
        apply_text_overrides(&mut styled, &overrides);

        let size = gpu.size;
        let layout_tree = build_layout(&styled, size.width as f32, size.height as f32);
        let commands = generate_draw_commands(&layout_tree);

        let clear_color = styled.style.background_color;

        self.state = Some(WebviewState {
            gpu,
            js,
            layout: layout_tree,
            commands,
            clear_color,
            html_source,
            css_sources,
            start_time: Instant::now(),
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
                s.js.dispatch_click(&s.layout, mx, my);
                if s.js.is_dirty() {
                    self.rebuild_layout();
                    self.state.as_ref().unwrap().gpu.window.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => unsafe {
                CURSOR_POS = (position.x as f32, position.y as f32);
            },
            WindowEvent::RedrawRequested => {
                let s = self.state.as_mut().unwrap();
                let now_ms = s.start_time.elapsed().as_secs_f64() * 1000.0;
                s.js.tick(now_ms);
                let needs_rebuild = s.js.is_dirty();
                if needs_rebuild {
                    self.rebuild_layout();
                }
                let s = self.state.as_mut().unwrap();
                s.gpu.render(&s.commands, s.clear_color);
                s.gpu.window.request_redraw();
            }
            _ => {}
        }
    }
}

static mut CURSOR_POS: (f32, f32) = (0.0, 0.0);

fn main() {
    env_logger::init();

    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::default();
    event_loop.run_app(&mut app).unwrap();
}
