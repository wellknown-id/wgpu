use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use bytemuck::{Pod, Zeroable};
use cosmic_text::{
    Attrs, Buffer as CosmicBuffer, Family, FontSystem, Metrics, Shaping, SwashCache,
};
use wgpu::util::DeviceExt;
use winit::{event_loop::OwnedDisplayHandle, window::Window};

use crate::types::DrawCommand;

#[cfg(feature = "xr")]
fn parse_vulkan_extension_list(extension_list: &str) -> Vec<&'static std::ffi::CStr> {
    extension_list
        .split_whitespace()
        .filter_map(|name| match std::ffi::CString::new(name) {
            Ok(cstr) => Some(Box::leak(cstr.into_boxed_c_str()) as &'static std::ffi::CStr),
            Err(e) => {
                log::warn!("Ignoring invalid Vulkan extension name '{name}': {e}");
                None
            }
        })
        .collect()
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LineInstance {
    p0: [f32; 2],
    p1: [f32; 2],
    color: [f32; 4],
    width: f32,
    draw_order: f32,
    _pad: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 2],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RectInstance {
    rect: [f32; 4], // x, y, w, h
    color: [f32; 4],
    radius: f32,
    border: f32,
    border_color: [f32; 4],
    transform: [f32; 16],
    flags: u32,
    draw_order: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GlyphInstance {
    rect: [f32; 4],    // x, y, w, h in screen pixels
    uv_rect: [f32; 4], // u0, v0, u1, v1 in atlas UV
    color: [f32; 4],
    transform: [f32; 16],
    center: [f32; 2],
    flags: u32,
    draw_order: f32,
}

enum InternalDrawGroup {
    Rects(Vec<RectInstance>),
    Glyphs(Vec<GlyphInstance>),
    Lines(Vec<LineInstance>),
}

struct GlyphAtlas {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
    cursor_x: u32,
    cursor_y: u32,
    row_height: u32,
    cache: HashMap<GlyphKey, GlyphEntry>,
}

#[derive(Hash, Eq, PartialEq, Clone)]
struct GlyphKey {
    glyph_id: u16,
    font_size_tenths: u32,
}

#[derive(Clone)]
struct GlyphEntry {
    uv: [f32; 4],
    width: u32,
    height: u32,
    offset_x: i32,
    offset_y: i32,
}

const ATLAS_SIZE: u32 = 2048;

impl GlyphAtlas {
    fn new(device: &wgpu::Device) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());

        Self {
            texture,
            view,
            width: ATLAS_SIZE,
            height: ATLAS_SIZE,
            cursor_x: 0,
            cursor_y: 0,
            row_height: 0,
            cache: HashMap::new(),
        }
    }

    fn get_or_insert(
        &mut self,
        queue: &wgpu::Queue,
        font_system: &mut FontSystem,
        swash_cache: &mut SwashCache,
        cache_key: cosmic_text::CacheKey,
        font_size: f32,
    ) -> Option<GlyphEntry> {
        let key = GlyphKey {
            glyph_id: cache_key.glyph_id,
            font_size_tenths: (font_size * 10.0) as u32,
        };

        if let Some(entry) = self.cache.get(&key) {
            return Some(entry.clone());
        }

        let image = swash_cache.get_image_uncached(font_system, cache_key)?;

        let w = image.placement.width;
        let h = image.placement.height;
        if w == 0 || h == 0 {
            let entry = GlyphEntry {
                uv: [0.0; 4],
                width: 0,
                height: 0,
                offset_x: image.placement.left,
                offset_y: image.placement.top,
            };
            self.cache.insert(key, entry.clone());
            return Some(entry);
        }

        if self.cursor_x + w > self.width {
            self.cursor_x = 0;
            self.cursor_y += self.row_height + 1;
            self.row_height = 0;
        }
        if self.cursor_y + h > self.height {
            return None;
        }

        let alpha_data: Vec<u8> = match image.content {
            cosmic_text::SwashContent::Mask => image.data.clone(),
            cosmic_text::SwashContent::Color => image
                .data
                .chunks(4)
                .map(|px| px.get(3).copied().unwrap_or(255))
                .collect(),
            cosmic_text::SwashContent::SubpixelMask => image
                .data
                .chunks(3)
                .map(|px| {
                    let r = px[0] as u16;
                    let g = px.get(1).copied().unwrap_or(0) as u16;
                    let b = px.get(2).copied().unwrap_or(0) as u16;
                    ((r + g + b) / 3) as u8
                })
                .collect(),
        };

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: self.cursor_x,
                    y: self.cursor_y,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &alpha_data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );

        let u0 = self.cursor_x as f32 / self.width as f32;
        let v0 = self.cursor_y as f32 / self.height as f32;
        let u1 = (self.cursor_x + w) as f32 / self.width as f32;
        let v1 = (self.cursor_y + h) as f32 / self.height as f32;

        let entry = GlyphEntry {
            uv: [u0, v0, u1, v1],
            width: w,
            height: h,
            offset_x: image.placement.left,
            offset_y: image.placement.top,
        };

        self.cursor_x += w + 1;
        self.row_height = self.row_height.max(h);
        self.cache.insert(key, entry.clone());

        Some(entry)
    }
}

