#![allow(clippy::disallowed_types, dead_code, static_mut_refs)]
#[cfg(target_os = "android")]
pub mod android_handoff;
pub mod css_engine;
pub mod gpu;
pub mod html_parser;
#[cfg(feature = "js")]
pub mod js_bridge;
pub mod layout;
pub mod renderer;
pub mod types;
#[cfg(feature = "js")]
pub mod webgpu_bridge;
#[cfg(feature = "xr")]
pub mod xr_session;

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
    pub const WEBGPU_HTML: &str = include_str!("../assets/webgpu.html");
    pub const WEBXR_HTML: &str = include_str!("../assets/webxr.html");

    pub fn get(name: &str) -> Option<&'static str> {
        match name {
            "index.html" => Some(INDEX_HTML),
            "todo.html" => Some(TODO_HTML),
            "about.html" => Some(ABOUT_HTML),
            "canvas.html" => Some(CANVAS_HTML),
            "css3d.html" => Some(CSS3D_HTML),
            "webgpu.html" => Some(WEBGPU_HTML),
            "webxr.html" => Some(WEBXR_HTML),
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
    pub current_asset: String,
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
    #[cfg(feature = "xr")]
    pub xr_context: Option<xr_session::XrContext>,
    #[cfg(feature = "xr")]
    pub xr_session: Option<xr_session::XrSession>,
    #[cfg(feature = "xr")]
    pub xr_depth_texture: Option<wgpu::Texture>,
    #[cfg(feature = "xr")]
    pub xr_depth_size: (u32, u32),
}

