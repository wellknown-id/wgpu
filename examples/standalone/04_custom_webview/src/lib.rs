#![allow(clippy::disallowed_types, dead_code, static_mut_refs)]
pub mod css_engine;
pub mod gpu;
pub mod html_parser;
#[cfg(feature = "js")]
pub mod js_bridge;
pub mod layout;
pub mod renderer;
pub mod slug;
pub mod types;
#[cfg(feature = "js")]
pub mod webgpu_bridge;
#[cfg(feature = "xr")]
pub mod xr_session;

use std::path::PathBuf;
#[cfg(target_os = "android")]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

#[cfg(feature = "xr")]
use openxr as xr;
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
    pub const WEBGPU_HTML: &str = include_str!("../assets/webgpu.html");
    pub const WEBXR_HTML: &str = include_str!("../assets/webxr.html");
    pub const POINTERS_HTML: &str = include_str!("../assets/pointers.html");

    pub fn get(name: &str) -> Option<&'static str> {
        match name {
            "index.html" => Some(INDEX_HTML),
            "todo.html" => Some(TODO_HTML),
            "about.html" => Some(ABOUT_HTML),
            "canvas.html" => Some(CANVAS_HTML),
            "css3d.html" => Some(CSS3D_HTML),
            "webgpu.html" => Some(WEBGPU_HTML),
            "webxr.html" => Some(WEBXR_HTML),
            "pointers.html" => Some(POINTERS_HTML),
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
    pub cmd_index: std::collections::HashMap<String, Vec<usize>>,
    pub clear_color: [f32; 4],
    pub current_asset: String,
    pub html_source: String,
    pub css_sources: Vec<String>,
    pub styled_base: types::StyledNode,
    pub asset_dir: PathBuf,
    #[allow(dead_code)]
    pub start_time: Instant,
    pub view_size: winit::dpi::PhysicalSize<u32>,
    pub view_scale_factor: f64,
    pub scroll_y: f32,
    pub mouse_buttons: i32,
    pub text_cache: TextMeasureCache,
    pub ghost_pos: Option<(f32, f32)>,
    pub prev_style_positions: std::collections::HashMap<String, (f32, f32)>,
    pub last_touch_y: Option<f32>,
    pub touch_default_prevented: bool,
    #[cfg(feature = "xr")]
    pub xr_context: Option<xr_session::XrContext>,
    #[cfg(feature = "xr")]
    pub xr_session: Option<xr_session::XrSession>,
    #[cfg(feature = "xr")]
    pub xr_session_failed: bool,
    #[cfg(feature = "xr")]
    pub xr_depth_texture: Option<wgpu::Texture>,
    #[cfg(feature = "xr")]
    pub xr_depth_size: (u32, u32),
    #[cfg(feature = "xr")]
    pub xr_panel_depth_texture: Option<wgpu::Texture>,
    #[cfg(feature = "xr")]
    pub xr_panel_depth_size: (u32, u32),
    #[cfg(feature = "xr")]
    pub xr_native_msaa_color_texture: Option<wgpu::Texture>,
    #[cfg(feature = "xr")]
    pub xr_native_msaa_depth_texture: Option<wgpu::Texture>,
    #[cfg(feature = "xr")]
    pub xr_native_msaa_size: (u32, u32),
    #[cfg(feature = "xr")]
    pub xr_panel_target: Option<gpu::OffscreenRenderTarget>,
    #[cfg(feature = "xr")]
    pub xr_promoted_targets: std::collections::HashMap<String, gpu::OffscreenRenderTarget>,
    #[cfg(feature = "xr")]
    pub page_xr_active: bool,
    #[cfg(feature = "xr")]
    pub xr_right_select_down: bool,
    #[cfg(feature = "xr")]
    pub xr_left_select_down: bool,
    #[cfg(feature = "xr")]
    pub xr_right_pointer_pos: Option<(f32, f32)>,
    #[cfg(feature = "xr")]
    pub xr_left_pointer_pos: Option<(f32, f32)>,
    #[cfg(feature = "xr")]
    pub xr_exit_down: bool,
}

#[derive(Default)]
pub struct App {
    pub state: Option<WebviewState>,
}

#[cfg(all(feature = "xr", target_os = "android"))]
struct XrHybridFramePlan {
    native_static_commands: Vec<DrawCommand>,
    overlay_static_commands: Vec<DrawCommand>,
    native_ghost_commands: Vec<DrawCommand>,
    overlay_ghost_commands: Vec<DrawCommand>,
}

#[cfg(all(feature = "xr", target_os = "android"))]
impl XrHybridFramePlan {
    fn overlay_has_content(&self) -> bool {
        !self.overlay_static_commands.is_empty() || !self.overlay_ghost_commands.is_empty()
    }
}

#[cfg(feature = "xr")]
struct XrNativeFramePlan {
    native_static_commands: Vec<DrawCommand>,
    native_ghost_commands: Vec<DrawCommand>,
    promoted_layers: Vec<gpu::XrTextureLayer>,
}

#[cfg(feature = "xr")]
struct XrPromotedLayerCandidate {
    key: String,
    commands: Vec<DrawCommand>,
    bounds: types::LayoutRect,
    transform: [f32; 16],
    is_fixed: bool,
    contains_text: bool,
    draw_order: f32,
    first_order: usize,
}

#[cfg(target_os = "android")]
static ANDROID_ACTIVITY_RESUMED: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "android")]
static ANDROID_ACTIVITY_FOCUSED: AtomicBool = AtomicBool::new(false);

fn asset_name(href: &str) -> &str {
    href.split('?')
        .next()
        .unwrap_or(href)
        .split('#')
        .next()
        .unwrap_or(href)
}

#[cfg(any(target_os = "android", target_os = "ios"))]
fn mobile_html_source(href: &str) -> String {
    let asset = asset_name(href);
    match embedded_assets::get(asset) {
        Some(source) => source.to_string(),
        None => {
            log::error!("Missing embedded asset {asset}, falling back to index.html");
            embedded_assets::INDEX_HTML.to_string()
        }
    }
}

#[cfg(feature = "xr")]
const XR_PANEL_DISTANCE: f32 = 1.4;
#[cfg(feature = "xr")]
const XR_PANEL_WIDTH: f32 = 2.0;
#[cfg(feature = "xr")]
const XR_PANEL_LOGICAL_WIDTH: u32 = 1920;
#[cfg(feature = "xr")]
const XR_PANEL_LOGICAL_HEIGHT: u32 = 1080;
#[cfg(feature = "xr")]
const XR_PANEL_RENDER_WIDTH: u32 = 3840;
#[cfg(feature = "xr")]
const XR_PANEL_RENDER_HEIGHT: u32 = 2160;
#[cfg(feature = "xr")]
const XR_NATIVE_MSAA_SAMPLES: u32 = 4;
#[cfg(feature = "xr")]
const XR_NATIVE_TEXT_RENDER_SCALE: f32 = 4.0;
#[cfg(feature = "xr")]
const XR_PROMOTED_LAYER_SCALE_BOOST: f32 = 1.5;
#[cfg(feature = "xr")]
const XR_SCROLL_SPEED: f32 = 14.0;
#[cfg(all(feature = "xr", target_os = "android"))]
const XR_POINTER_YAW_BIAS_DEGREES: f32 = 5.0;
#[cfg(all(feature = "xr", target_os = "android"))]
const XR_POINTER_PITCH_BIAS_DEGREES: f32 = 0.0;

#[cfg(feature = "xr")]
fn xr_panel_view_size() -> winit::dpi::PhysicalSize<u32> {
    winit::dpi::PhysicalSize::new(XR_PANEL_LOGICAL_WIDTH, XR_PANEL_LOGICAL_HEIGHT)
}

#[cfg(feature = "xr")]
fn xr_panel_render_size() -> winit::dpi::PhysicalSize<u32> {
    winit::dpi::PhysicalSize::new(XR_PANEL_RENDER_WIDTH, XR_PANEL_RENDER_HEIGHT)
}

#[cfg(feature = "xr")]
fn xr_panel_render_scale_factor() -> f32 {
    XR_PANEL_RENDER_WIDTH as f32 / XR_PANEL_LOGICAL_WIDTH as f32
}

#[cfg(feature = "xr")]
fn xr_native_render_scale_factor(base_scale: f32) -> f32 {
    base_scale.max(XR_NATIVE_TEXT_RENDER_SCALE)
}

#[cfg(feature = "xr")]
fn xr_promoted_layer_render_scale_factor(base_scale: f32) -> f32 {
    xr_native_render_scale_factor(base_scale) * XR_PROMOTED_LAYER_SCALE_BOOST
}

#[cfg(feature = "xr")]
fn xr_panel_height(target_size: winit::dpi::PhysicalSize<u32>) -> f32 {
    XR_PANEL_WIDTH * target_size.height as f32 / target_size.width.max(1) as f32
}

#[cfg(feature = "xr")]
fn xr_panel_layer_size(target_size: winit::dpi::PhysicalSize<u32>) -> xr::Extent2Df {
    xr::Extent2Df {
        width: XR_PANEL_WIDTH,
        height: xr_panel_height(target_size),
    }
}

#[cfg(feature = "xr")]
fn xr_panel_local_pose() -> xr::Posef {
    xr::Posef {
        orientation: xr::Quaternionf::IDENTITY,
        position: xr::Vector3f {
            x: 0.0,
            y: 0.0,
            z: -XR_PANEL_DISTANCE,
        },
    }
}

#[cfg(feature = "xr")]
fn ensure_xr_native_msaa_targets(state: &mut WebviewState, width: u32, height: u32) {
    if state.xr_native_msaa_color_texture.is_some()
        && state.xr_native_msaa_depth_texture.is_some()
        && state.xr_native_msaa_size == (width, height)
    {
        return;
    }

    let color = state.gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("xr native msaa color"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: XR_NATIVE_MSAA_SAMPLES,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth = state.gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("xr native msaa depth"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: XR_NATIVE_MSAA_SAMPLES,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    state.xr_native_msaa_color_texture = Some(color);
    state.xr_native_msaa_depth_texture = Some(depth);
    state.xr_native_msaa_size = (width, height);
}

#[cfg(all(feature = "xr", target_os = "android"))]
fn clear_xr_projection_views(
    gpu: &GpuState,
    left_view: &wgpu::TextureView,
    right_view: &wgpu::TextureView,
    clear_color: [f32; 4],
) {
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("xr blank projection encoder"),
        });
    for view in [left_view, right_view] {
        let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("xr blank projection pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: clear_color[0] as f64,
                        g: clear_color[1] as f64,
                        b: clear_color[2] as f64,
                        a: clear_color[3] as f64,
                    }),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    gpu.queue.submit(std::iter::once(encoder.finish()));
}