const RECT_SHADER: &str = r#"
struct ScreenUniform {
    size: vec2<f32>,
    srgb_target: f32,
    scroll_y: f32,
};

fn linearize(c: vec3<f32>, srgb: f32) -> vec3<f32> {
    return mix(c, pow(c, vec3<f32>(2.2)), srgb);
}
@group(0) @binding(0) var<uniform> screen: ScreenUniform;

struct RectInput {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) rect: vec4<f32>,
    @location(3) color: vec4<f32>,
    @location(4) radius: f32,
    @location(5) border: f32,
    @location(6) border_color: vec4<f32>,
    @location(7) transform_0: vec4<f32>,
    @location(8) transform_1: vec4<f32>,
    @location(9) transform_2: vec4<f32>,
    @location(10) transform_3: vec4<f32>,
    @location(11) flags: u32,
    @location(12) draw_order: f32,
};

struct RectOutput {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) local_pos: vec2<f32>,
    @location(2) rect_size: vec2<f32>,
    @location(3) radius: f32,
    @location(4) border: f32,
    @location(5) border_color: vec4<f32>,
};

@vertex
fn vs_rect(in: RectInput) -> RectOutput {
    var out: RectOutput;
    
    // Convert normalized instance pos to local coordinates (centered at 0,0)
    let local_pos = vec4<f32>(in.pos.x * in.rect.z - in.rect.z * 0.5, 
                             in.pos.y * in.rect.w - in.rect.w * 0.5, 
                             0.0, 1.0);
    
    // Apply transform matrix
    let matrix = mat4x4<f32>(in.transform_0, in.transform_1, in.transform_2, in.transform_3);
    let world_pos_4 = matrix * local_pos;
    
    // Add original center position and subtract scroll
    // Add original center position and subtract scroll if not fixed (flag bit 0)
    let is_fixed = (in.flags & 1u) != 0u;
    let s = select(vec2<f32>(0.0, screen.scroll_y), vec2<f32>(0.0), is_fixed);
    
    let w = world_pos_4.w;
    let z = world_pos_4.z;
    // Screen-space center offset, scaled by w for correct homogeneous coords
    let cx = (in.rect.x + in.rect.z * 0.5 - s.x);
    let cy = (in.rect.y + in.rect.w * 0.5 - s.y);
    
    let x = world_pos_4.x + cx * w;
    let y = world_pos_4.y + cy * w;

    let nx = (x / (screen.size.x * w)) * 2.0 - 1.0;
    let ny = (1.0 - (y / (screen.size.y * w))) * 2.0 - 1.0;
    
    let depth_ndc = (1.0 - in.draw_order * 0.001) - z / 50000.0;
    out.pos = vec4<f32>(nx * w, ny * w, depth_ndc * w, w); 
    
    out.color = in.color;
    out.local_pos = vec2<f32>(in.pos.x * in.rect.z, in.pos.y * in.rect.w);
    out.rect_size = vec2<f32>(in.rect.z, in.rect.w);
    out.radius = in.radius;
    out.border = in.border;
    out.border_color = in.border_color;
    return out;
}

fn rounded_rect_sdf(pos: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(pos) - half_size + vec2<f32>(radius);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - radius;
}

@fragment
fn fs_rect(in: RectOutput) -> @location(0) vec4<f32> {
    let half_size = in.rect_size * 0.5;
    let radius = in.radius;
    let border = in.border;
    let centered = in.local_pos - half_size;
    let r = min(radius, min(half_size.x, half_size.y));
    let dist = rounded_rect_sdf(centered, half_size, r);

    if dist > 0.5 {
        discard;
    }

    let alpha = 1.0 - smoothstep(-0.5, 0.5, dist);

    if border > 0.0 {
        let inner_dist = rounded_rect_sdf(centered, half_size - vec2(border), max(r - border, 0.0));
        let inner_alpha = smoothstep(-0.5, 0.5, inner_dist);
        let fill = in.color * (1.0 - inner_alpha);
        let border_fill = in.border_color * inner_alpha;
        return vec4<f32>(linearize(fill.rgb + border_fill.rgb, screen.srgb_target), alpha * max(fill.a, border_fill.a));
    }

    return vec4<f32>(linearize(in.color.rgb, screen.srgb_target), in.color.a * alpha);
}
"#;

const GLYPH_SHADER: &str = r#"
struct ScreenUniform {
    size: vec2<f32>,
    srgb_target: f32,
    scroll_y: f32,
};