#[derive(Default)]
pub struct App {
    pub state: Option<WebviewState>,
    #[cfg(target_os = "android")]
    pub android_app: Option<winit::platform::android::activity::AndroidApp>,
    #[cfg(target_os = "android")]
    pub immersive_activity: bool,
    #[cfg(target_os = "android")]
    pub launch_asset: Option<String>,
    #[cfg(target_os = "android")]
    pub auto_enter_vr: bool,
}

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
        let initial_asset = self
            .launch_asset
            .clone()
            .unwrap_or_else(|| "index.html".to_string());
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
            current_asset: initial_asset,
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
            #[cfg(feature = "xr")]
            xr_context,
            #[cfg(feature = "xr")]
            xr_session: None,
            #[cfg(feature = "xr")]
            xr_depth_texture: None,
            #[cfg(feature = "xr")]
            xr_depth_size: (0, 0),
        });

        #[cfg(all(target_os = "android", feature = "js"))]
        if self.immersive_activity && self.auto_enter_vr {
            self.auto_enter_vr = false;
            let state = self.state.as_mut().unwrap();
            if state.current_asset == "webxr.html" {
                log::info!("Auto-entering XR in immersive activity");
                if let Err(e) = state.js.eval_script("enterVR();") {
                    log::error!("failed to auto-enter XR: {:?}", e);
                }
            }
        }

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
                if self.state.is_none() {
                    return;
                }

                // XR session lifecycle management
                #[cfg(all(feature = "js", feature = "xr"))]
                {
                    let s = self.state.as_mut().unwrap();
                    if s.js.take_xr_request() && s.xr_session.is_none() {
                        #[cfg(target_os = "android")]
                        if !self.immersive_activity {
                            if let Some(app) = self.android_app.as_ref() {
                                match android_handoff::launch_immersive_activity(
                                    app,
                                    &s.current_asset,
                                    true,
                                ) {
                                    Ok(()) => {
                                        log::info!(
                                            "Launched immersive activity for {}",
                                            s.current_asset
                                        );
                                        return;
                                    }
                                    Err(e) => {
                                        log::error!("Failed to launch immersive activity: {:?}", e);
                                    }
                                }
                            }
                        }

                        log::info!("Creating XR session from JS request");
                        if let Some(ctx) = s.xr_context.as_ref() {
                            match xr_session::XrSession::new(
                                ctx,
                                &s.gpu.instance,
                                &s.gpu.adapter,
                                &s.gpu.device,
                            ) {
                                Ok(session) => {
                                    s.xr_session = Some(session);
                                    log::info!("XR session created");
                                }
                                Err(e) => log::error!("Failed to create XR session: {:?}", e),
                            }
                        } else {
                            log::error!("Cannot create XR session without XR context");
                        }
                    }
                    if s.js.take_xr_end_request() {
                        s.xr_session = None;
                        log::info!("XR session ended");
                    }
                }

                // XR render loop (when session is active)
                #[cfg(all(feature = "xr", feature = "js"))]
                {
                    let s = self.state.as_mut().unwrap();
                    let mut xr_rendered = false;
                    if let Some(ref mut xr) = s.xr_session {
                        if let Err(e) = xr.poll_events() {
                            log::error!("XR poll error: {:?}", e);
                        }
                        if xr.state() == xr_session::XrState::Running {
                            let (sw, sh) = xr.swapchain_size();
                            match xr.wait_frame() {
                                Ok(Some(frame_data)) => match xr.acquire_swapchain_image() {
                                    Ok((texture, _idx)) => {
                                        // Create per-eye views from the array texture
                                        let left_view =
                                            texture.create_view(&wgpu::TextureViewDescriptor {
                                                dimension: Some(wgpu::TextureViewDimension::D2),
                                                base_array_layer: 0,
                                                array_layer_count: Some(1),
                                                ..Default::default()
                                            });
                                        let right_view =
                                            texture.create_view(&wgpu::TextureViewDescriptor {
                                                dimension: Some(wgpu::TextureViewDimension::D2),
                                                base_array_layer: 1,
                                                array_layer_count: Some(1),
                                                ..Default::default()
                                            });

                                        // Create or reuse depth texture
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

                                        // Register in WebGpuBridge
                                        let (left_vh, right_vh, depth_vh) = {
                                            let mut bridge = s.js.webgpu_bridge_mut();
                                            let gpu = bridge.as_mut().unwrap();
                                            let lvh = gpu.register_texture_view(left_view);
                                            let rvh = gpu.register_texture_view(right_view);
                                            let dvh = gpu.register_texture_view(depth_view);
                                            (lvh, rvh, dvh)
                                        };

                                        // Provide view data to JS
                                        let left_eye = js_bridge::XrEyeData {
                                            tex_handle: 0, // not needed, JS uses view handle
                                            view_handle: left_vh,
                                            width: sw,
                                            height: sh,
                                            proj_matrix: frame_data.views[0].projection_matrix,
                                            view_inv_matrix: frame_data.views[0].view_matrix,
                                        };
                                        let right_eye = js_bridge::XrEyeData {
                                            tex_handle: 0,
                                            view_handle: right_vh,
                                            width: sw,
                                            height: sh,
                                            proj_matrix: frame_data.views[1].projection_matrix,
                                            view_inv_matrix: frame_data.views[1].view_matrix,
                                        };

                                        // Set depth view handle on both eyes
                                        // (JS will read it from the sub-image)
                                        let left_eye = js_bridge::XrEyeData {
                                            tex_handle: depth_vh, // repurpose: JS depth view
                                            ..left_eye
                                        };
                                        let right_eye = js_bridge::XrEyeData {
                                            tex_handle: depth_vh,
                                            ..right_eye
                                        };

                                        s.js.set_xr_view_data(Some(js_bridge::XrViewData {
                                            left: left_eye,
                                            right: right_eye,
                                        }));

                                        // Tick JS animation frame (this runs the XR render callback)
                                        let now_ms = s.start_time.elapsed().as_secs_f64() * 1000.0;
                                        s.js.tick(now_ms);

                                        // Clean up registered views
                                        s.js.set_xr_view_data(None);
                                        {
                                            let mut bridge = s.js.webgpu_bridge_mut();
                                            let gpu = bridge.as_mut().unwrap();
                                            gpu.unregister_texture_view(left_vh);
                                            gpu.unregister_texture_view(right_vh);
                                            gpu.unregister_texture_view(depth_vh);
                                        }

                                        let _ =
                                            s.gpu.device.poll(wgpu::PollType::wait_indefinitely());
                                        if let Err(e) = xr.release_and_end_frame(&frame_data) {
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
                                },
                                Ok(None) => {}
                                Err(e) => log::error!("XR wait_frame error: {:?}", e),
                            }
                        } else if xr.state() == xr_session::XrState::Stopping {
                            drop(s.xr_session.take());
                            log::info!("XR session stopped");
                        }
                    }
                    if xr_rendered {
                        s.gpu.window.request_redraw();
                        return;
                    }
                }

                // Normal 2D panel rendering
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
pub fn android_main(app: winit::platform::android::activity::AndroidApp) {
    use winit::platform::android::EventLoopBuilderExtAndroid;

    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );

    std::panic::set_hook(Box::new(|info| {
        log::error!("PANIC: {info}");
    }));

    log::info!("android_main: starting");

    let launch = crate::android_handoff::read_launch_options(&app).unwrap_or_else(|e| {
        log::error!("Failed to query Android launch options: {:?}", e);
        crate::android_handoff::LaunchOptions::default()
    });

    loop {
        match EventLoop::builder().with_android_app(app.clone()).build() {
            Ok(event_loop) => {
                event_loop.set_control_flow(ControlFlow::Poll);
                let mut application = App {
                    state: None,
                    android_app: Some(app.clone()),
                    immersive_activity: launch.immersive_activity,
                    launch_asset: launch.launch_asset.clone(),
                    auto_enter_vr: launch.auto_enter_vr,
                };
                let _ = event_loop.run_app(&mut application);
                break;
            }
            Err(e) => {
                log::warn!("event loop build failed ({e}), retrying...");
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        }
    }
}