#[cfg(feature = "xr")]
fn xr_panel_model_matrix(target_size: winit::dpi::PhysicalSize<u32>) -> [f32; 16] {
    xr_panel_model_matrix_at(target_size, 0.0, 0.0, -XR_PANEL_DISTANCE, 1.0)
}

#[cfg(feature = "xr")]
fn xr_panel_pose_matrix() -> [f32; 16] {
    let pose = xr_panel_local_pose();
    types::mat4_translate(
        &types::mat4_identity(),
        pose.position.x,
        pose.position.y,
        pose.position.z,
    )
}

#[cfg(feature = "xr")]
fn xr_panel_model_matrix_at(
    target_size: winit::dpi::PhysicalSize<u32>,
    x: f32,
    y: f32,
    z: f32,
    scale_factor: f32,
) -> [f32; 16] {
    let mut scale = types::mat4_identity();
    scale[0] = XR_PANEL_WIDTH * scale_factor;
    scale[5] = xr_panel_height(target_size) * scale_factor;
    let translate = types::mat4_translate(&types::mat4_identity(), x, y, z);
    types::mat4_mul(&translate, &scale)
}

#[cfg(feature = "xr")]
fn xr_panel_mvp(
    view: &crate::xr_session::XrEyeView,
    target_size: winit::dpi::PhysicalSize<u32>,
) -> [f32; 16] {
    let model = xr_panel_model_matrix(target_size);
    let view_model = types::mat4_mul(&view.view_matrix, &model);
    types::mat4_mul(&view.projection_matrix, &view_model)
}

#[cfg(feature = "xr")]
fn xr_native_page_mvp(view: &crate::xr_session::XrEyeView) -> [f32; 16] {
    let model = xr_panel_pose_matrix();
    let view_model = types::mat4_mul(&view.view_matrix, &model);
    types::mat4_mul(&view.projection_matrix, &view_model)
}

#[cfg(feature = "xr")]
fn xr_panel_mvp_at(
    view: &crate::xr_session::XrEyeView,
    target_size: winit::dpi::PhysicalSize<u32>,
    x: f32,
    y: f32,
    z: f32,
    scale_factor: f32,
) -> [f32; 16] {
    let model = xr_panel_model_matrix_at(target_size, x, y, z, scale_factor);
    let view_model = types::mat4_mul(&view.view_matrix, &model);
    types::mat4_mul(&view.projection_matrix, &view_model)
}

#[cfg(feature = "xr")]
fn rotate_vec3(q: &xr::Quaternionf, v: [f32; 3]) -> [f32; 3] {
    let u = [q.x, q.y, q.z];
    let uv = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let uuv = [
        u[1] * uv[2] - u[2] * uv[1],
        u[2] * uv[0] - u[0] * uv[2],
        u[0] * uv[1] - u[1] * uv[0],
    ];
    [
        v[0] + 2.0 * (q.w * uv[0] + uuv[0]),
        v[1] + 2.0 * (q.w * uv[1] + uuv[1]),
        v[2] + 2.0 * (q.w * uv[2] + uuv[2]),
    ]
}

#[cfg(feature = "xr")]
fn quat_normalize(q: xr::Quaternionf) -> xr::Quaternionf {
    let len = (q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w).sqrt();
    if len <= 1e-6 {
        xr::Quaternionf::IDENTITY
    } else {
        xr::Quaternionf {
            x: q.x / len,
            y: q.y / len,
            z: q.z / len,
            w: q.w / len,
        }
    }
}

#[cfg(feature = "xr")]
fn quat_conjugate(q: &xr::Quaternionf) -> xr::Quaternionf {
    xr::Quaternionf {
        x: -q.x,
        y: -q.y,
        z: -q.z,
        w: q.w,
    }
}

#[cfg(feature = "xr")]
fn quat_mul(a: &xr::Quaternionf, b: &xr::Quaternionf) -> xr::Quaternionf {
    quat_normalize(xr::Quaternionf {
        x: a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
        y: a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
        z: a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
        w: a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
    })
}

#[cfg(feature = "xr")]
fn quat_from_axis_angle(axis: [f32; 3], angle: f32) -> xr::Quaternionf {
    let axis_len = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
    if axis_len <= 1e-6 {
        return xr::Quaternionf::IDENTITY;
    }
    let half = angle * 0.5;
    let scale = half.sin() / axis_len;
    quat_normalize(xr::Quaternionf {
        x: axis[0] * scale,
        y: axis[1] * scale,
        z: axis[2] * scale,
        w: half.cos(),
    })
}

#[cfg(feature = "xr")]
fn pointer_orientation(pose: &xr::Posef) -> xr::Quaternionf {
    #[cfg(target_os = "android")]
    {
        let yaw_correction =
            quat_from_axis_angle([0.0, 1.0, 0.0], XR_POINTER_YAW_BIAS_DEGREES.to_radians());
        let pitch_correction =
            quat_from_axis_angle([1.0, 0.0, 0.0], XR_POINTER_PITCH_BIAS_DEGREES.to_radians());
        let local_correction = quat_mul(&yaw_correction, &pitch_correction);
        return quat_mul(&pose.orientation, &local_correction);
    }
    #[cfg(not(target_os = "android"))]
    {
        pose.orientation
    }
}

#[cfg(feature = "xr")]
fn xr_panel_pointer(
    pose: &xr::Posef,
    panel_pose: &xr::Posef,
    target_size: winit::dpi::PhysicalSize<u32>,
    scale_factor: f64,
) -> Option<(f32, f32)> {
    let inv_panel_orientation = quat_conjugate(&panel_pose.orientation);
    let origin = rotate_vec3(
        &inv_panel_orientation,
        [
            pose.position.x - panel_pose.position.x,
            pose.position.y - panel_pose.position.y,
            pose.position.z - panel_pose.position.z,
        ],
    );
    let pointer_orientation = pointer_orientation(pose);
    let dir = rotate_vec3(
        &inv_panel_orientation,
        rotate_vec3(&pointer_orientation, [0.0, 0.0, -1.0]),
    );
    if dir[2].abs() < 1e-4 {
        return None;
    }
    let t = -origin[2] / dir[2];
    if t <= 0.0 {
        return None;
    }
    let (hit_x, hit_y) = (origin[0] + dir[0] * t, origin[1] + dir[1] * t);
    let panel_half_width = XR_PANEL_WIDTH * 0.5;
    let panel_half_height = xr_panel_height(target_size) * 0.5;
    if hit_x.abs() > panel_half_width || hit_y.abs() > panel_half_height {
        return None;
    }
    let logical_width = target_size.width as f32 / scale_factor as f32;
    let logical_height = target_size.height as f32 / scale_factor as f32;
    let u = hit_x / XR_PANEL_WIDTH + 0.5;
    let v = 0.5 - hit_y / (panel_half_height * 2.0);
    Some((u * logical_width, v * logical_height))
}

impl App {
    fn target_view_size(&self, _gpu: &GpuState) -> winit::dpi::PhysicalSize<u32> {
        #[cfg(all(target_os = "android", feature = "xr"))]
        {
            xr_panel_view_size()
        }
        #[cfg(not(all(target_os = "android", feature = "xr")))]
        {
            _gpu.size
        }
    }

    fn target_view_scale_factor(&self, _gpu: &GpuState) -> f64 {
        #[cfg(all(target_os = "android", feature = "xr"))]
        {
            1.0
        }
        #[cfg(not(all(target_os = "android", feature = "xr")))]
        {
            _gpu.scale_factor
        }
    }

    pub fn rebuild_layout(&mut self) {
        let state = self.state.as_mut().unwrap();
        Self::rebuild_layout_state(state);
    }

    fn rebuild_layout_state(state: &mut WebviewState) {
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

        let size = state.view_size;
        let scale = state.view_scale_factor as f32;
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
        state.cmd_index = Self::build_cmd_index(&state.static_commands, &state.ghost_commands);
        state.clear_color = styled.style.background_color;
        state.ghost_pos = None;
        state.prev_style_positions.clear();
        state.gpu.static_dirty = true;
        #[cfg(feature = "js")]
        {
            state
                .js
                .update_element_rects(state.layout.collect_element_rects());
            state.js.set_scroll_y(state.scroll_y);
            let style_ov = state.js.style_overrides();
            for (id, props) in &style_ov {
                let left = props
                    .iter()
                    .find(|(k, _)| k == "left")
                    .and_then(|(_, v)| css_engine::parse_length(v));
                let top = props
                    .iter()
                    .find(|(k, _)| k == "top")
                    .and_then(|(_, v)| css_engine::parse_length(v));
                if let (Some(l), Some(t)) = (left, top) {
                    state.prev_style_positions.insert(id.clone(), (l, t));
                }
            }
            state.js.clear_dirty();
        }
    }