fn linearize(c: vec3<f32>, srgb: f32) -> vec3<f32> {
    return mix(c, pow(c, vec3<f32>(2.2)), srgb);
}
@group(0) @binding(0) var<uniform> screen: ScreenUniform;
@group(0) @binding(1) var glyph_tex: texture_2d<f32>;
@group(0) @binding(2) var glyph_sampler: sampler;

struct GlyphInput {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) rect: vec4<f32>,
    @location(3) uv_rect: vec4<f32>,
    @location(4) color: vec4<f32>,
    @location(5) transform_0: vec4<f32>,
    @location(6) transform_1: vec4<f32>,
    @location(7) transform_2: vec4<f32>,
    @location(8) transform_3: vec4<f32>,
    @location(9) center: vec2<f32>,
    @location(10) flags: u32,
    @location(11) draw_order: f32,
};

struct GlyphOutput {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_glyph(in: GlyphInput) -> GlyphOutput {
    var out: GlyphOutput;
    // For text, the given in.rect is the individual glyph bounding box.
    // Calculate raw position of the vertex
    let raw_x = in.rect.x + in.pos.x * in.rect.z;
    let raw_y = in.rect.y + in.pos.y * in.rect.w;
    
    // Offset by container's center before applying transform
    let local_pos = vec4<f32>(raw_x - in.center.x, raw_y - in.center.y, 0.0, 1.0);
    let matrix = mat4x4<f32>(in.transform_0, in.transform_1, in.transform_2, in.transform_3);
    let world_pos_4 = matrix * local_pos;

    let is_fixed = (in.flags & 1u) != 0u;
    let s = select(vec2<f32>(0.0, screen.scroll_y), vec2<f32>(0.0), is_fixed);
    
    let w = world_pos_4.w;
    let z = world_pos_4.z;
    let cx = in.center.x - s.x;
    let cy = in.center.y - s.y;
    
    let x = world_pos_4.x + cx * w;
    let y = world_pos_4.y + cy * w;

    let nx = (x / (screen.size.x * w)) * 2.0 - 1.0;
    let ny = (1.0 - (y / (screen.size.y * w))) * 2.0 - 1.0;
    
    let depth_ndc = (1.0 - in.draw_order * 0.001) - z / 50000.0;
    out.pos = vec4<f32>(nx * w, ny * w, depth_ndc * w, w); 
    let u = in.uv_rect.x + in.uv.x * (in.uv_rect.z - in.uv_rect.x);
    let v = in.uv_rect.y + in.uv.y * (in.uv_rect.w - in.uv_rect.y);
    out.uv = vec2<f32>(u, v);
    out.color = in.color;
    return out;
}

@fragment
fn fs_glyph(in: GlyphOutput) -> @location(0) vec4<f32> {
    let coverage = textureSample(glyph_tex, glyph_sampler, in.uv).r;
    return vec4<f32>(linearize(in.color.rgb, screen.srgb_target), in.color.a * coverage);
}
"#;

const LINE_SHADER: &str = r#"
struct ScreenUniform {
    size: vec2<f32>,
    srgb_target: f32,
    scroll_y: f32,
};

fn linearize(c: vec3<f32>, srgb: f32) -> vec3<f32> {
    return mix(c, pow(c, vec3<f32>(2.2)), srgb);
}
@group(0) @binding(0) var<uniform> screen: ScreenUniform;

struct LineInput {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) p0: vec2<f32>,
    @location(3) p1: vec2<f32>,
    @location(4) color: vec4<f32>,
    @location(5) width: f32,
    @location(6) draw_order: f32,
};

