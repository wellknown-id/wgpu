#![allow(clippy::disallowed_types)]
mod gpu_parsers;
mod gpu_state;
mod host_api;
mod js_engine;
mod ui_overlay;

use std::{cell::RefCell, path::PathBuf, rc::Rc, sync::Arc, time::Instant};

use bytemuck::{Pod, Zeroable};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowId},
};

use gpu_state::GpuState;
use js_engine::JsEngine;

pub(crate) type JsResult<T> = std::result::Result<T, rquickjs::Error>;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct Vertex {
    position: [f32; 3],
    normal: [f32; 3],
}

impl Vertex {
    const ATTRS: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];

    pub(crate) fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRS,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct InstanceRaw {
    matrix: [f32; 16],
}

impl InstanceRaw {
    const ATTRS: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
        2 => Float32x4,
        3 => Float32x4,
        4 => Float32x4,
        5 => Float32x4
    ];

    pub(crate) fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRS,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct CameraRaw {
    pub(crate) view_proj: [f32; 16],
}

#[derive(Clone)]
pub(crate) struct GeometryUpload {
    pub(crate) positions: Vec<f32>,
    pub(crate) normals: Vec<f32>,
    pub(crate) indices: Vec<u32>,
}

#[derive(Clone)]
pub(crate) struct FrameUpload {
    pub(crate) projection_matrix: [f32; 16],
    pub(crate) view_matrix: [f32; 16],
    pub(crate) world_matrix: [f32; 16],
    pub(crate) instance_matrices: Vec<f32>,
    pub(crate) count: u32,
    pub(crate) clear_color: [f32; 4],
}

#[derive(Default)]
struct App {
    gpu: Option<Rc<RefCell<GpuState>>>,
    js: Option<JsEngine>,
    start_time: Option<Instant>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("QuickJS HTML Renderer")
                        .with_inner_size(winit::dpi::PhysicalSize::new(1280, 720)),
                )
                .unwrap(),
        );

        let gpu = Rc::new(RefCell::new(
            pollster::block_on(GpuState::new(
                event_loop.owned_display_handle(),
                window.clone(),
            ))
            .unwrap(),
        ));

        let asset_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets");
        let arg = std::env::args()
            .nth(1)
            .unwrap_or_else(|| "index.html".to_string());

        let (filename, search) = if let Some(idx) = arg.find('?') {
            let search = format!("?{}", &arg[idx + 1..]);
            let filename = arg[..idx].to_string();
            (filename, search)
        } else {
            (arg, String::new())
        };

        let js = JsEngine::new(asset_dir, gpu.clone(), 1280, 720, &filename, &search).unwrap();

        self.start_time = Some(Instant::now());
        self.gpu = Some(gpu);
        self.js = Some(js);
        window.request_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        let Some(js) = self.js.as_mut() else {
            return;
        };

        match event {
            WindowEvent::CursorMoved { position, .. } => {
                gpu.borrow_mut().mouse = (position.x as f32, position.y as f32);
                let _ = js.dispatch_mouse_event("pointermove", position.x, position.y);
            }
            WindowEvent::MouseInput {
                state,
                button: winit::event::MouseButton::Left,
                ..
            } => {
                let ev_type = if state == winit::event::ElementState::Pressed {
                    "pointerdown"
                } else {
                    "pointerup"
                };
                let mouse = gpu.borrow().mouse;
                let _ = js.dispatch_mouse_event(ev_type, mouse.0 as f64, mouse.1 as f64);
            }
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                gpu.borrow_mut().resize(size);
                js.resize(size.width, size.height).unwrap();
                gpu.borrow().window().request_redraw();
            }
            WindowEvent::RedrawRequested => {
                if let Some(start_time) = self.start_time {
                    let ms = start_time.elapsed().as_secs_f64() * 1000.0;
                    js.set_time_ms(ms);
                }

                if let Err(err) = js.tick() {
                    eprintln!("QuickJS tick failed: {err:#}");
                    event_loop.exit();
                    return;
                }
                gpu.borrow_mut().present_submitted_surface_textures();

                if let Some(geometry) = js.take_geometry() {
                    gpu.borrow_mut().upload_geometry(geometry).unwrap();
                }
                gpu.borrow_mut().render(js.take_frame());
                gpu.borrow().window().request_redraw();
            }
            _ => {}
        }
    }
}

fn main() {
    env_logger::init();

    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::default();
    event_loop.run_app(&mut app).unwrap();
}