    fn max_scroll(state: &WebviewState) -> f32 {
        (state.layout.content_height()
            - state.view_size.height as f32 / state.view_scale_factor as f32)
            .max(0.0)
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

    #[cfg(feature = "js")]
    pub fn try_patch_visual(&mut self) -> bool {
        let state = self.state.as_mut().unwrap();
        if !state.js.is_visual_only_dirty() {
            return false;
        }
        let dirty_elements = state.js.dirty_elements();

        struct PatchData {
            elem_id: String,
            new_left: Option<f32>,
            new_top: Option<f32>,
            new_bg: Option<[f32; 4]>,
            new_text: Option<String>,
        }

        let patches: Vec<PatchData> = {
            let style_ov = state.js.style_overrides_ref();
            let text_ov = state.js.text_overrides();
            dirty_elements
                .iter()
                .map(|elem_id| {
                    let (new_left, new_top, new_bg) = if let Some(props) = style_ov.get(elem_id) {
                        (
                            props.get("left").and_then(|v| css_engine::parse_length(v)),
                            props.get("top").and_then(|v| css_engine::parse_length(v)),
                            props
                                .get("background-color")
                                .and_then(|v| css_engine::parse_color(v)),
                        )
                    } else {
                        (None, None, None)
                    };
                    let new_text = text_ov.get(elem_id).cloned();
                    PatchData {
                        elem_id: elem_id.clone(),
                        new_left,
                        new_top,
                        new_bg,
                        new_text,
                    }
                })
                .collect()
        };

        let static_len = state.static_commands.len();

        for p in &patches {
            let prev = state.prev_style_positions.get(&p.elem_id).copied();
            let (dx, dy) = match (p.new_left, p.new_top, prev) {
                (Some(nx), Some(ny), Some((ox, oy))) => (nx - ox, ny - oy),
                _ => (0.0, 0.0),
            };

            if p.new_left.is_some() || p.new_top.is_some() {
                state.prev_style_positions.insert(
                    p.elem_id.clone(),
                    (
                        p.new_left.unwrap_or(prev.map(|v| v.0).unwrap_or(0.0)),
                        p.new_top.unwrap_or(prev.map(|v| v.1).unwrap_or(0.0)),
                    ),
                );
            }

            let has_position_change = dx != 0.0 || dy != 0.0;

            if has_position_change {
                let mut all_indices: Vec<usize> = Vec::new();
                if let Some(indices) = state.cmd_index.get(&p.elem_id) {
                    all_indices.extend(indices);
                }
                for desc_id in state.layout.collect_descendant_ids(&p.elem_id) {
                    if let Some(indices) = state.cmd_index.get(&desc_id) {
                        all_indices.extend(indices);
                    }
                }
                for &idx in &all_indices {
                    let cmd = if idx < static_len {
                        &mut state.static_commands[idx]
                    } else {
                        &mut state.ghost_commands[idx - static_len]
                    };
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

            if p.new_bg.is_some() || p.new_text.is_some() {
                if let Some(indices) = state.cmd_index.get(&p.elem_id).cloned() {
                    for idx in &indices {
                        let cmd = if *idx < static_len {
                            &mut state.static_commands[*idx]
                        } else {
                            &mut state.ghost_commands[*idx - static_len]
                        };

                        if let Some(bg) = p.new_bg {
                            if let DrawCommand::Rect { color, .. } = cmd {
                                *color = bg;
                            }
                        }

                        if let Some(ref new_text) = p.new_text {
                            if let DrawCommand::Text { text, .. } = cmd {
                                *text = new_text.clone();
                            }
                        }
                    }
                }
            }
        }

        state.gpu.static_dirty = true;
        state.js.clear_dirty();
        true
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

    #[cfg(feature = "xr")]
    fn command_uses_xr_native_transform(command: &DrawCommand) -> bool {
        let identity = types::mat4_identity();
        match command {
            DrawCommand::Rect { transform, .. }
            | DrawCommand::Border { transform, .. }
            | DrawCommand::Text { transform, .. } => *transform != identity,
            DrawCommand::Line { .. } => false,
        }
    }

    #[cfg(feature = "xr")]
    fn should_use_xr_native_page(
        static_commands: &[DrawCommand],
        ghost_commands: &[DrawCommand],
        has_canvases: bool,
    ) -> bool {
        !has_canvases
            && static_commands
                .iter()
                .chain(ghost_commands.iter())
                .any(Self::command_uses_xr_native_transform)
    }

    #[cfg(all(feature = "xr", target_os = "android"))]
    fn build_xr_hybrid_frame_plan(
        static_commands: &[DrawCommand],
        ghost_commands: &[DrawCommand],
    ) -> XrHybridFramePlan {
        const XR_TEXT_OVERLAY_MAX_AREA: f32 = 200_000.0;

        fn promote_to_overlay(command: &DrawCommand) -> bool {
            let identity = types::mat4_identity();
            match command {
                DrawCommand::Text { transform, .. } => *transform == identity,
                DrawCommand::Rect {
                    rect, transform, ..
                }
                | DrawCommand::Border {
                    rect, transform, ..
                } => *transform == identity && rect.w * rect.h <= XR_TEXT_OVERLAY_MAX_AREA,
                DrawCommand::Line { .. } => false,
            }
        }

        let mut native_static = Vec::new();
        let mut overlay_static = Vec::new();
        for command in static_commands {
            if promote_to_overlay(command) {
                overlay_static.push(command.clone());
            } else {
                native_static.push(command.clone());
            }
        }

        let mut native_ghost = Vec::new();
        let mut overlay_ghost = Vec::new();
        for command in ghost_commands {
            if promote_to_overlay(command) {
                overlay_ghost.push(command.clone());
            } else {
                native_ghost.push(command.clone());
            }
        }

        XrHybridFramePlan {
            native_static_commands: native_static,
            overlay_static_commands: overlay_static,
            native_ghost_commands: native_ghost,
            overlay_ghost_commands: overlay_ghost,
        }
    }

    #[cfg(feature = "xr")]
    fn command_element_id(command: &DrawCommand) -> Option<&str> {
        match command {
            DrawCommand::Rect { element_id, .. }
            | DrawCommand::Border { element_id, .. }
            | DrawCommand::Text { element_id, .. } => element_id.as_deref(),
            DrawCommand::Line { .. } => None,
        }
    }

    #[cfg(feature = "xr")]
    fn command_transform(command: &DrawCommand) -> Option<[f32; 16]> {
        match command {
            DrawCommand::Rect { transform, .. }
            | DrawCommand::Border { transform, .. }
            | DrawCommand::Text { transform, .. } => Some(*transform),
            DrawCommand::Line { .. } => None,
        }
    }

    #[cfg(feature = "xr")]
    fn command_is_fixed(command: &DrawCommand) -> bool {
        match command {
            DrawCommand::Rect { is_fixed, .. }
            | DrawCommand::Border { is_fixed, .. }
            | DrawCommand::Text { is_fixed, .. } => *is_fixed,
            DrawCommand::Line { .. } => false,
        }
    }

    #[cfg(feature = "xr")]
    fn union_rect(a: &types::LayoutRect, b: &types::LayoutRect) -> types::LayoutRect {
        let min_x = a.x.min(b.x);
        let min_y = a.y.min(b.y);
        let max_x = (a.x + a.w).max(b.x + b.w);
        let max_y = (a.y + a.h).max(b.y + b.h);
        types::LayoutRect {
            x: min_x,
            y: min_y,
            w: max_x - min_x,
            h: max_y - min_y,
        }
    }

    #[cfg(feature = "xr")]
    fn command_bounds(gpu: &mut GpuState, command: &DrawCommand) -> Option<types::LayoutRect> {
        match command {
            DrawCommand::Rect { rect, .. } | DrawCommand::Border { rect, .. } => Some(rect.clone()),
            DrawCommand::Text {
                text,
                x,
                y,
                max_width,
                font_size,
                ..
            } => {
                let (w, h) = gpu.measure_text(text, *font_size, *max_width);
                Some(types::LayoutRect { x: *x, y: *y, w, h })
            }
            DrawCommand::Line {
                x0,
                y0,
                x1,
                y1,
                width,
                ..
            } => Some(types::LayoutRect {
                x: x0.min(*x1) - width * 0.5,
                y: y0.min(*y1) - width * 0.5,
                w: (x1 - x0).abs() + *width,
                h: (y1 - y0).abs() + *width,
            }),
        }
    }

    #[cfg(feature = "xr")]
    fn normalize_layer_command(command: &DrawCommand, origin_x: f32, origin_y: f32) -> DrawCommand {
        let identity = types::mat4_identity();
        match command {
            DrawCommand::Rect {
                rect,
                color,
                border_radius,
                element_id,
                ..
            } => DrawCommand::Rect {
                rect: types::LayoutRect {
                    x: rect.x - origin_x,
                    y: rect.y - origin_y,
                    w: rect.w,
                    h: rect.h,
                },
                color: *color,
                border_radius: *border_radius,
                element_id: element_id.clone(),
                transform: identity,
                is_fixed: true,
            },
            DrawCommand::Border {
                rect,
                color,
                width,
                radius,
                element_id,
                ..
            } => DrawCommand::Border {
                rect: types::LayoutRect {
                    x: rect.x - origin_x,
                    y: rect.y - origin_y,
                    w: rect.w,
                    h: rect.h,
                },
                color: *color,
                width: *width,
                radius: *radius,
                element_id: element_id.clone(),
                transform: identity,
                is_fixed: true,
            },
            DrawCommand::Text {
                text,
                x,
                y,
                max_width,
                color,
                font_size,
                element_id,
                center,
                ..
            } => DrawCommand::Text {
                text: text.clone(),
                x: *x - origin_x,
                y: *y - origin_y,
                max_width: *max_width,
                color: *color,
                font_size: *font_size,
                element_id: element_id.clone(),
                is_fixed: true,
                transform: identity,
                center: [center[0] - origin_x, center[1] - origin_y],
            },
            DrawCommand::Line {
                x0,
                y0,
                x1,
                y1,
                color,
                width,
            } => DrawCommand::Line {
                x0: *x0 - origin_x,
                y0: *y0 - origin_y,
                x1: *x1 - origin_x,
                y1: *y1 - origin_y,
                color: *color,
                width: *width,
            },
        }
    }

    #[cfg(feature = "xr")]
    fn collect_promoted_layer_candidates(
        gpu: &mut GpuState,
        commands: &[DrawCommand],
    ) -> (Vec<DrawCommand>, Vec<XrPromotedLayerCandidate>) {
        let identity = types::mat4_identity();
        let mut groups = Vec::<XrPromotedLayerCandidate>::new();
        let mut group_index = std::collections::HashMap::<String, usize>::new();
        let mut invalid_ids = std::collections::HashSet::<String>::new();

        for (order, command) in commands.iter().enumerate() {
            let Some(id) = Self::command_element_id(command) else {
                continue;
            };
            let Some(transform) = Self::command_transform(command) else {
                continue;
            };
            if transform == identity {
                continue;
            }
            let Some(bounds) = Self::command_bounds(gpu, command) else {
                continue;
            };
            let is_fixed = Self::command_is_fixed(command);
            let key = id.to_string();

            let group_idx = if let Some(&idx) = group_index.get(&key) {
                let existing = &groups[idx];
                if existing.transform != transform || existing.is_fixed != is_fixed {
                    invalid_ids.insert(key.clone());
                    continue;
                }
                idx
            } else {
                let idx = groups.len();
                group_index.insert(key.clone(), idx);
                groups.push(XrPromotedLayerCandidate {
                    key: key.clone(),
                    commands: Vec::new(),
                    bounds: bounds.clone(),
                    transform,
                    is_fixed,
                    contains_text: false,
                    draw_order: order as f32 + 1.0,
                    first_order: order,
                });
                idx
            };

            let group = &mut groups[group_idx];
            group.commands.push(command.clone());
            group.bounds = Self::union_rect(&group.bounds, &bounds);
            group.contains_text |= matches!(command, DrawCommand::Text { .. });
            group.draw_order = order as f32 + 1.0;
        }

        let promotable_ids = groups
            .iter()
            .filter(|group| group.contains_text && !invalid_ids.contains(&group.key))
            .map(|group| group.key.clone())
            .collect::<std::collections::HashSet<_>>();

        let native_commands = commands
            .iter()
            .filter(|command| {
                Self::command_element_id(command)
                    .map(|id| !promotable_ids.contains(id))
                    .unwrap_or(true)
            })
            .cloned()
            .collect();

        let mut promoted_layers = groups
            .into_iter()
            .filter(|group| group.contains_text && !invalid_ids.contains(&group.key))
            .collect::<Vec<_>>();
        promoted_layers.sort_by_key(|group| group.first_order);

        (native_commands, promoted_layers)
    }

    #[cfg(feature = "xr")]
    fn build_xr_native_frame_plan(
        state: &mut WebviewState,
        static_commands: &[DrawCommand],
        ghost_commands: &[DrawCommand],
        layer_scale_factor: f32,
    ) -> XrNativeFramePlan {
        let (native_static_commands, mut promoted_layers) =
            Self::collect_promoted_layer_candidates(&mut state.gpu, static_commands);
        let (native_ghost_commands, ghost_layers) =
            Self::collect_promoted_layer_candidates(&mut state.gpu, ghost_commands);
        promoted_layers.extend(ghost_layers);
        promoted_layers.sort_by_key(|group| group.first_order);

        let mut xr_layers = Vec::new();
        for layer in promoted_layers {
            let width = (layer.bounds.w * layer_scale_factor).ceil().max(1.0) as u32;
            let height = (layer.bounds.h * layer_scale_factor).ceil().max(1.0) as u32;
            let needs_recreate = state
                .xr_promoted_targets
                .get(&layer.key)
                .map(|target| target.size.width != width || target.size.height != height)
                .unwrap_or(true);
            if needs_recreate {
                state.xr_promoted_targets.insert(
                    layer.key.clone(),
                    state.gpu.create_render_target(width, height),
                );
            }

            let local_commands = layer
                .commands
                .iter()
                .map(|command| {
                    Self::normalize_layer_command(command, layer.bounds.x, layer.bounds.y)
                })
                .collect::<Vec<_>>();

            state.gpu.static_dirty = true;
            if let Some(target) = state.xr_promoted_targets.get(&layer.key) {
                state.gpu.render_to_target(
                    target,
                    layer_scale_factor,
                    &local_commands,
                    &[],
                    [0.0, 0.0, 0.0, 0.0],
                    0.0,
                    &[],
                );
                xr_layers.push(gpu::XrTextureLayer {
                    view: target.texture.create_view(&Default::default()),
                    rect: [
                        layer.bounds.x,
                        layer.bounds.y,
                        layer.bounds.w,
                        layer.bounds.h,
                    ],
                    transform: layer.transform,
                    is_fixed: layer.is_fixed,
                    draw_order: layer.draw_order,
                });
            }
        }

        XrNativeFramePlan {
            native_static_commands,
            native_ghost_commands,
            promoted_layers: xr_layers,
        }
    }

    fn build_cmd_index(
        static_cmds: &[DrawCommand],
        ghost_cmds: &[DrawCommand],
    ) -> std::collections::HashMap<String, Vec<usize>> {
        let mut index: std::collections::HashMap<String, Vec<usize>> =
            std::collections::HashMap::new();
        for (i, cmd) in static_cmds.iter().chain(ghost_cmds.iter()).enumerate() {
            let id = match cmd {
                DrawCommand::Rect { element_id, .. }
                | DrawCommand::Border { element_id, .. }
                | DrawCommand::Text { element_id, .. } => element_id.as_deref(),
                _ => None,
            };
            if let Some(id) = id {
                index.entry(id.to_string()).or_default().push(i);
            }
        }
        index
    }

    #[cfg(feature = "js")]
    fn tick_dom(&mut self) {
        {
            let s = self.state.as_mut().unwrap();
            let now_ms = s.start_time.elapsed().as_secs_f64() * 1000.0;
            s.js.tick(now_ms);
        }
        if self.state.as_ref().unwrap().js.is_dirty() {
            self.rebuild_layout();
        }
    }

    #[cfg(feature = "xr")]
    fn create_xr_session(state: &mut WebviewState) {
        if state.xr_session.is_some() || state.xr_session_failed {
            return;
        }
        #[cfg(target_os = "android")]
        {
            let activity_resumed = ANDROID_ACTIVITY_RESUMED.load(Ordering::Relaxed);
            let activity_focused = ANDROID_ACTIVITY_FOCUSED.load(Ordering::Relaxed);
            if !(activity_resumed && activity_focused) {
                return;
            }
        }
        log::info!("Creating XR session");
        if let Some(ctx) = state.xr_context.as_ref() {
            match xr_session::XrSession::new(
                ctx,
                &state.gpu.instance,
                &state.gpu.adapter,
                &state.gpu.device,
                #[cfg(target_os = "android")]
                (xr_panel_render_size().width, xr_panel_render_size().height),
            ) {
                Ok(session) => {
                    state.xr_session = Some(session);
                    state.xr_session_failed = false;
                    log::info!("XR session created");
                }
                Err(e) => {
                    state.xr_session_failed = true;
                    log::error!("Failed to create XR session: {:?}", e);
                }
            }
        } else {
            state.xr_session_failed = true;
            log::error!("Cannot create XR session without XR context");
        }
    }

    pub fn navigate(&mut self, href: &str) {
        let state = self.state.as_mut().unwrap();
        let current_asset = asset_name(href).to_string();
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        let html_source = match std::fs::read_to_string(state.asset_dir.join(href)) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Navigation failed: {e}");
                return;
            }
        };
        #[cfg(any(target_os = "android", target_os = "ios"))]
        let html_source = mobile_html_source(href);

        let css_sources = extract_styles(&html_source);
        #[cfg(feature = "js")]
        {
            let script = html_parser::extract_script(&html_source);
            let mut new_js = js_bridge::JsBridge::new().expect("failed to init JS bridge");
            new_js.init_webgpu(state.gpu.device.clone(), state.gpu.queue.clone());
            if let Some(script) = script.as_deref() {
                if let Err(e) = new_js.eval_script(script) {
                    log::error!("failed to eval JS: {:?}", e);
                }
            }
            state.js = new_js;
        }

        state.current_asset = current_asset;
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
        #[cfg(target_os = "android")]
        ANDROID_ACTIVITY_RESUMED.store(true, Ordering::Relaxed);
        if self.state.is_some() {
            log::info!("Ignoring duplicate resumed event; keeping existing app state");
            if let Some(state) = self.state.as_ref() {
                state.gpu.window.request_redraw();
            }
            return;
        }

        let window_attrs = Window::default_attributes().with_title("Custom Webview - wgpu");
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        let window_attrs = window_attrs.with_inner_size(winit::dpi::PhysicalSize::new(1280, 720));

        let window = Arc::new(event_loop.create_window(window_attrs).unwrap());

        #[cfg(feature = "xr")]
        let xr_context = match crate::xr_session::XrContext::new() {
            Ok(ctx) => Some(ctx),
            Err(e) => {
                log::error!("Failed to initialize OpenXR context: {:?}", e);
                None
            }
        };

        let mut gpu = pollster::block_on(GpuState::new(
            event_loop.owned_display_handle(),
            window.clone(),
            #[cfg(feature = "xr")]
            xr_context.as_ref(),
        ))
        .unwrap();

        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        let (initial_asset, html_source) = {
            let asset_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets");
            let html_file = std::env::args()
                .nth(1)
                .unwrap_or_else(|| "index.html".to_string());
            let html_source = std::fs::read_to_string(asset_dir.join(&html_file))
                .expect("failed to read HTML file");
            (html_file, html_source)
        };
        #[cfg(target_os = "android")]
        let initial_asset = "index.html".to_string();
        #[cfg(target_os = "ios")]
        let initial_asset = "index.html".to_string();
        #[cfg(any(target_os = "android", target_os = "ios"))]
        let html_source = mobile_html_source(&initial_asset);

        let css_sources = extract_styles(&html_source);

        #[cfg(feature = "js")]
        let js = {
            let script = html_parser::extract_script(&html_source);
            match js_bridge::JsBridge::new() {
                Ok(mut js) => {
                    js.init_webgpu(gpu.device.clone(), gpu.queue.clone());
                    if let Some(script) = script.as_deref() {
                        if let Err(e) = js.eval_script(script) {
                            log::error!("failed to eval JS: {:?}", e);
                        }
                    }
                    js
                }
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

        let view_size = self.target_view_size(&gpu);
        let view_scale_factor = self.target_view_scale_factor(&gpu);
        let scale = view_scale_factor as f32;
        let mut text_cache = TextMeasureCache::default();
        let layout_tree = build_layout(
            &styled,
            view_size.width as f32 / scale,
            view_size.height as f32 / scale,
            &mut text_cache,
            &mut |text, font_size, max_width| gpu.measure_text(text, font_size, max_width),
        );
        let all_commands = generate_draw_commands(&layout_tree, &canvas_ops);
        let (static_commands, ghost_commands) = Self::split_commands(all_commands);
        let cmd_index = Self::build_cmd_index(&static_commands, &ghost_commands);

        let clear_color = styled.style.background_color;

        self.state = Some(WebviewState {
            gpu,
            #[cfg(feature = "js")]
            js,
            layout: layout_tree,
            static_commands,
            ghost_commands,
            cmd_index,
            clear_color,
            current_asset: initial_asset,
            html_source,
            css_sources,
            styled_base,
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            asset_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets"),
            #[cfg(any(target_os = "android", target_os = "ios"))]
            asset_dir: PathBuf::new(),
            start_time: Instant::now(),
            view_size,
            view_scale_factor,
            scroll_y: 0.0,
            mouse_buttons: 0,
            text_cache,
            ghost_pos: None,
            prev_style_positions: std::collections::HashMap::new(),
            last_touch_y: None,
            touch_default_prevented: false,
            #[cfg(feature = "xr")]
            xr_context,
            #[cfg(feature = "xr")]
            xr_session: None,
            #[cfg(feature = "xr")]
            xr_session_failed: false,
            #[cfg(feature = "xr")]
            xr_depth_texture: None,
            #[cfg(feature = "xr")]
            xr_depth_size: (0, 0),
            #[cfg(feature = "xr")]
            xr_panel_depth_texture: None,
            #[cfg(feature = "xr")]
            xr_panel_depth_size: (0, 0),
            #[cfg(feature = "xr")]
            xr_native_msaa_color_texture: None,
            #[cfg(feature = "xr")]
            xr_native_msaa_depth_texture: None,
            #[cfg(feature = "xr")]
            xr_native_msaa_size: (0, 0),
            #[cfg(feature = "xr")]
            xr_panel_target: None,
            #[cfg(feature = "xr")]
            xr_promoted_targets: std::collections::HashMap::new(),
            #[cfg(feature = "xr")]
            page_xr_active: false,
            #[cfg(feature = "xr")]
            xr_right_select_down: false,
            #[cfg(feature = "xr")]
            xr_left_select_down: false,
            #[cfg(feature = "xr")]
            xr_right_pointer_pos: None,
            #[cfg(feature = "xr")]
            xr_left_pointer_pos: None,
            #[cfg(feature = "xr")]
            xr_exit_down: false,
        });

        window.request_redraw();
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        #[cfg(target_os = "android")]
        {
            ANDROID_ACTIVITY_RESUMED.store(false, Ordering::Relaxed);
            ANDROID_ACTIVITY_FOCUSED.store(false, Ordering::Relaxed);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if self.state.is_none() {
            return;
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                let state = self.state.as_mut().unwrap();
                state.gpu.resize(size);
                #[cfg(all(target_os = "android", feature = "xr"))]
                {
                    state.view_size = xr_panel_view_size();
                    state.view_scale_factor = 1.0;
                }
                #[cfg(not(all(target_os = "android", feature = "xr")))]
                {
                    state.view_size = state.gpu.size;
                    state.view_scale_factor = state.gpu.scale_factor;
                }
                self.rebuild_layout();
                self.state.as_ref().unwrap().gpu.window.request_redraw();
            }
            WindowEvent::Focused(focused) => {
                #[cfg(target_os = "android")]
                ANDROID_ACTIVITY_FOCUSED.store(focused, Ordering::Relaxed);

                if focused {
                    self.state.as_ref().unwrap().gpu.window.request_redraw();
                }
            }
            WindowEvent::MouseInput {
                state: btn_state,
                button: btn,
                ..
            } => {
                let (button_index, button_bit) = match btn {
                    winit::event::MouseButton::Left => (0, 1),
                    winit::event::MouseButton::Right => (2, 2),
                    winit::event::MouseButton::Middle => (1, 4),
                    _ => (0, 1),
                };
                let (mx, my) = unsafe { CURSOR_POS };
                match btn_state {
                    winit::event::ElementState::Pressed => {
                        let s = self.state.as_mut().unwrap();
                        s.mouse_buttons |= button_bit;
                        if button_index == 0 {
                            let hit = renderer::hit_test(&s.layout, mx, my + s.scroll_y);
                            if let Some(ref hit) = hit {
                                if let Some(ref href) = hit.href {
                                    let href = href.clone();
                                    self.navigate(&href);
                                    return;
                                }
                            }
                        }
                        #[cfg(feature = "js")]
                        {
                            let s = self.state.as_mut().unwrap();
                            s.js.dispatch_pointer_down(
                                &s.layout,
                                mx,
                                my,
                                s.scroll_y,
                                1,
                                "mouse",
                                1.0,
                                true,
                                s.mouse_buttons,
                                button_index,
                            );
                            if s.js.is_dirty() && !self.try_patch_visual() {
                                self.rebuild_layout();
                                self.state.as_ref().unwrap().gpu.window.request_redraw();
                            }
                        }
                    }
                    winit::event::ElementState::Released => {
                        let s = self.state.as_mut().unwrap();
                        s.mouse_buttons &= !button_bit;
                        #[cfg(feature = "js")]
                        {
                            s.js.dispatch_pointer_up(
                                &s.layout,
                                mx,
                                my,
                                s.scroll_y,
                                1,
                                "mouse",
                                0.0,
                                true,
                                s.mouse_buttons,
                                button_index,
                            );
                            if s.js.is_dirty() && !self.try_patch_visual() {
                                self.rebuild_layout();
                                self.state.as_ref().unwrap().gpu.window.request_redraw();
                            }
                        }
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let s = self.state.as_mut().unwrap();
                let dy = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => -y * 40.0,
                    winit::event::MouseScrollDelta::PixelDelta(pos) => -pos.y as f32,
                };
                let max_scroll = Self::max_scroll(s);
                s.scroll_y = (s.scroll_y + dy).clamp(0.0, max_scroll);
                #[cfg(feature = "js")]
                s.js.set_scroll_y(s.scroll_y);
                s.gpu.window.request_redraw();
            }
            WindowEvent::Touch(touch) => {
                let scale = self.state.as_ref().unwrap().view_scale_factor as f32;
                let mx = touch.location.x as f32 / scale;
                let my = touch.location.y as f32 / scale;
                unsafe {
                    CURSOR_POS = (mx, my);
                }
                match touch.phase {
                    winit::event::TouchPhase::Started => {
                        let s = self.state.as_mut().unwrap();
                        s.mouse_buttons = 1;
                        s.last_touch_y = Some(my);
                        s.touch_default_prevented = false;

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
                            let pid = touch.id as i32 + 1;
                            let is_primary = pid == 1;
                            let prevented = s.js.dispatch_pointer_down(
                                &s.layout, mx, my, s.scroll_y, pid, "touch", 1.0, is_primary, 1, 0,
                            );
                            if prevented {
                                s.touch_default_prevented = true;
                            }
                            if s.js.is_dirty() {
                                self.rebuild_layout();
                            }
                        }
                        self.state.as_ref().unwrap().gpu.window.request_redraw();
                    }
                    winit::event::TouchPhase::Moved => {
                        #[cfg(feature = "js")]
                        let prevented = {
                            let s = self.state.as_mut().unwrap();
                            let pid = touch.id as i32 + 1;
                            let is_primary = pid == 1;
                            let p = s.js.dispatch_pointer_move(
                                &s.layout, mx, my, s.scroll_y, pid, "touch", 1.0, is_primary, 1, 0,
                            ) || s.touch_default_prevented;
                            if s.js.is_dirty() {
                                self.rebuild_layout();
                            }
                            p
                        };

                        let s = self.state.as_mut().unwrap();
                        #[cfg(feature = "js")]
                        let skip_scroll = prevented;
                        #[cfg(not(feature = "js"))]
                        let skip_scroll = false;

                        if !skip_scroll && s.ghost_commands.is_empty() {
                            if let Some(last_y) = s.last_touch_y {
                                let dy = last_y - my;
                                let max_scroll = Self::max_scroll(s);
                                s.scroll_y = (s.scroll_y + dy).clamp(0.0, max_scroll);
                                #[cfg(feature = "js")]
                                s.js.set_scroll_y(s.scroll_y);
                            }
                        }
                        s.last_touch_y = Some(my);
                        self.state.as_ref().unwrap().gpu.window.request_redraw();
                    }
                    winit::event::TouchPhase::Ended | winit::event::TouchPhase::Cancelled => {
                        let s = self.state.as_mut().unwrap();
                        s.mouse_buttons = 0;
                        s.last_touch_y = None;
                        #[cfg(feature = "js")]
                        {
                            let s = self.state.as_mut().unwrap();
                            let pid = touch.id as i32 + 1;
                            let is_primary = pid == 1;
                            s.js.dispatch_pointer_up(
                                &s.layout, mx, my, s.scroll_y, pid, "touch", 0.0, is_primary, 0, 0,
                            );
                            s.ghost_pos = None;
                            if s.js.is_dirty() {
                                self.rebuild_layout();
                            }
                        }
                        self.state.as_ref().unwrap().gpu.window.request_redraw();
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let scale = self
                    .state
                    .as_ref()
                    .map(|s| s.view_scale_factor as f32)
                    .unwrap_or(1.0);
                let (mx, my) = (position.x as f32 / scale, position.y as f32 / scale);
                unsafe {
                    CURSOR_POS = (mx, my);
                }
                #[cfg(feature = "js")]
                {
                    let s = self.state.as_mut().unwrap();
                    let buttons = s.mouse_buttons;
                    let pressure = if buttons > 0 { 1.0 } else { 0.0 };
                    s.js.dispatch_pointer_move(
                        &s.layout, mx, my, s.scroll_y, 1, "mouse", pressure, true, buttons, 0,
                    );
                    if s.js.is_dirty() && !self.try_patch_visual() {
                        self.rebuild_layout();
                    }
                }
                self.state.as_ref().unwrap().gpu.window.request_redraw();
            }
            WindowEvent::CursorLeft { .. } => {
                #[cfg(feature = "js")]
                {
                    let s = self.state.as_mut().unwrap();
                    s.js.dispatch_pointer_leave(1);
                    if s.js.is_dirty() {
                        self.rebuild_layout();
                    }
                }
                self.state.as_ref().unwrap().gpu.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                if self.state.is_none() {
                    return;
                }

                // XR session lifecycle management
                #[cfg(all(feature = "js", feature = "xr"))]
                {
                    let s = self.state.as_mut().unwrap();
                    #[cfg(target_os = "android")]
                    if s.xr_session.is_none() {
                        Self::create_xr_session(s);
                    }

                    if s.js.take_xr_request() {
                        s.page_xr_active = true;
                        if s.xr_session.is_none() {
                            Self::create_xr_session(s);
                        }
                    }
                    if s.js.take_xr_end_request() {
                        s.page_xr_active = false;
                        s.xr_right_select_down = false;
                        s.xr_left_select_down = false;
                        s.xr_right_pointer_pos = None;
                        s.xr_left_pointer_pos = None;
                        s.xr_exit_down = false;
                        #[cfg(not(target_os = "android"))]
                        {
                            s.xr_session = None;
                            log::info!("XR session ended");
                        }
                        #[cfg(target_os = "android")]
                        {
                            log::info!("Returned to immersive panel");
                        }
                    }
                }

                // XR render loop (when session is active)
                #[cfg(all(feature = "xr", feature = "js"))]
                {
                    let mut xr_rendered = false;
                    let mut xr_navigation = None;
                    let xr_session = self.state.as_mut().unwrap().xr_session.take();
                    if let Some(mut xr) = xr_session {
                        if let Err(e) = xr.poll_events() {
                            log::error!("XR poll error: {:?}", e);
                        }
                        if xr.state() == xr_session::XrState::Running {
                            let (sw, sh) = xr.swapchain_size();
                            match xr.wait_frame() {
                                Ok(Some(frame_data)) => {
                                    if self.state.as_ref().unwrap().page_xr_active {
                                        match xr.acquire_swapchain_image() {
                                            Ok((texture, _idx)) => {
                                                let s = self.state.as_mut().unwrap();
                                                let left_view = texture.create_view(
                                                    &wgpu::TextureViewDescriptor {
                                                        dimension: Some(
                                                            wgpu::TextureViewDimension::D2,
                                                        ),
                                                        base_array_layer: 0,
                                                        array_layer_count: Some(1),
                                                        ..Default::default()
                                                    },
                                                );
                                                let right_view = texture.create_view(
                                                    &wgpu::TextureViewDescriptor {
                                                        dimension: Some(
                                                            wgpu::TextureViewDimension::D2,
                                                        ),
                                                        base_array_layer: 1,
                                                        array_layer_count: Some(1),
                                                        ..Default::default()
                                                    },
                                                );

                                                if s.xr_depth_texture.is_none()
                                                    || s.xr_depth_size != (sw, sh)
                                                {
                                                    let dt = s.gpu.device.create_texture(
                                                        &wgpu::TextureDescriptor {
                                                            label: Some("xr depth"),
                                                            size: wgpu::Extent3d {
                                                                width: sw,
                                                                height: sh,
                                                                depth_or_array_layers: 1,
                                                            },
                                                            mip_level_count: 1,
                                                            sample_count: 1,
                                                            dimension: wgpu::TextureDimension::D2,
                                                            format: wgpu::TextureFormat::Depth24Plus,
                                                            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                                                            view_formats: &[],
                                                        },
                                                    );
                                                    s.xr_depth_texture = Some(dt);
                                                    s.xr_depth_size = (sw, sh);
                                                }
                                                let depth_view = s
                                                    .xr_depth_texture
                                                    .as_ref()
                                                    .unwrap()
                                                    .create_view(&Default::default());

                                                if let Err(e) = xr.sync_input() {
                                                    log::error!("XR input sync error: {:?}", e);
                                                }
                                                let exit_down = xr.exit_pressed().unwrap_or(false);
                                                if !s.xr_exit_down && exit_down {
                                                    if let Err(e) =
                                                        s.js.force_end_active_xr_session()
                                                    {
                                                        log::error!(
                                                            "Failed to end XR session from controller: {:?}",
                                                            e
                                                        );
                                                    }
                                                }
                                                s.xr_exit_down = exit_down;
                                                let (left_vh, right_vh, depth_vh) = {
                                                    let mut bridge = s.js.webgpu_bridge_mut();
                                                    let gpu = bridge.as_mut().unwrap();
                                                    let lvh = gpu.register_texture_view(left_view);
                                                    let rvh = gpu.register_texture_view(right_view);
                                                    let dvh = gpu.register_texture_view(depth_view);
                                                    (lvh, rvh, dvh)
                                                };

                                                let left_eye = js_bridge::XrEyeData {
                                                    tex_handle: depth_vh,
                                                    view_handle: left_vh,
                                                    width: sw,
                                                    height: sh,
                                                    proj_matrix: frame_data.views[0]
                                                        .projection_matrix,
                                                    view_inv_matrix: frame_data.views[0]
                                                        .view_matrix,
                                                };
                                                let right_eye = js_bridge::XrEyeData {
                                                    tex_handle: depth_vh,
                                                    view_handle: right_vh,
                                                    width: sw,
                                                    height: sh,
                                                    proj_matrix: frame_data.views[1]
                                                        .projection_matrix,
                                                    view_inv_matrix: frame_data.views[1]
                                                        .view_matrix,
                                                };

                                                s.js.set_xr_view_data(Some(
                                                    js_bridge::XrViewData {
                                                        left: left_eye,
                                                        right: right_eye,
                                                    },
                                                ));

                                                let now_ms =
                                                    s.start_time.elapsed().as_secs_f64() * 1000.0;
                                                s.js.tick(now_ms);
                                                s.js.set_xr_view_data(None);
                                                {
                                                    let mut bridge = s.js.webgpu_bridge_mut();
                                                    let gpu = bridge.as_mut().unwrap();
                                                    gpu.unregister_texture_view(left_vh);
                                                    gpu.unregister_texture_view(right_vh);
                                                    gpu.unregister_texture_view(depth_vh);
                                                }

                                                let _ = s.gpu.device.poll(wgpu::PollType::Poll);
                                                if let Err(e) =
                                                    xr.release_and_end_frame(&frame_data)
                                                {
                                                    log::error!("XR end frame error: {:?}", e);
                                                }
                                                xr_rendered = true;
                                            }
                                            Err(e) => {
                                                log::error!("XR acquire error: {:?}", e);
                                                if let Err(end_err) = xr.end_frame_without_layers(
                                                    frame_data.predicted_display_time,
                                                ) {
                                                    log::error!(
                                                        "XR end frame after acquire failure error: {:?}",
                                                        end_err
                                                    );
                                                }
                                            }
                                        }
                                    } else {
                                        let s = self.state.as_mut().unwrap();
                                        if let Err(e) = xr.sync_input() {
                                            log::error!("XR input sync error: {:?}", e);
                                        }
                                        let right_select =
                                            xr.right_select_pressed().unwrap_or(false);
                                        let left_select = xr.left_select_pressed().unwrap_or(false);
                                        let scroll_axis = xr.scroll_axis().unwrap_or(0.0);
                                        if scroll_axis != 0.0 {
                                            let max_scroll = Self::max_scroll(s);
                                            s.scroll_y = (s.scroll_y
                                                - scroll_axis * XR_SCROLL_SPEED)
                                                .clamp(0.0, max_scroll);
                                            s.js.set_scroll_y(s.scroll_y);
                                        }

                                        // Right hand (pointerId=1, primary)
                                        let right_pose = match xr
                                            .right_pointer_pose(frame_data.predicted_display_time)
                                        {
                                            Ok(p) => p,
                                            Err(e) => {
                                                log::error!("XR right pose error: {:?}", e);
                                                None
                                            }
                                        };
                                        if let Some((mx, my)) = right_pose.and_then(|pose| {
                                            xr_panel_pointer(
                                                &pose,
                                                &xr_panel_local_pose(),
                                                s.view_size,
                                                s.view_scale_factor,
                                            )
                                        }) {
                                            unsafe {
                                                CURSOR_POS = (mx, my);
                                            }
                                            s.xr_right_pointer_pos = Some((mx, my));
                                            let btns = if right_select { 1 } else { 0 };
                                            let pressure = if right_select { 1.0 } else { 0.0 };
                                            s.js.dispatch_pointer_move(
                                                &s.layout,
                                                mx,
                                                my,
                                                s.scroll_y,
                                                1,
                                                "xr-controller",
                                                pressure,
                                                true,
                                                btns,
                                                0,
                                            );
                                        } else if !right_select {
                                            s.xr_right_pointer_pos = None;
                                        }
                                        if !s.xr_right_select_down && right_select {
                                            if let Some((mx, my)) = s.xr_right_pointer_pos {
                                                let y = my + s.scroll_y;
                                                let hit = renderer::hit_test(&s.layout, mx, y);
                                                if let Some(href) =
                                                    hit.and_then(|hit| hit.href.clone())
                                                {
                                                    xr_navigation = Some(href);
                                                } else {
                                                    s.js.dispatch_pointer_down(
                                                        &s.layout,
                                                        mx,
                                                        my,
                                                        s.scroll_y,
                                                        1,
                                                        "xr-controller",
                                                        1.0,
                                                        true,
                                                        1,
                                                        0,
                                                    );
                                                }
                                            }
                                        }
                                        if s.xr_right_select_down && !right_select {
                                            if let Some((mx, my)) = s.xr_right_pointer_pos {
                                                s.js.dispatch_pointer_up(
                                                    &s.layout,
                                                    mx,
                                                    my,
                                                    s.scroll_y,
                                                    1,
                                                    "xr-controller",
                                                    0.0,
                                                    true,
                                                    0,
                                                    0,
                                                );
                                            }
                                        }
                                        s.xr_right_select_down = right_select;

                                        // Left hand (pointerId=2)
                                        let left_pose = match xr
                                            .left_pointer_pose(frame_data.predicted_display_time)
                                        {
                                            Ok(p) => p,
                                            Err(e) => {
                                                log::error!("XR left pose error: {:?}", e);
                                                None
                                            }
                                        };
                                        if let Some((mx, my)) = left_pose.and_then(|pose| {
                                            xr_panel_pointer(
                                                &pose,
                                                &xr_panel_local_pose(),
                                                s.view_size,
                                                s.view_scale_factor,
                                            )
                                        }) {
                                            s.xr_left_pointer_pos = Some((mx, my));
                                            let btns = if left_select { 1 } else { 0 };
                                            let pressure = if left_select { 1.0 } else { 0.0 };
                                            s.js.dispatch_pointer_move(
                                                &s.layout,
                                                mx,
                                                my,
                                                s.scroll_y,
                                                2,
                                                "xr-controller",
                                                pressure,
                                                false,
                                                btns,
                                                0,
                                            );
                                        } else if !left_select {
                                            s.xr_left_pointer_pos = None;
                                        }
                                        if !s.xr_left_select_down && left_select {
                                            if let Some((mx, my)) = s.xr_left_pointer_pos {
                                                let y = my + s.scroll_y;
                                                let hit = renderer::hit_test(&s.layout, mx, y);
                                                if let Some(href) =
                                                    hit.and_then(|hit| hit.href.clone())
                                                {
                                                    xr_navigation = Some(href);
                                                } else {
                                                    s.js.dispatch_pointer_down(
                                                        &s.layout,
                                                        mx,
                                                        my,
                                                        s.scroll_y,
                                                        2,
                                                        "xr-controller",
                                                        1.0,
                                                        false,
                                                        1,
                                                        0,
                                                    );
                                                }
                                            }
                                        }
                                        if s.xr_left_select_down && !left_select {
                                            if let Some((mx, my)) = s.xr_left_pointer_pos {
                                                s.js.dispatch_pointer_up(
                                                    &s.layout,
                                                    mx,
                                                    my,
                                                    s.scroll_y,
                                                    2,
                                                    "xr-controller",
                                                    0.0,
                                                    false,
                                                    0,
                                                    0,
                                                );
                                            }
                                        }
                                        s.xr_left_select_down = left_select;
                                        s.xr_exit_down = false;

                                        let now_ms = s.start_time.elapsed().as_secs_f64() * 1000.0;
                                        s.js.tick(now_ms);
                                        if s.js.is_dirty() {
                                            Self::rebuild_layout_state(s);
                                        }

                                        let mut overlay_commands = s.ghost_commands.clone();
                                        for (mx, my, color) in [
                                            s.xr_right_pointer_pos
                                                .map(|(x, y)| (x, y, [1.0f32, 1.0, 1.0, 0.9])),
                                            s.xr_left_pointer_pos
                                                .map(|(x, y)| (x, y, [0.5f32, 0.8, 1.0, 0.9])),
                                        ]
                                        .into_iter()
                                        .flatten()
                                        {
                                            overlay_commands.push(DrawCommand::Rect {
                                                rect: types::LayoutRect {
                                                    x: mx - 8.0,
                                                    y: my - 8.0,
                                                    w: 16.0,
                                                    h: 16.0,
                                                },
                                                color: [color[0], color[1], color[2], 0.2],
                                                border_radius: 8.0,
                                                element_id: None,
                                                transform: types::mat4_identity(),
                                                is_fixed: true,
                                            });
                                            overlay_commands.push(DrawCommand::Border {
                                                rect: types::LayoutRect {
                                                    x: mx - 8.0,
                                                    y: my - 8.0,
                                                    w: 16.0,
                                                    h: 16.0,
                                                },
                                                color,
                                                width: 2.0,
                                                radius: 8.0,
                                                element_id: None,
                                                transform: types::mat4_identity(),
                                                is_fixed: true,
                                            });
                                        }

                                        let owned_canvases: Vec<([f32; 4], wgpu::TextureView)> = {
                                            let bridge_ref = s.js.webgpu_bridge();
                                            if let Some(ref bridge) = *bridge_ref {
                                                let elem_rects = s.layout.collect_element_rects();
                                                bridge
                                                    .canvas_ids()
                                                    .iter()
                                                    .filter_map(|id| {
                                                        let tv =
                                                            bridge.get_canvas_texture_view(id)?;
                                                        let lr = elem_rects.get(id)?;
                                                        Some(([lr.x, lr.y, lr.w, lr.h], tv.clone()))
                                                    })
                                                    .collect()
                                            } else {
                                                Vec::new()
                                            }
                                        };
                                        let canvases: Vec<([f32; 4], &wgpu::TextureView)> =
                                            owned_canvases
                                                .iter()
                                                .map(|(rect, view)| (*rect, view))
                                                .collect();

                                        #[cfg(target_os = "android")]
                                        {
                                            let use_xr_native = Self::should_use_xr_native_page(
                                                &s.static_commands,
                                                &overlay_commands,
                                                !canvases.is_empty(),
                                            );
                                            let panel_size = xr_panel_layer_size(s.view_size);

                                            match xr.acquire_swapchain_texture() {
                                                Ok((eye_texture, _eye_idx)) => {
                                                    let left_view = eye_texture.create_view(
                                                        &wgpu::TextureViewDescriptor {
                                                            dimension: Some(
                                                                wgpu::TextureViewDimension::D2,
                                                            ),
                                                            base_array_layer: 0,
                                                            array_layer_count: Some(1),
                                                            ..Default::default()
                                                        },
                                                    );
                                                    let right_view = eye_texture.create_view(
                                                        &wgpu::TextureViewDescriptor {
                                                            dimension: Some(
                                                                wgpu::TextureViewDimension::D2,
                                                            ),
                                                            base_array_layer: 1,
                                                            array_layer_count: Some(1),
                                                            ..Default::default()
                                                        },
                                                    );
                                                    if use_xr_native {
                                                        let hybrid_plan =
                                                            Self::build_xr_hybrid_frame_plan(
                                                                &s.static_commands,
                                                                &overlay_commands,
                                                            );
                                                        let native_scale =
                                                            xr_native_render_scale_factor(
                                                                xr_panel_render_scale_factor(),
                                                            );
                                                        let promoted_scale =
                                                            xr_promoted_layer_render_scale_factor(
                                                                xr_panel_render_scale_factor(),
                                                            );
                                                        let native_plan =
                                                            Self::build_xr_native_frame_plan(
                                                                s,
                                                                &hybrid_plan.native_static_commands,
                                                                &hybrid_plan.native_ghost_commands,
                                                                promoted_scale,
                                                            );
                                                        let (sw, sh) = xr.swapchain_size();
                                                        ensure_xr_native_msaa_targets(s, sw, sh);
                                                        let msaa_color_view = s
                                                            .xr_native_msaa_color_texture
                                                            .as_ref()
                                                            .unwrap()
                                                            .create_view(&Default::default());
                                                        let depth_view = s
                                                            .xr_native_msaa_depth_texture
                                                            .as_ref()
                                                            .unwrap()
                                                            .create_view(&Default::default());
                                                        s.gpu.static_dirty = true;
                                                        s.gpu.render_xr_native_views(
                                                            &left_view,
                                                            &right_view,
                                                            Some(&msaa_color_view),
                                                            &depth_view,
                                                            [
                                                                s.view_size.width as f32,
                                                                s.view_size.height as f32,
                                                            ],
                                                            native_scale,
                                                            [panel_size.width, panel_size.height],
                                                            s.clear_color,
                                                            &native_plan.native_static_commands,
                                                            &native_plan.native_ghost_commands,
                                                            s.scroll_y,
                                                            xr_native_page_mvp(
                                                                &frame_data.views[0],
                                                            ),
                                                            xr_native_page_mvp(
                                                                &frame_data.views[1],
                                                            ),
                                                        );
                                                        s.gpu.render_xr_textured_layers(
                                                            &left_view,
                                                            &right_view,
                                                            Some(&msaa_color_view),
                                                            &depth_view,
                                                            [
                                                                s.view_size.width as f32,
                                                                s.view_size.height as f32,
                                                            ],
                                                            [panel_size.width, panel_size.height],
                                                            s.scroll_y,
                                                            xr_native_page_mvp(
                                                                &frame_data.views[0],
                                                            ),
                                                            xr_native_page_mvp(
                                                                &frame_data.views[1],
                                                            ),
                                                            &native_plan.promoted_layers,
                                                        );
                                                        if hybrid_plan.overlay_has_content() {
                                                            let (pw, ph) =
                                                                xr.panel_swapchain_size();
                                                            if s.xr_panel_depth_texture.is_none()
                                                                || s.xr_panel_depth_size != (pw, ph)
                                                            {
                                                                let dt = s.gpu.device.create_texture(
                                                                    &wgpu::TextureDescriptor {
                                                                        label: Some("xr panel depth"),
                                                                        size: wgpu::Extent3d {
                                                                            width: pw,
                                                                            height: ph,
                                                                            depth_or_array_layers: 1,
                                                                        },
                                                                        mip_level_count: 1,
                                                                        sample_count: 1,
                                                                        dimension:
                                                                            wgpu::TextureDimension::D2,
                                                                        format:
                                                                            wgpu::TextureFormat::Depth32Float,
                                                                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                                                                        view_formats: &[],
                                                                    },
                                                                );
                                                                s.xr_panel_depth_texture = Some(dt);
                                                                s.xr_panel_depth_size = (pw, ph);
                                                            }
                                                            match xr
                                                                .acquire_panel_swapchain_texture()
                                                            {
                                                                Ok((panel_texture, _panel_idx)) => {
                                                                    let panel_view = panel_texture
                                                                        .create_view(
                                                                            &wgpu::TextureViewDescriptor {
                                                                                dimension: Some(
                                                                                    wgpu::TextureViewDimension::D2,
                                                                                ),
                                                                                ..Default::default()
                                                                            },
                                                                        );
                                                                    let panel_depth_view = s
                                                                        .xr_panel_depth_texture
                                                                        .as_ref()
                                                                        .unwrap()
                                                                        .create_view(
                                                                            &Default::default(),
                                                                        );
                                                                    s.gpu.static_dirty = true;
                                                                    s.gpu.render_to_view(
                                                                        &panel_view,
                                                                        &panel_depth_view,
                                                                        winit::dpi::PhysicalSize::new(
                                                                            pw, ph,
                                                                        ),
                                                                        xr_panel_render_scale_factor(),
                                                                        true,
                                                                        &hybrid_plan
                                                                            .overlay_static_commands,
                                                                        &hybrid_plan
                                                                            .overlay_ghost_commands,
                                                                        [0.0, 0.0, 0.0, 0.0],
                                                                        s.scroll_y,
                                                                        &[],
                                                                    );
                                                                    let _ = s
                                                                        .gpu
                                                                        .device
                                                                        .poll(wgpu::PollType::Poll);
                                                                    if let Err(e) = xr
                                                                        .release_both_and_end_frame(
                                                                            &frame_data,
                                                                            xr_panel_local_pose(),
                                                                            panel_size,
                                                                        )
                                                                    {
                                                                        log::error!(
                                                                            "XR hybrid end frame error: {:?}",
                                                                            e
                                                                        );
                                                                    }
                                                                }
                                                                Err(e) => {
                                                                    log::error!(
                                                                        "XR overlay panel acquire error: {:?}",
                                                                        e
                                                                    );
                                                                    if let Err(end_err) = xr
                                                                        .release_and_end_frame(
                                                                            &frame_data,
                                                                        )
                                                                    {
                                                                        log::error!(
                                                                            "XR native end frame after overlay failure error: {:?}",
                                                                            end_err
                                                                        );
                                                                    }
                                                                }
                                                            }
                                                        } else if let Err(e) =
                                                            xr.release_and_end_frame(&frame_data)
                                                        {
                                                            log::error!(
                                                                "XR native end frame error: {:?}",
                                                                e
                                                            );
                                                        }
                                                        let _ =
                                                            s.gpu.device.poll(wgpu::PollType::Poll);
                                                    } else {
                                                        let (pw, ph) = xr.panel_swapchain_size();
                                                        if s.xr_panel_depth_texture.is_none()
                                                            || s.xr_panel_depth_size != (pw, ph)
                                                        {
                                                            let dt = s.gpu.device.create_texture(
                                                                &wgpu::TextureDescriptor {
                                                                    label: Some("xr panel depth"),
                                                                    size: wgpu::Extent3d {
                                                                        width: pw,
                                                                        height: ph,
                                                                        depth_or_array_layers: 1,
                                                                    },
                                                                    mip_level_count: 1,
                                                                    sample_count: 1,
                                                                    dimension:
                                                                        wgpu::TextureDimension::D2,
                                                                    format:
                                                                        wgpu::TextureFormat::Depth32Float,
                                                                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                                                                    view_formats: &[],
                                                                },
                                                            );
                                                            s.xr_panel_depth_texture = Some(dt);
                                                            s.xr_panel_depth_size = (pw, ph);
                                                        }
                                                        clear_xr_projection_views(
                                                            &s.gpu,
                                                            &left_view,
                                                            &right_view,
                                                            [0.0, 0.0, 0.0, 1.0],
                                                        );

                                                        match xr.acquire_panel_swapchain_texture() {
                                                            Ok((panel_texture, _panel_idx)) => {
                                                                let panel_view = panel_texture
                                                                    .create_view(
                                                                        &wgpu::TextureViewDescriptor {
                                                                            dimension: Some(
                                                                                wgpu::TextureViewDimension::D2,
                                                                            ),
                                                                            ..Default::default()
                                                                        },
                                                                    );
                                                                let panel_depth_view = s
                                                                    .xr_panel_depth_texture
                                                                    .as_ref()
                                                                    .unwrap()
                                                                    .create_view(
                                                                        &Default::default(),
                                                                    );
                                                                s.gpu.render_to_view(
                                                                    &panel_view,
                                                                    &panel_depth_view,
                                                                    winit::dpi::PhysicalSize::new(
                                                                        pw, ph,
                                                                    ),
                                                                    xr_panel_render_scale_factor(),
                                                                    true,
                                                                    &s.static_commands,
                                                                    &overlay_commands,
                                                                    s.clear_color,
                                                                    s.scroll_y,
                                                                    &canvases,
                                                                );
                                                                let _ = s
                                                                    .gpu
                                                                    .device
                                                                    .poll(wgpu::PollType::Poll);
                                                                if let Err(e) = xr
                                                                    .release_both_and_end_frame(
                                                                        &frame_data,
                                                                        xr_panel_local_pose(),
                                                                        panel_size,
                                                                    )
                                                                {
                                                                    log::error!(
                                                                        "XR panel end frame error: {:?}",
                                                                        e
                                                                    );
                                                                }
                                                            }
                                                            Err(e) => {
                                                                log::error!(
                                                                    "XR panel acquire error: {:?}",
                                                                    e
                                                                );
                                                                if let Err(end_err) = xr
                                                                    .end_frame_without_layers(
                                                                        frame_data
                                                                            .predicted_display_time,
                                                                    )
                                                                {
                                                                    log::error!(
                                                                        "XR end frame after panel acquire failure error: {:?}",
                                                                        end_err
                                                                    );
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                                Err(e) => {
                                                    log::error!("XR acquire error: {:?}", e);
                                                    if let Err(end_err) = xr
                                                        .end_frame_without_layers(
                                                            frame_data.predicted_display_time,
                                                        )
                                                    {
                                                        log::error!(
                                                            "XR end frame after acquire failure error: {:?}",
                                                            end_err
                                                        );
                                                    }
                                                }
                                            }
                                        }

                                        #[cfg(not(target_os = "android"))]
                                        {
                                            let use_xr_native = Self::should_use_xr_native_page(
                                                &s.static_commands,
                                                &overlay_commands,
                                                !canvases.is_empty(),
                                            );
                                            match xr.acquire_swapchain_image() {
                                                Ok((texture, _idx)) => {
                                                    let left_view = texture.create_view(
                                                        &wgpu::TextureViewDescriptor {
                                                            dimension: Some(
                                                                wgpu::TextureViewDimension::D2,
                                                            ),
                                                            base_array_layer: 0,
                                                            array_layer_count: Some(1),
                                                            ..Default::default()
                                                        },
                                                    );
                                                    let right_view = texture.create_view(
                                                        &wgpu::TextureViewDescriptor {
                                                            dimension: Some(
                                                                wgpu::TextureViewDimension::D2,
                                                            ),
                                                            base_array_layer: 1,
                                                            array_layer_count: Some(1),
                                                            ..Default::default()
                                                        },
                                                    );
                                                    if use_xr_native {
                                                        let native_scale =
                                                            xr_native_render_scale_factor(
                                                                s.view_scale_factor as f32,
                                                            );
                                                        let static_commands =
                                                            s.static_commands.clone();
                                                        let promoted_scale =
                                                            xr_promoted_layer_render_scale_factor(
                                                                s.view_scale_factor as f32,
                                                            );
                                                        let native_plan =
                                                            Self::build_xr_native_frame_plan(
                                                                s,
                                                                &static_commands,
                                                                &overlay_commands,
                                                                promoted_scale,
                                                            );
                                                        let (sw, sh) = xr.swapchain_size();
                                                        ensure_xr_native_msaa_targets(s, sw, sh);
                                                        let msaa_color_view = s
                                                            .xr_native_msaa_color_texture
                                                            .as_ref()
                                                            .unwrap()
                                                            .create_view(&Default::default());
                                                        let depth_view = s
                                                            .xr_native_msaa_depth_texture
                                                            .as_ref()
                                                            .unwrap()
                                                            .create_view(&Default::default());
                                                        let panel_size =
                                                            xr_panel_layer_size(s.view_size);
                                                        s.gpu.render_xr_native_views(
                                                            &left_view,
                                                            &right_view,
                                                            Some(&msaa_color_view),
                                                            &depth_view,
                                                            [
                                                                s.view_size.width as f32
                                                                    / s.view_scale_factor as f32,
                                                                s.view_size.height as f32
                                                                    / s.view_scale_factor as f32,
                                                            ],
                                                            native_scale,
                                                            [panel_size.width, panel_size.height],
                                                            s.clear_color,
                                                            &native_plan.native_static_commands,
                                                            &native_plan.native_ghost_commands,
                                                            s.scroll_y,
                                                            xr_native_page_mvp(
                                                                &frame_data.views[0],
                                                            ),
                                                            xr_native_page_mvp(
                                                                &frame_data.views[1],
                                                            ),
                                                        );
                                                        s.gpu.render_xr_textured_layers(
                                                            &left_view,
                                                            &right_view,
                                                            Some(&msaa_color_view),
                                                            &depth_view,
                                                            [
                                                                s.view_size.width as f32
                                                                    / s.view_scale_factor as f32,
                                                                s.view_size.height as f32
                                                                    / s.view_scale_factor as f32,
                                                            ],
                                                            [panel_size.width, panel_size.height],
                                                            s.scroll_y,
                                                            xr_native_page_mvp(
                                                                &frame_data.views[0],
                                                            ),
                                                            xr_native_page_mvp(
                                                                &frame_data.views[1],
                                                            ),
                                                            &native_plan.promoted_layers,
                                                        );
                                                        let _ =
                                                            s.gpu.device.poll(wgpu::PollType::Poll);
                                                        if let Err(e) =
                                                            xr.release_and_end_frame(&frame_data)
                                                        {
                                                            log::error!(
                                                                "XR native end frame error: {:?}",
                                                                e
                                                            );
                                                        }
                                                    } else {
                                                        let recreate_target = s
                                                            .xr_panel_target
                                                            .as_ref()
                                                            .map(|target| {
                                                                target.size != s.view_size
                                                            })
                                                            .unwrap_or(true);
                                                        if recreate_target {
                                                            s.xr_panel_target =
                                                                Some(s.gpu.create_render_target(
                                                                    s.view_size.width,
                                                                    s.view_size.height,
                                                                ));
                                                        }

                                                        let (gpu, panel_target) = (
                                                            &mut s.gpu,
                                                            s.xr_panel_target.as_ref().unwrap(),
                                                        );
                                                        gpu.render_to_target(
                                                            panel_target,
                                                            s.view_scale_factor as f32,
                                                            &s.static_commands,
                                                            &overlay_commands,
                                                            s.clear_color,
                                                            s.scroll_y,
                                                            &canvases,
                                                        );
                                                        gpu.render_xr_panel_views(
                                                            &left_view,
                                                            &right_view,
                                                            &panel_target.sample_view,
                                                            [0.01, 0.015, 0.025, 1.0],
                                                            xr_panel_mvp(
                                                                &frame_data.views[0],
                                                                panel_target.size,
                                                            ),
                                                            xr_panel_mvp(
                                                                &frame_data.views[1],
                                                                panel_target.size,
                                                            ),
                                                        );
                                                        let _ =
                                                            s.gpu.device.poll(wgpu::PollType::Poll);
                                                        if let Err(e) =
                                                            xr.release_and_end_frame(&frame_data)
                                                        {
                                                            log::error!(
                                                                "XR end frame error: {:?}",
                                                                e
                                                            );
                                                        }
                                                    }
                                                }
                                                Err(e) => {
                                                    log::error!("XR acquire error: {:?}", e);
                                                    if let Err(end_err) = xr
                                                        .end_frame_without_layers(
                                                            frame_data.predicted_display_time,
                                                        )
                                                    {
                                                        log::error!(
                                                            "XR end frame after acquire failure error: {:?}",
                                                            end_err
                                                        );
                                                    }
                                                }
                                            }
                                        }

                                        xr_rendered = true;
                                    }
                                }
                                Ok(None) => {}
                                Err(e) => log::error!("XR wait_frame error: {:?}", e),
                            }
                        }

                        if xr.state() == xr_session::XrState::Stopping {
                            let s = self.state.as_mut().unwrap();
                            s.page_xr_active = false;
                            s.xr_right_select_down = false;
                            s.xr_left_select_down = false;
                            s.xr_right_pointer_pos = None;
                            s.xr_left_pointer_pos = None;
                            s.xr_exit_down = false;
                            log::info!("XR session stopped");
                        } else {
                            self.state.as_mut().unwrap().xr_session = Some(xr);
                        }
                        if let Some(href) = xr_navigation.take() {
                            self.navigate(&href);
                        }
                    }
                    if xr_rendered {
                        self.state.as_ref().unwrap().gpu.window.request_redraw();
                        return;
                    }
                }

                // Normal 2D panel rendering
                #[cfg(feature = "js")]
                {
                    self.tick_dom();
                }
                let s = self.state.as_mut().unwrap();

                #[cfg(feature = "js")]
                {
                    let bridge_ref = s.js.webgpu_bridge();
                    let canvases: Vec<([f32; 4], &wgpu::TextureView)> =
                        if let Some(ref bridge) = *bridge_ref {
                            let elem_rects = s.layout.collect_element_rects();
                            bridge
                                .canvas_ids()
                                .iter()
                                .filter_map(|id| {
                                    let tv = bridge.get_canvas_texture_view(id)?;
                                    let lr = elem_rects.get(id)?;
                                    Some(([lr.x, lr.y, lr.w, lr.h], tv))
                                })
                                .collect()
                        } else {
                            Vec::new()
                        };
                    s.gpu.render(
                        &s.static_commands,
                        &s.ghost_commands,
                        s.clear_color,
                        s.scroll_y,
                        &canvases,
                    );
                }
                #[cfg(not(feature = "js"))]
                s.gpu.render(
                    &s.static_commands,
                    &s.ghost_commands,
                    s.clear_color,
                    s.scroll_y,
                    &[],
                );
                s.gpu.window.request_redraw();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        #[cfg(target_os = "android")]
        {
            if let Some(window_id) = self.state.as_ref().map(|s| s.gpu.window.id()) {
                self.window_event(_event_loop, window_id, WindowEvent::RedrawRequested);
            }
            return;
        }

        #[cfg(not(target_os = "android"))]
        if let Some(ref s) = self.state {
            s.gpu.window.request_redraw();
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
pub extern "system" fn Java_com_example_custom_1webview_VrActivity_nativeSetResumed(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    resumed: u8,
) {
    ANDROID_ACTIVITY_RESUMED.store(resumed != 0, Ordering::Relaxed);
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_com_example_custom_1webview_VrActivity_nativeSetFocused(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    focused: u8,
) {
    ANDROID_ACTIVITY_FOCUSED.store(focused != 0, Ordering::Relaxed);
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

    ANDROID_ACTIVITY_RESUMED.store(false, Ordering::Relaxed);
    ANDROID_ACTIVITY_FOCUSED.store(false, Ordering::Relaxed);

    log::info!("android_main: starting");

    loop {
        match EventLoop::builder().with_android_app(app.clone()).build() {
            Ok(event_loop) => {
                event_loop.set_control_flow(ControlFlow::Poll);
                let mut application = App::default();
                let _ = event_loop.run_app(&mut application);
                break;
            }
            Err(e) => {
                if e.to_string().contains("EventLoop can't be recreated") {
                    log::info!("android_main: event loop already exists; keeping current loop");
                    break;
                }
                log::warn!("event loop build failed ({e}), retrying...");
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        }
    }
}