struct LineOutput {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_line(in: LineInput) -> LineOutput {
    var out: LineOutput;
    let dir = in.p1 - in.p0;
    let len = length(dir);
    var normal: vec2<f32>;
    if len > 0.001 {
        normal = vec2<f32>(-dir.y, dir.x) / len;
    } else {
        normal = vec2<f32>(0.0, 1.0);
    }
    let base = mix(in.p0, in.p1, in.pos.x);
    let offset = normal * (in.pos.y * 2.0 - 1.0) * in.width * 0.5;
    let screen_pos = base + offset;
    let x = screen_pos.x;
    let y = screen_pos.y - screen.scroll_y;
    let nx = (x / screen.size.x) * 2.0 - 1.0;
    let ny = (1.0 - (y / screen.size.y)) * 2.0 - 1.0;
    let depth_ndc = 1.0 - in.draw_order * 0.001;
    out.pos = vec4<f32>(nx, ny, depth_ndc, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_line(in: LineOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(linearize(in.color.rgb, screen.srgb_target), in.color.a);
}
"#;

const TEXTURED_RECT_SHADER: &str = r#"
struct ScreenUniform {
    size: vec2<f32>,
    srgb_target: f32,
    scroll_y: f32,
};

fn linearize(c: vec3<f32>, srgb: f32) -> vec3<f32> {
    return mix(c, pow(c, vec3<f32>(2.2)), srgb);
}
@group(0) @binding(0) var<uniform> screen: ScreenUniform;

struct TexRectInput {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) rect: vec4<f32>,
    @location(3) draw_order: f32,
};

struct TexRectOutput {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_texrect(in: TexRectInput) -> TexRectOutput {
    var out: TexRectOutput;
    let x = in.rect.x + in.pos.x * in.rect.z;
    let y = in.rect.y + in.pos.y * in.rect.w - screen.scroll_y;
    let nx = (x / screen.size.x) * 2.0 - 1.0;
    let ny = (1.0 - (y / screen.size.y)) * 2.0 - 1.0;
    let depth = 1.0 - in.draw_order * 0.001;
    out.pos = vec4<f32>(nx, ny, depth, 1.0);
    out.uv = in.uv;
    return out;
}

@group(1) @binding(0) var canvas_tex: texture_2d<f32>;
@group(1) @binding(1) var canvas_sampler: sampler;

@fragment
fn fs_texrect(in: TexRectOutput) -> @location(0) vec4<f32> {
    let c = textureSample(canvas_tex, canvas_sampler, in.uv);
    return vec4<f32>(linearize(c.rgb, screen.srgb_target), c.a);
}
"#;

pub struct GpuState {
    pub window: Arc<Window>,
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    pub surface: wgpu::Surface<'static>,
    pub surface_format: wgpu::TextureFormat,
    render_format: wgpu::TextureFormat,
    pub size: winit::dpi::PhysicalSize<u32>,
    pub scale_factor: f64,

    screen_buffer: wgpu::Buffer,
    vertex_buffer: wgpu::Buffer,

    rect_pipeline: wgpu::RenderPipeline,
    rect_bind_group: wgpu::BindGroup,

    glyph_pipeline: wgpu::RenderPipeline,
    glyph_bind_group: wgpu::BindGroup,

    line_pipeline: wgpu::RenderPipeline,

    texrect_pipeline: wgpu::RenderPipeline,
    texrect_bgl: wgpu::BindGroupLayout,
    texrect_sampler: wgpu::Sampler,

    atlas: GlyphAtlas,
    pub font_system: FontSystem,
    swash_cache: SwashCache,
    pub static_dirty: bool,
    cached_static_groups: Vec<InternalDrawGroup>,
    depth_view: wgpu::TextureView,
}

impl GpuState {
    pub async fn new(
        display: OwnedDisplayHandle,
        window: Arc<Window>,
        #[cfg(feature = "xr")] xr_context: Option<&crate::xr_session::XrContext>,
    ) -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
            Box::new(display),
        ));

        let mut target_adapter = None;
        #[cfg(feature = "xr")]
        if let Some(xr_ctx) = xr_context {
            if let Ok(target_pd) = xr_ctx.vulkan_graphics_device(&instance) {
                target_adapter = instance
                    .enumerate_adapters(wgpu::Backends::VULKAN)
                    .await
                    .into_iter()
                    .find(|a| {
                        crate::xr_session::extract_vulkan_physical_device(a).unwrap_or(0)
                            == target_pd
                    });
            }
        }

        let adapter = if let Some(a) = target_adapter {
            a
        } else {
            instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await?
        };

        let device_desc = wgpu::DeviceDescriptor::default();
        let (device, queue) = {
            #[cfg(feature = "xr")]
            {
                if let Some(xr_ctx) = xr_context {
                    if let Ok(exts) = xr_ctx
                        .instance
                        .vulkan_legacy_device_extensions(xr_ctx.system)
                    {
                        let xr_device_extensions = parse_vulkan_extension_list(&exts);
                        if !xr_device_extensions.is_empty() {
                            use wgpu::hal;

                            let xr_device_extensions_count = xr_device_extensions.len();
                            if let Some(hal_adapter) =
                                unsafe { adapter.as_hal::<hal::api::Vulkan>() }
                            {
                                let callback_exts = xr_device_extensions.clone();
                                let open_result = unsafe {
                                    hal_adapter.open_with_callback(
                                        device_desc.required_features,
                                        &device_desc.required_limits,
                                        &device_desc.memory_hints,
                                        Some(Box::new(move |args| {
                                            for &xr_ext in &callback_exts {
                                                if !args.extensions.contains(&xr_ext) {
                                                    args.extensions.push(xr_ext);
                                                }
                                            }
                                        })),
                                    )
                                };

                                match open_result {
                                    Ok(hal_device) => {
                                        log::info!(
                                            "Creating Vulkan device with {xr_device_extensions_count} XR-required extension(s)"
                                        );
                                        if let Ok(pair) = unsafe {
                                            adapter.create_device_from_hal::<hal::api::Vulkan>(
                                                hal_device,
                                                &device_desc,
                                            )
                                        } {
                                            pair
                                        } else {
                                            log::warn!(
                                                "create_device_from_hal failed; falling back to request_device"
                                            );
                                            adapter.request_device(&device_desc).await?
                                        }
                                    }
                                    Err(e) => {
                                        log::warn!(
                                            "open_with_callback failed ({e:?}); falling back to request_device"
                                        );
                                        adapter.request_device(&device_desc).await?
                                    }
                                }
                            } else {
                                log::warn!(
                                    "Vulkan HAL adapter unavailable; falling back to request_device"
                                );
                                adapter.request_device(&device_desc).await?
                            }
                        } else {
                            adapter.request_device(&device_desc).await?
                        }
                    } else {
                        log::warn!(
                            "Failed to query XR-required Vulkan device extensions; falling back to request_device"
                        );
                        adapter.request_device(&device_desc).await?
                    }
                } else {
                    adapter.request_device(&device_desc).await?
                }
            }
            #[cfg(not(feature = "xr"))]
            {
                adapter.request_device(&device_desc).await?
            }
        };
        let device = Arc::new(device);
        let queue = Arc::new(queue);

        let size = window.inner_size();
        let surface = instance.create_surface(window.clone())?;
        let caps = surface.get_capabilities(&adapter);
        let surface_format = caps
            .formats
            .first()
            .copied()
            .ok_or_else(|| anyhow!("no surface format"))?;

        let supports_view_formats = adapter
            .get_downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::SURFACE_VIEW_FORMATS);

        let render_format = if supports_view_formats {
            surface_format.remove_srgb_suffix()
        } else {
            surface_format
        };

        let view_formats = if render_format != surface_format {
            vec![render_format]
        } else {
            vec![]
        };

        surface.configure(
            &device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: surface_format,
                width: size.width.max(1),
                height: size.height.max(1),
                present_mode: wgpu::PresentMode::AutoVsync,
                alpha_mode: wgpu::CompositeAlphaMode::Auto,
                view_formats: view_formats.clone(),
                desired_maximum_frame_latency: 2,
            },
        );

        let depth_view = create_depth_view(&device, size.width, size.height);

        let screen_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screen uniform"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("quad verts"),
            contents: bytemuck::cast_slice(&[
                Vertex {
                    position: [0.0, 0.0],
                    uv: [0.0, 0.0],
                },
                Vertex {
                    position: [1.0, 0.0],
                    uv: [1.0, 0.0],
                },
                Vertex {
                    position: [0.0, 1.0],
                    uv: [0.0, 1.0],
                },
                Vertex {
                    position: [1.0, 0.0],
                    uv: [1.0, 0.0],
                },
                Vertex {
                    position: [1.0, 1.0],
                    uv: [1.0, 1.0],
                },
                Vertex {
                    position: [0.0, 1.0],
                    uv: [0.0, 1.0],
                },
            ]),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let atlas = GlyphAtlas::new(&device);

        // -- rect pipeline --
        let rect_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rect bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let rect_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rect bg"),
            layout: &rect_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: screen_buffer.as_entire_binding(),
            }],
        });
        let rect_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rect shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(RECT_SHADER)),
        });
        let rect_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rect pl"),
            bind_group_layouts: &[Some(&rect_bgl)],
            immediate_size: 0,
        });
        let rect_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rect pipeline"),
            layout: Some(&rect_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &rect_shader,
                entry_point: Some("vs_rect"),
                compilation_options: Default::default(),
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<RectInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![
                            2 => Float32x4,
                            3 => Float32x4,
                            4 => Float32,
                            5 => Float32,
                            6 => Float32x4,
                            7 => Float32x4,
                            8 => Float32x4,
                            9 => Float32x4,
                            10 => Float32x4,
                            11 => Uint32,
                            12 => Float32,
                        ],
                    },
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &rect_shader,
                entry_point: Some("fs_rect"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: render_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        // -- glyph pipeline --
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let glyph_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glyph bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let glyph_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glyph bg"),
            layout: &glyph_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: screen_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&atlas.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let glyph_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("glyph shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(GLYPH_SHADER)),
        });
        let glyph_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("glyph pl"),
                bind_group_layouts: &[Some(&glyph_bgl)],
                immediate_size: 0,
            });
        let glyph_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("glyph pipeline"),
            layout: Some(&glyph_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &glyph_shader,
                entry_point: Some("vs_glyph"),
                compilation_options: Default::default(),
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<GlyphInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![
                            2 => Float32x4,   // rect
                            3 => Float32x4,   // uv
                            4 => Float32x4,   // color
                            5 => Float32x4,   // transform 0
                            6 => Float32x4,   // transform 1
                            7 => Float32x4,   // transform 2
                            8 => Float32x4,   // transform 3
                            9 => Float32x2,   // center
                            10 => Uint32,     // flags
                            11 => Float32,    // draw_order
                        ],
                    },
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &glyph_shader,
                entry_point: Some("fs_glyph"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: render_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        // -- line pipeline --
        let line_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("line shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(LINE_SHADER)),
        });
        let line_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("line pl"),
            bind_group_layouts: &[Some(&rect_bgl)],
            immediate_size: 0,
        });
        let line_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("line pipeline"),
            layout: Some(&line_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &line_shader,
                entry_point: Some("vs_line"),
                compilation_options: Default::default(),
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<LineInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![
                            2 => Float32x2,
                            3 => Float32x2,
                            4 => Float32x4,
                            5 => Float32,
                            6 => Float32,
                        ],
                    },
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &line_shader,
                entry_point: Some("fs_line"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: render_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        // -- textured rect pipeline (for WebGPU canvas compositing) --
        let texrect_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("texrect shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(TEXTURED_RECT_SHADER)),
        });
        let texrect_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("texrect bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let texrect_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let texrect_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("texrect pl"),
            bind_group_layouts: &[Some(&rect_bgl), Some(&texrect_bgl)],
            immediate_size: 0,
        });

        #[repr(C)]
        #[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
        struct TexRectInstance {
            rect: [f32; 4],
            draw_order: f32,
        }

        let texrect_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("texrect pipeline"),
            layout: Some(&texrect_pl),
            vertex: wgpu::VertexState {
                module: &texrect_shader,
                entry_point: Some("vs_texrect"),
                compilation_options: Default::default(),
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<TexRectInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![2 => Float32x4, 3 => Float32],
                    },
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &texrect_shader,
                entry_point: Some("fs_texrect"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: render_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        #[cfg(target_os = "android")]
        let font_system = {
            let mut db = cosmic_text::fontdb::Database::new();
            for name in &["DroidSans.ttf", "DroidSans-Bold.ttf", "DroidSansMono.ttf"] {
                let path = format!("/system/fonts/{name}");
                if std::path::Path::new(&path).exists() {
                    db.load_font_file(path).ok();
                }
            }
            FontSystem::new_with_locale_and_db("en-US".to_string(), db)
        };
        #[cfg(target_os = "ios")]
        let font_system = {
            let mut db = cosmic_text::fontdb::Database::new();
            db.load_fonts_dir("/System/Library/Fonts");
            db.load_fonts_dir("/System/Library/Fonts/Core");
            FontSystem::new_with_locale_and_db("en-US".to_string(), db)
        };
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        let font_system = FontSystem::new();
        let swash_cache = SwashCache::new();

        let scale_factor = window.scale_factor();

        Ok(Self {
            window,
            instance,
            adapter,
            device,
            queue,
            surface,
            surface_format,
            render_format,
            size,
            screen_buffer,
            vertex_buffer,
            rect_pipeline,
            rect_bind_group,
            glyph_pipeline,
            glyph_bind_group,
            line_pipeline,
            texrect_pipeline,
            texrect_bgl,
            texrect_sampler,
            atlas,
            font_system,
            swash_cache,
            scale_factor,
            static_dirty: true,
            cached_static_groups: Vec::new(),
            depth_view,
        })
    }

    pub fn resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
        if new_size.width > 0 && new_size.height > 0 {
            self.size = new_size;
            let view_formats = if self.render_format != self.surface_format {
                vec![self.render_format]
            } else {
                vec![]
            };
            self.surface.configure(
                &self.device,
                &wgpu::SurfaceConfiguration {
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    format: self.surface_format,
                    width: new_size.width,
                    height: new_size.height,
                    present_mode: wgpu::PresentMode::AutoVsync,
                    alpha_mode: wgpu::CompositeAlphaMode::Auto,
                    view_formats,
                    desired_maximum_frame_latency: 2,
                },
            );
            self.depth_view = create_depth_view(&self.device, new_size.width, new_size.height);
        }
    }

    pub fn render(
        &mut self,
        static_commands: &[DrawCommand],
        ghost_commands: &[DrawCommand],
        clear_color: [f32; 4],
        scroll_y: f32,
        webgpu_canvases: &[([f32; 4], &wgpu::TextureView)],
    ) {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            _ => return,
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.render_format),
            ..Default::default()
        });

        self.queue.write_buffer(
            &self.screen_buffer,
            0,
            bytemuck::cast_slice(&[
                self.size.width as f32 / self.scale_factor as f32,
                self.size.height as f32 / self.scale_factor as f32,
                if self.surface_format.is_srgb() && self.render_format == self.surface_format {
                    1.0_f32
                } else {
                    0.0_f32
                },
                scroll_y,
            ]),
        );

        if self.static_dirty {
            self.cached_static_groups = self.build_draw_groups(static_commands);
            self.static_dirty = false;
        }
        let ghost_groups = self.build_draw_groups(ghost_commands);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("webview encoder"),
            });

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("static pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.render_groups(&self.cached_static_groups, &mut rpass);
        }

        if !ghost_groups.is_empty() {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ghost pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.render_groups(&ghost_groups, &mut rpass);
        }

        // Draw WebGPU canvas textures
        if !webgpu_canvases.is_empty() {
            #[repr(C)]
            #[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
            struct TexRectInstance {
                rect: [f32; 4],
                draw_order: f32,
            }

            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("texrect pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rpass.set_pipeline(&self.texrect_pipeline);
            rpass.set_bind_group(0, Some(&self.rect_bind_group), &[]);
            rpass.set_vertex_buffer(0, self.vertex_buffer.slice(..));

            for (rect, tex_view) in webgpu_canvases {
                let instance = TexRectInstance {
                    rect: *rect,
                    draw_order: 500.0,
                };
                let instance_buf =
                    self.device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("texrect instance"),
                            contents: bytemuck::cast_slice(&[instance]),
                            usage: wgpu::BufferUsages::VERTEX,
                        });
                let tex_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("texrect bg"),
                    layout: &self.texrect_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(tex_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.texrect_sampler),
                        },
                    ],
                });
                rpass.set_bind_group(1, Some(&tex_bg), &[]);
                rpass.set_vertex_buffer(1, instance_buf.slice(..));
                rpass.draw(0..6, 0..1);
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
    }

    fn build_draw_groups(&mut self, commands: &[DrawCommand]) -> Vec<InternalDrawGroup> {
        let mut groups: Vec<InternalDrawGroup> = Vec::new();
        let mut draw_order: f32 = 0.0;
        for cmd in commands {
            match cmd {
                DrawCommand::Rect {
                    rect,
                    color,
                    border_radius,
                    transform,
                    is_fixed,
                    ..
                } => {
                    let inst = RectInstance {
                        rect: [rect.x, rect.y, rect.w, rect.h],
                        color: *color,
                        radius: *border_radius,
                        border: 0.0,
                        border_color: [0.0; 4],
                        transform: *transform,
                        flags: if *is_fixed { 1u32 } else { 0 },
                        draw_order,
                    };
                    draw_order += 1.0;
                    if let Some(InternalDrawGroup::Rects(ref mut v)) = groups.last_mut() {
                        v.push(inst);
                    } else {
                        groups.push(InternalDrawGroup::Rects(vec![inst]));
                    }
                }
                DrawCommand::Border {
                    rect,
                    color,
                    width,
                    radius,
                    transform,
                    is_fixed,
                    ..
                } => {
                    let inst = RectInstance {
                        rect: [rect.x, rect.y, rect.w, rect.h],
                        color: [0.0, 0.0, 0.0, 0.0],
                        radius: *radius,
                        border: *width,
                        border_color: *color,
                        transform: *transform,
                        flags: if *is_fixed { 1u32 } else { 0 },
                        draw_order,
                    };
                    draw_order += 1.0;
                    if let Some(InternalDrawGroup::Rects(ref mut v)) = groups.last_mut() {
                        v.push(inst);
                    } else {
                        groups.push(InternalDrawGroup::Rects(vec![inst]));
                    }
                }
                DrawCommand::Text {
                    text,
                    x,
                    y,
                    max_width,
                    color,
                    font_size,
                    is_fixed,
                    transform,
                    center,
                    ..
                } => {
                    let mut glyphs = Vec::new();
                    self.rasterize_text(
                        text,
                        *x,
                        *y,
                        *max_width,
                        *color,
                        *font_size,
                        *is_fixed,
                        *transform,
                        *center,
                        &mut glyphs,
                    );
                    if !glyphs.is_empty() {
                        for g in &mut glyphs {
                            g.draw_order = draw_order;
                        }
                        draw_order += 1.0;
                        if let Some(InternalDrawGroup::Glyphs(ref mut v)) = groups.last_mut() {
                            v.extend(glyphs);
                        } else {
                            groups.push(InternalDrawGroup::Glyphs(glyphs));
                        }
                    }
                }
                DrawCommand::Line {
                    x0,
                    y0,
                    x1,
                    y1,
                    color,
                    width,
                } => {
                    let inst = LineInstance {
                        p0: [*x0, *y0],
                        p1: [*x1, *y1],
                        color: *color,
                        width: *width,
                        draw_order,
                        _pad: [0.0; 2],
                    };
                    draw_order += 1.0;
                    if let Some(InternalDrawGroup::Lines(ref mut v)) = groups.last_mut() {
                        v.push(inst);
                    } else {
                        groups.push(InternalDrawGroup::Lines(vec![inst]));
                    }
                }
            }
        }
        groups
    }

    fn render_groups<'a>(&'a self, groups: &[InternalDrawGroup], rpass: &mut wgpu::RenderPass<'a>) {
        for group in groups {
            match group {
                InternalDrawGroup::Rects(instances) => {
                    let buf = self
                        .device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("rect instances"),
                            contents: bytemuck::cast_slice(instances),
                            usage: wgpu::BufferUsages::VERTEX,
                        });
                    rpass.set_pipeline(&self.rect_pipeline);
                    rpass.set_bind_group(0, &self.rect_bind_group, &[]);
                    rpass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                    rpass.set_vertex_buffer(1, buf.slice(..));
                    rpass.draw(0..6, 0..instances.len() as u32);
                }
                InternalDrawGroup::Glyphs(instances) => {
                    let buf = self
                        .device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("glyph instances"),
                            contents: bytemuck::cast_slice(instances),
                            usage: wgpu::BufferUsages::VERTEX,
                        });
                    rpass.set_pipeline(&self.glyph_pipeline);
                    rpass.set_bind_group(0, &self.glyph_bind_group, &[]);
                    rpass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                    rpass.set_vertex_buffer(1, buf.slice(..));
                    rpass.draw(0..6, 0..instances.len() as u32);
                }
                InternalDrawGroup::Lines(instances) => {
                    let buf = self
                        .device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("line instances"),
                            contents: bytemuck::cast_slice(instances),
                            usage: wgpu::BufferUsages::VERTEX,
                        });
                    rpass.set_pipeline(&self.line_pipeline);
                    rpass.set_bind_group(0, &self.rect_bind_group, &[]);
                    rpass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                    rpass.set_vertex_buffer(1, buf.slice(..));
                    rpass.draw(0..6, 0..instances.len() as u32);
                }
            }
        }
    }

    fn rasterize_text(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        max_width: f32,
        color: [f32; 4],
        font_size: f32,
        is_fixed: bool,
        transform: [f32; 16],
        center: [f32; 2],
        instances: &mut Vec<GlyphInstance>,
    ) {
        let scale = self.scale_factor as f32;
        let phys_font_size = font_size * scale;
        let metrics = Metrics::new(phys_font_size, phys_font_size * 1.2);
        let family = if cfg!(target_os = "android") {
            Family::Name("Roboto")
        } else {
            Family::SansSerif
        };
        let attrs = Attrs::new().family(family);
        let mut buffer = CosmicBuffer::new(&mut self.font_system, metrics);
        buffer.set_size(&mut self.font_system, Some(max_width * scale), None);
        buffer.set_text(&mut self.font_system, text, attrs, Shaping::Advanced);
        buffer.shape_until_scroll(&mut self.font_system, false);

        let inv = 1.0 / scale;
        for run in buffer.layout_runs() {
            for glyph in run.glyphs.iter() {
                let physical = glyph.physical((x * scale, y * scale), 1.0);
                if let Some(entry) = self.atlas.get_or_insert(
                    &self.queue,
                    &mut self.font_system,
                    &mut self.swash_cache,
                    physical.cache_key,
                    phys_font_size,
                ) {
                    if entry.width == 0 || entry.height == 0 {
                        continue;
                    }
                    let gx = (physical.x as f32 + entry.offset_x as f32) * inv;
                    let gy = (physical.y as f32 - entry.offset_y as f32 + run.line_y) * inv;
                    instances.push(GlyphInstance {
                        rect: [gx, gy, entry.width as f32 * inv, entry.height as f32 * inv],
                        uv_rect: entry.uv,
                        color,
                        transform,
                        center,
                        flags: if is_fixed { 1u32 } else { 0 },
                        draw_order: 0.0,
                    });
                }
            }
        }
    }

    pub fn measure_text(&mut self, text: &str, font_size: f32, max_width: f32) -> (f32, f32) {
        let line_height = font_size * 1.2;
        let metrics = Metrics::new(font_size, line_height);
        let family = if cfg!(target_os = "android") {
            Family::Name("Roboto")
        } else {
            Family::SansSerif
        };
        let attrs = Attrs::new().family(family);
        let mut buffer = CosmicBuffer::new(&mut self.font_system, metrics);
        buffer.set_size(&mut self.font_system, Some(max_width), None);
        buffer.set_text(&mut self.font_system, text, attrs, Shaping::Advanced);
        buffer.shape_until_scroll(&mut self.font_system, false);

        let mut w = 0.0f32;
        let mut h = 0.0f32;
        for run in buffer.layout_runs() {
            h += run.line_height;
            for glyph in run.glyphs {
                w = w.max(glyph.x + glyph.w);
            }
        }
        (w, h)
    }
}

fn create_depth_view(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}
