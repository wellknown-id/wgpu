use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use bytemuck::{Pod, Zeroable};
use cosmic_text::{
    fontdb::{
        Family as DbFamily, Query as DbQuery, Stretch as DbStretch, Style as DbStyle,
        Weight as DbWeight,
    },
    Attrs, Buffer as CosmicBuffer, Family, FontSystem, Metrics, Shaping, SwashCache,
};
use wgpu::util::DeviceExt;
use winit::{event_loop::OwnedDisplayHandle, window::Window};

use crate::slug::{build_font_atlas, FontAtlas as SlugFontAtlas};
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

#[cfg(feature = "xr")]
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PanelVertex {
    position: [f32; 3],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TexRectInstance {
    rect: [f32; 4],
    draw_order: f32,
}

#[cfg(feature = "xr")]
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct XrPanelUniform {
    mvp: [f32; 16],
}

#[cfg(feature = "xr")]
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct XrPageUniform {
    mvp: [f32; 16],
    page_panel: [f32; 4],
    misc: [f32; 4],
}

#[cfg(feature = "xr")]
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct XrTexRectInstance {
    rect: [f32; 4],
    transform: [f32; 16],
    flags: u32,
    draw_order: f32,
    _pad: [u32; 2],
}

#[cfg(feature = "xr")]
pub struct XrTextureLayer {
    pub view: wgpu::TextureView,
    pub rect: [f32; 4],
    pub transform: [f32; 16],
    pub is_fixed: bool,
    pub draw_order: f32,
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
    Glyphs {
        vertices: Vec<SlugVertex>,
        indices: Vec<u32>,
    },
    Lines(Vec<LineInstance>),
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SlugVertex {
    pos: [f32; 4],
    tex: [f32; 4],
    bnd: [f32; 4],
    color: [f32; 4],
    transform: [f32; 16],
    center: [f32; 2],
    flags: u32,
    draw_order: f32,
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
@group(0) @binding(1) var curve_texture: texture_2d<f32>;
@group(0) @binding(2) var band_texture: texture_2d<u32>;

struct GlyphInput {
    @location(0) pos: vec4<f32>,
    @location(1) tex: vec4<f32>,
    @location(2) bnd: vec4<f32>,
    @location(3) color: vec4<f32>,
    @location(4) transform_0: vec4<f32>,
    @location(5) transform_1: vec4<f32>,
    @location(6) transform_2: vec4<f32>,
    @location(7) transform_3: vec4<f32>,
    @location(8) center: vec2<f32>,
    @location(9) flags: u32,
    @location(10) draw_order: f32,
};

struct GlyphOutput {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) render_coord: vec2<f32>,
    @location(2) @interpolate(flat) banding: vec4<f32>,
    @location(3) @interpolate(flat) glyph: vec4<i32>,
};

@vertex
fn vs_glyph(in: GlyphInput) -> GlyphOutput {
    var out: GlyphOutput;
    let local_pos = vec4<f32>(in.pos.x - in.center.x, in.pos.y - in.center.y, 0.0, 1.0);
    let matrix = mat4x4<f32>(in.transform_0, in.transform_1, in.transform_2, in.transform_3);
    let world_pos_4 = matrix * local_pos;

    let w = world_pos_4.w;
    let z = world_pos_4.z;
    let is_fixed = (in.flags & 1u) != 0u;
    let scroll = select(screen.scroll_y, 0.0, is_fixed);
    let cx = in.center.x;
    let cy = in.center.y - scroll;
    let x = world_pos_4.x + cx * w;
    let y = world_pos_4.y + cy * w;

    let nx = (x / (screen.size.x * w)) * 2.0 - 1.0;
    let ny = (1.0 - (y / (screen.size.y * w))) * 2.0 - 1.0;
    let depth_ndc = (1.0 - in.draw_order * 0.001) - z / 50000.0;
    out.pos = vec4<f32>(nx * w, ny * w, depth_ndc * w, w);
    out.color = in.color;
    out.render_coord = in.tex.xy;
    out.banding = in.bnd;

    let glyph_loc = bitcast<u32>(in.tex.z);
    let band_max = bitcast<u32>(in.tex.w);
    out.glyph = vec4<i32>(
        i32(glyph_loc & 0xFFFFu),
        i32(glyph_loc >> 16u),
        i32(band_max & 0xFFu),
        i32((band_max >> 16u) & 0xFFu),
    );
    return out;
}

fn calc_root_code(y1: f32, y2: f32, y3: f32) -> u32 {
    let i1 = bitcast<u32>(y1) >> 31u;
    let i2 = bitcast<u32>(y2) >> 30u;
    let i3 = bitcast<u32>(y3) >> 29u;
    var shift = (i2 & 2u) | (i1 & ~2u);
    shift = (i3 & 4u) | (shift & ~4u);
    return (0x2E74u >> shift) & 0x0101u;
}

fn solve_horiz_poly(p12: vec4<f32>, p3: vec2<f32>) -> vec2<f32> {
    let a = p12.xy - p12.zw * 2.0 + p3;
    let b = p12.xy - p12.zw;
    let ra = 1.0 / a.y;
    let rb = 0.5 / b.y;
    let d = sqrt(max(b.y * b.y - a.y * p12.y, 0.0));
    var t1 = (b.y - d) * ra;
    var t2 = (b.y + d) * ra;
    if abs(a.y) < 1.0 / 65536.0 {
        t1 = p12.y * rb;
        t2 = t1;
    }
    return vec2<f32>(
        (a.x * t1 - b.x * 2.0) * t1 + p12.x,
        (a.x * t2 - b.x * 2.0) * t2 + p12.x,
    );
}

fn solve_vert_poly(p12: vec4<f32>, p3: vec2<f32>) -> vec2<f32> {
    let a = p12.xy - p12.zw * 2.0 + p3;
    let b = p12.xy - p12.zw;
    let ra = 1.0 / a.x;
    let rb = 0.5 / b.x;
    let d = sqrt(max(b.x * b.x - a.x * p12.x, 0.0));
    var t1 = (b.x - d) * ra;
    var t2 = (b.x + d) * ra;
    if abs(a.x) < 1.0 / 65536.0 {
        t1 = p12.x * rb;
        t2 = t1;
    }
    return vec2<f32>(
        (a.y * t1 - b.y * 2.0) * t1 + p12.y,
        (a.y * t2 - b.y * 2.0) * t2 + p12.y,
    );
}

fn calc_band_loc(glyph_loc: vec2<i32>, offset: u32) -> vec2<i32> {
    var band_loc = vec2<i32>(glyph_loc.x + i32(offset), glyph_loc.y);
    band_loc.y += band_loc.x >> 12u;
    band_loc.x &= 4095;
    return band_loc;
}

fn calc_coverage(xcov: f32, ycov: f32, xwgt: f32, ywgt: f32) -> f32 {
    var coverage = max(
        abs(xcov * xwgt + ycov * ywgt) / max(xwgt + ywgt, 1.0 / 65536.0),
        min(abs(xcov), abs(ycov)),
    );
    coverage = clamp(coverage, 0.0, 1.0);
    return coverage;
}

@fragment
fn fs_glyph(in: GlyphOutput) -> @location(0) vec4<f32> {
    let render_coord = in.render_coord;
    let band_transform = in.banding;
    let glyph_loc = in.glyph.xy;
    let band_max = in.glyph.zw;
    let ems_per_pixel = max(fwidth(render_coord), vec2<f32>(1.0 / 65536.0));
    let pixels_per_em = 1.0 / ems_per_pixel;

    let band_index = clamp(
        vec2<i32>(render_coord * band_transform.xy + band_transform.zw),
        vec2<i32>(0, 0),
        band_max,
    );

    var xcov = 0.0;
    var xwgt = 0.0;
    let hband_data = textureLoad(band_texture, vec2<i32>(glyph_loc.x + band_index.y, glyph_loc.y), 0).xy;
    let hband_loc = calc_band_loc(glyph_loc, hband_data.y);
    for (var curve_index: i32 = 0; curve_index < i32(hband_data.x); curve_index++) {
        let curve_loc = vec2<i32>(textureLoad(band_texture, vec2<i32>(hband_loc.x + curve_index, hband_loc.y), 0).xy);
        let p12 = textureLoad(curve_texture, curve_loc, 0) - vec4<f32>(render_coord, render_coord);
        let p3 = textureLoad(curve_texture, vec2<i32>(curve_loc.x + 1, curve_loc.y), 0).xy - render_coord;
        let code = calc_root_code(p12.y, p12.w, p3.y);
        if code != 0u {
            let roots = solve_horiz_poly(p12, p3) * pixels_per_em.x;
            if (code & 1u) != 0u {
                xcov += clamp(roots.x + 0.5, 0.0, 1.0);
                xwgt = max(xwgt, clamp(1.0 - abs(roots.x) * 2.0, 0.0, 1.0));
            }
            if code > 1u {
                xcov -= clamp(roots.y + 0.5, 0.0, 1.0);
                xwgt = max(xwgt, clamp(1.0 - abs(roots.y) * 2.0, 0.0, 1.0));
            }
        }
    }

    var ycov = 0.0;
    var ywgt = 0.0;
    let vband_data = textureLoad(
        band_texture,
        vec2<i32>(glyph_loc.x + band_max.y + 1 + band_index.x, glyph_loc.y),
        0,
    ).xy;
    let vband_loc = calc_band_loc(glyph_loc, vband_data.y);
    for (var curve_index: i32 = 0; curve_index < i32(vband_data.x); curve_index++) {
        let curve_loc = vec2<i32>(textureLoad(band_texture, vec2<i32>(vband_loc.x + curve_index, vband_loc.y), 0).xy);
        let p12 = textureLoad(curve_texture, curve_loc, 0) - vec4<f32>(render_coord, render_coord);
        let p3 = textureLoad(curve_texture, vec2<i32>(curve_loc.x + 1, curve_loc.y), 0).xy - render_coord;
        let code = calc_root_code(p12.x, p12.z, p3.x);
        if code != 0u {
            let roots = solve_vert_poly(p12, p3) * pixels_per_em.y;
            if (code & 1u) != 0u {
                ycov -= clamp(roots.x + 0.5, 0.0, 1.0);
                ywgt = max(ywgt, clamp(1.0 - abs(roots.x) * 2.0, 0.0, 1.0));
            }
            if code > 1u {
                ycov += clamp(roots.y + 0.5, 0.0, 1.0);
                ywgt = max(ywgt, clamp(1.0 - abs(roots.y) * 2.0, 0.0, 1.0));
            }
        }
    }

    let coverage = calc_coverage(xcov, ycov, xwgt, ywgt);
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

#[cfg(feature = "xr")]
const XR_RECT_SHADER: &str = r#"
struct XrPageUniform {
    mvp: mat4x4<f32>,
    page_panel: vec4<f32>,
    misc: vec4<f32>,
};

fn linearize(c: vec3<f32>, srgb: f32) -> vec3<f32> {
    return mix(c, pow(c, vec3<f32>(2.2)), srgb);
}

@group(0) @binding(0) var<uniform> page: XrPageUniform;

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
    let local_pos = vec4<f32>(
        in.pos.x * in.rect.z - in.rect.z * 0.5,
        in.pos.y * in.rect.w - in.rect.w * 0.5,
        0.0,
        1.0,
    );
    let matrix = mat4x4<f32>(in.transform_0, in.transform_1, in.transform_2, in.transform_3);
    let world_pos_4 = matrix * local_pos;

    let is_fixed = (in.flags & 1u) != 0u;
    let scroll = select(page.misc.x, 0.0, is_fixed);

    let cx = in.rect.x + in.rect.z * 0.5;
    let cy = in.rect.y + in.rect.w * 0.5 - scroll;
    let page_pos = vec3<f32>(world_pos_4.x + cx, world_pos_4.y + cy, world_pos_4.z);

    let panel_x = (page_pos.x / page.page_panel.x - 0.5) * page.page_panel.z;
    let panel_y = (0.5 - page_pos.y / page.page_panel.y) * page.page_panel.w;
    let panel_z = page_pos.z * page.misc.z;
    let clip_pos = page.mvp * vec4<f32>(panel_x, panel_y, panel_z, 1.0);
    out.pos = vec4<f32>(
        clip_pos.xy,
        clip_pos.z - in.draw_order * page.misc.w * clip_pos.w,
        clip_pos.w,
    );
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

    let aa = max(fwidth(dist), 0.5);
    let alpha = 1.0 - smoothstep(-aa, aa, dist);

    if border > 0.0 {
        let inner_dist = rounded_rect_sdf(centered, half_size - vec2(border), max(r - border, 0.0));
        let inner_aa = max(fwidth(inner_dist), 0.5);
        let inner_alpha = smoothstep(-inner_aa, inner_aa, inner_dist);
        let fill = in.color * (1.0 - inner_alpha);
        let border_fill = in.border_color * inner_alpha;
        return vec4<f32>(
            linearize(fill.rgb + border_fill.rgb, page.misc.y),
            alpha * max(fill.a, border_fill.a),
        );
    }

    return vec4<f32>(linearize(in.color.rgb, page.misc.y), in.color.a * alpha);
}
"#;

#[cfg(feature = "xr")]
const XR_GLYPH_SHADER: &str = r#"
struct XrPageUniform {
    mvp: mat4x4<f32>,
    page_panel: vec4<f32>,
    misc: vec4<f32>,
};

fn linearize(c: vec3<f32>, srgb: f32) -> vec3<f32> {
    return mix(c, pow(c, vec3<f32>(2.2)), srgb);
}

@group(0) @binding(0) var<uniform> page: XrPageUniform;
@group(0) @binding(1) var curve_texture: texture_2d<f32>;
@group(0) @binding(2) var band_texture: texture_2d<u32>;

struct GlyphInput {
    @location(0) pos: vec4<f32>,
    @location(1) tex: vec4<f32>,
    @location(2) bnd: vec4<f32>,
    @location(3) color: vec4<f32>,
    @location(4) transform_0: vec4<f32>,
    @location(5) transform_1: vec4<f32>,
    @location(6) transform_2: vec4<f32>,
    @location(7) transform_3: vec4<f32>,
    @location(8) center: vec2<f32>,
    @location(9) flags: u32,
    @location(10) draw_order: f32,
};

struct GlyphOutput {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) render_coord: vec2<f32>,
    @location(2) @interpolate(flat) banding: vec4<f32>,
    @location(3) @interpolate(flat) glyph: vec4<i32>,
};

@vertex
fn vs_glyph(in: GlyphInput) -> GlyphOutput {
    var out: GlyphOutput;
    let local_pos = vec4<f32>(in.pos.x - in.center.x, in.pos.y - in.center.y, 0.0, 1.0);
    let matrix = mat4x4<f32>(in.transform_0, in.transform_1, in.transform_2, in.transform_3);
    let world_pos_4 = matrix * local_pos;

    let is_fixed = (in.flags & 1u) != 0u;
    let scroll = select(page.misc.x, 0.0, is_fixed);
    let cx = in.center.x;
    let cy = in.center.y - scroll;
    let page_pos = vec3<f32>(world_pos_4.x + cx, world_pos_4.y + cy, world_pos_4.z);

    let panel_x = (page_pos.x / page.page_panel.x - 0.5) * page.page_panel.z;
    let panel_y = (0.5 - page_pos.y / page.page_panel.y) * page.page_panel.w;
    let panel_z = page_pos.z * page.misc.z;
    let clip_pos = page.mvp * vec4<f32>(panel_x, panel_y, panel_z, 1.0);
    out.pos = vec4<f32>(
        clip_pos.xy,
        clip_pos.z - in.draw_order * page.misc.w * clip_pos.w,
        clip_pos.w,
    );
    out.color = in.color;
    out.render_coord = in.tex.xy;
    out.banding = in.bnd;

    let glyph_loc = bitcast<u32>(in.tex.z);
    let band_max = bitcast<u32>(in.tex.w);
    out.glyph = vec4<i32>(
        i32(glyph_loc & 0xFFFFu),
        i32(glyph_loc >> 16u),
        i32(band_max & 0xFFu),
        i32((band_max >> 16u) & 0xFFu),
    );
    return out;
}

fn calc_root_code(y1: f32, y2: f32, y3: f32) -> u32 {
    let i1 = bitcast<u32>(y1) >> 31u;
    let i2 = bitcast<u32>(y2) >> 30u;
    let i3 = bitcast<u32>(y3) >> 29u;
    var shift = (i2 & 2u) | (i1 & ~2u);
    shift = (i3 & 4u) | (shift & ~4u);
    return (0x2E74u >> shift) & 0x0101u;
}

fn solve_horiz_poly(p12: vec4<f32>, p3: vec2<f32>) -> vec2<f32> {
    let a = p12.xy - p12.zw * 2.0 + p3;
    let b = p12.xy - p12.zw;
    let ra = 1.0 / a.y;
    let rb = 0.5 / b.y;
    let d = sqrt(max(b.y * b.y - a.y * p12.y, 0.0));
    var t1 = (b.y - d) * ra;
    var t2 = (b.y + d) * ra;
    if abs(a.y) < 1.0 / 65536.0 {
        t1 = p12.y * rb;
        t2 = t1;
    }
    return vec2<f32>(
        (a.x * t1 - b.x * 2.0) * t1 + p12.x,
        (a.x * t2 - b.x * 2.0) * t2 + p12.x,
    );
}

fn solve_vert_poly(p12: vec4<f32>, p3: vec2<f32>) -> vec2<f32> {
    let a = p12.xy - p12.zw * 2.0 + p3;
    let b = p12.xy - p12.zw;
    let ra = 1.0 / a.x;
    let rb = 0.5 / b.x;
    let d = sqrt(max(b.x * b.x - a.x * p12.x, 0.0));
    var t1 = (b.x - d) * ra;
    var t2 = (b.x + d) * ra;
    if abs(a.x) < 1.0 / 65536.0 {
        t1 = p12.x * rb;
        t2 = t1;
    }
    return vec2<f32>(
        (a.y * t1 - b.y * 2.0) * t1 + p12.y,
        (a.y * t2 - b.y * 2.0) * t2 + p12.y,
    );
}

fn calc_band_loc(glyph_loc: vec2<i32>, offset: u32) -> vec2<i32> {
    var band_loc = vec2<i32>(glyph_loc.x + i32(offset), glyph_loc.y);
    band_loc.y += band_loc.x >> 12u;
    band_loc.x &= 4095;
    return band_loc;
}

fn calc_coverage(xcov: f32, ycov: f32, xwgt: f32, ywgt: f32) -> f32 {
    var coverage = max(
        abs(xcov * xwgt + ycov * ywgt) / max(xwgt + ywgt, 1.0 / 65536.0),
        min(abs(xcov), abs(ycov)),
    );
    coverage = clamp(coverage, 0.0, 1.0);
    return coverage;
}

@fragment
fn fs_glyph(in: GlyphOutput) -> @location(0) vec4<f32> {
    let render_coord = in.render_coord;
    let band_transform = in.banding;
    let glyph_loc = in.glyph.xy;
    let band_max = in.glyph.zw;
    let ems_per_pixel = max(fwidth(render_coord), vec2<f32>(1.0 / 65536.0));
    let pixels_per_em = 1.0 / ems_per_pixel;

    let band_index = clamp(
        vec2<i32>(render_coord * band_transform.xy + band_transform.zw),
        vec2<i32>(0, 0),
        band_max,
    );

    var xcov = 0.0;
    var xwgt = 0.0;
    let hband_data = textureLoad(
        band_texture,
        vec2<i32>(glyph_loc.x + band_index.y, glyph_loc.y),
        0,
    ).xy;
    let hband_loc = calc_band_loc(glyph_loc, hband_data.y);
    for (var curve_index: i32 = 0; curve_index < i32(hband_data.x); curve_index++) {
        let curve_loc = vec2<i32>(
            textureLoad(band_texture, vec2<i32>(hband_loc.x + curve_index, hband_loc.y), 0).xy,
        );
        let p12 = textureLoad(curve_texture, curve_loc, 0) - vec4<f32>(render_coord, render_coord);
        let p3 = textureLoad(curve_texture, vec2<i32>(curve_loc.x + 1, curve_loc.y), 0).xy
            - render_coord;
        let code = calc_root_code(p12.y, p12.w, p3.y);
        if code != 0u {
            let roots = solve_horiz_poly(p12, p3) * pixels_per_em.x;
            if (code & 1u) != 0u {
                xcov += clamp(roots.x + 0.5, 0.0, 1.0);
                xwgt = max(xwgt, clamp(1.0 - abs(roots.x) * 2.0, 0.0, 1.0));
            }
            if code > 1u {
                xcov -= clamp(roots.y + 0.5, 0.0, 1.0);
                xwgt = max(xwgt, clamp(1.0 - abs(roots.y) * 2.0, 0.0, 1.0));
            }
        }
    }

    var ycov = 0.0;
    var ywgt = 0.0;
    let vband_data = textureLoad(
        band_texture,
        vec2<i32>(glyph_loc.x + band_max.y + 1 + band_index.x, glyph_loc.y),
        0,
    ).xy;
    let vband_loc = calc_band_loc(glyph_loc, vband_data.y);
    for (var curve_index: i32 = 0; curve_index < i32(vband_data.x); curve_index++) {
        let curve_loc = vec2<i32>(
            textureLoad(band_texture, vec2<i32>(vband_loc.x + curve_index, vband_loc.y), 0).xy,
        );
        let p12 = textureLoad(curve_texture, curve_loc, 0) - vec4<f32>(render_coord, render_coord);
        let p3 = textureLoad(curve_texture, vec2<i32>(curve_loc.x + 1, curve_loc.y), 0).xy
            - render_coord;
        let code = calc_root_code(p12.x, p12.z, p3.x);
        if code != 0u {
            let roots = solve_vert_poly(p12, p3) * pixels_per_em.y;
            if (code & 1u) != 0u {
                ycov -= clamp(roots.x + 0.5, 0.0, 1.0);
                ywgt = max(ywgt, clamp(1.0 - abs(roots.x) * 2.0, 0.0, 1.0));
            }
            if code > 1u {
                ycov += clamp(roots.y + 0.5, 0.0, 1.0);
                ywgt = max(ywgt, clamp(1.0 - abs(roots.y) * 2.0, 0.0, 1.0));
            }
        }
    }

    let coverage = calc_coverage(xcov, ycov, xwgt, ywgt);
    return vec4<f32>(linearize(in.color.rgb, page.misc.y), in.color.a * coverage);
}
"#;

#[cfg(feature = "xr")]
const XR_LINE_SHADER: &str = r#"
struct XrPageUniform {
    mvp: mat4x4<f32>,
    page_panel: vec4<f32>,
    misc: vec4<f32>,
};

fn linearize(c: vec3<f32>, srgb: f32) -> vec3<f32> {
    return mix(c, pow(c, vec3<f32>(2.2)), srgb);
}

@group(0) @binding(0) var<uniform> page: XrPageUniform;

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
    @location(1) edge_dist: f32,
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
    let page_pos = base + offset - vec2<f32>(0.0, page.misc.x);

    let panel_x = (page_pos.x / page.page_panel.x - 0.5) * page.page_panel.z;
    let panel_y = (0.5 - page_pos.y / page.page_panel.y) * page.page_panel.w;
    let clip_pos = page.mvp * vec4<f32>(panel_x, panel_y, 0.0, 1.0);
    out.pos = vec4<f32>(
        clip_pos.xy,
        clip_pos.z - in.draw_order * page.misc.w * clip_pos.w,
        clip_pos.w,
    );
    out.color = in.color;
    out.edge_dist = 1.0 - abs(in.pos.y * 2.0 - 1.0);
    return out;
}

@fragment
fn fs_line(in: LineOutput) -> @location(0) vec4<f32> {
    let aa = max(fwidth(in.edge_dist), 0.0001);
    let alpha = smoothstep(0.0, aa, in.edge_dist);
    return vec4<f32>(linearize(in.color.rgb, page.misc.y), in.color.a * alpha);
}
"#;

#[cfg(feature = "xr")]
const XR_TEXRECT_SHADER: &str = r#"
struct XrPageUniform {
    mvp: mat4x4<f32>,
    page_panel: vec4<f32>,
    misc: vec4<f32>,
};

fn linearize(c: vec3<f32>, srgb: f32) -> vec3<f32> {
    return mix(c, pow(c, vec3<f32>(2.2)), srgb);
}

@group(0) @binding(0) var<uniform> page: XrPageUniform;
@group(1) @binding(0) var layer_tex: texture_2d<f32>;
@group(1) @binding(1) var layer_sampler: sampler;

struct TexRectInput {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) rect: vec4<f32>,
    @location(3) transform_0: vec4<f32>,
    @location(4) transform_1: vec4<f32>,
    @location(5) transform_2: vec4<f32>,
    @location(6) transform_3: vec4<f32>,
    @location(7) flags: u32,
    @location(8) draw_order: f32,
};

struct TexRectOutput {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_texrect(in: TexRectInput) -> TexRectOutput {
    var out: TexRectOutput;
    let local_pos = vec4<f32>(
        in.pos.x * in.rect.z - in.rect.z * 0.5,
        in.pos.y * in.rect.w - in.rect.w * 0.5,
        0.0,
        1.0,
    );
    let matrix = mat4x4<f32>(in.transform_0, in.transform_1, in.transform_2, in.transform_3);
    let world_pos_4 = matrix * local_pos;

    let is_fixed = (in.flags & 1u) != 0u;
    let scroll = select(page.misc.x, 0.0, is_fixed);
    let cx = in.rect.x + in.rect.z * 0.5;
    let cy = in.rect.y + in.rect.w * 0.5 - scroll;
    let page_pos = vec3<f32>(world_pos_4.x + cx, world_pos_4.y + cy, world_pos_4.z);

    let panel_x = (page_pos.x / page.page_panel.x - 0.5) * page.page_panel.z;
    let panel_y = (0.5 - page_pos.y / page.page_panel.y) * page.page_panel.w;
    let panel_z = page_pos.z * page.misc.z;
    let clip_pos = page.mvp * vec4<f32>(panel_x, panel_y, panel_z, 1.0);
    out.pos = vec4<f32>(
        clip_pos.xy,
        clip_pos.z - in.draw_order * page.misc.w * clip_pos.w,
        clip_pos.w,
    );
    out.uv = in.uv;
    return out;
}

@fragment
fn fs_texrect(in: TexRectOutput) -> @location(0) vec4<f32> {
    let sample = textureSample(layer_tex, layer_sampler, in.uv);
    return vec4<f32>(linearize(sample.rgb, page.misc.y), sample.a);
}
"#;

#[cfg(feature = "xr")]
const XR_PANEL_SHADER: &str = r#"
struct PanelUniform {
    mvp: mat4x4<f32>,
};

struct VsInput {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

struct VsOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@group(0) @binding(0) var<uniform> panel: PanelUniform;
@group(0) @binding(1) var panel_tex: texture_2d<f32>;
@group(0) @binding(2) var panel_sampler: sampler;

@vertex
fn vs_panel(in: VsInput) -> VsOutput {
    var out: VsOutput;
    out.position = panel.mvp * vec4<f32>(in.position, 1.0);
    out.uv = in.uv;
    return out;
}

@fragment
fn fs_panel(in: VsOutput) -> @location(0) vec4<f32> {
    return textureSample(panel_tex, panel_sampler, in.uv);
}
"#;

#[cfg(feature = "xr")]
const XR_MIPMAP_SHADER: &str = r#"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;
    let x = i32(vertex_index) / 2;
    let y = i32(vertex_index) & 1;
    let tc = vec2<f32>(f32(x) * 2.0, f32(y) * 2.0);
    out.position = vec4<f32>(tc.x * 2.0 - 1.0, 1.0 - tc.y * 2.0, 0.0, 1.0);
    out.tex_coords = tc;
    return out;
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(src_tex, src_sampler, in.tex_coords);
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
    present_mode: wgpu::PresentMode,
    render_format: wgpu::TextureFormat,
    pub size: winit::dpi::PhysicalSize<u32>,
    pub scale_factor: f64,
    render_scale_factor: f32,

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
    #[cfg(feature = "xr")]
    xr_page_bgl: wgpu::BindGroupLayout,
    #[cfg(feature = "xr")]
    xr_rect_pipeline: wgpu::RenderPipeline,
    #[cfg(feature = "xr")]
    xr_glyph_bgl: wgpu::BindGroupLayout,
    #[cfg(feature = "xr")]
    xr_glyph_pipeline: wgpu::RenderPipeline,
    #[cfg(feature = "xr")]
    xr_line_pipeline: wgpu::RenderPipeline,
    #[cfg(feature = "xr")]
    xr_texrect_pipeline: wgpu::RenderPipeline,
    #[cfg(feature = "xr")]
    xr_panel_pipeline: wgpu::RenderPipeline,
    #[cfg(feature = "xr")]
    xr_panel_bgl: wgpu::BindGroupLayout,
    #[cfg(feature = "xr")]
    xr_panel_vertex_buffer: wgpu::Buffer,
    #[cfg(feature = "xr")]
    xr_mipmap_pipeline: wgpu::RenderPipeline,
    #[cfg(feature = "xr")]
    xr_mipmap_bgl: wgpu::BindGroupLayout,
    #[cfg(feature = "xr")]
    xr_mipmap_sampler: wgpu::Sampler,

    atlas: GlyphAtlas,
    slug_font_family: String,
    slug_atlas: SlugFontAtlas,
    slug_curve_texture: wgpu::Texture,
    slug_band_texture: wgpu::Texture,
    pub font_system: FontSystem,
    swash_cache: SwashCache,
    pub static_dirty: bool,
    cached_static_groups: Vec<InternalDrawGroup>,
    depth_view: wgpu::TextureView,
}

pub struct OffscreenRenderTarget {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub sample_view: wgpu::TextureView,
    depth_texture: wgpu::Texture,
    pub depth_view: wgpu::TextureView,
    pub size: winit::dpi::PhysicalSize<u32>,
    pub format: wgpu::TextureFormat,
    pub mip_level_count: u32,
}

fn create_font_system() -> FontSystem {
    #[cfg(target_os = "android")]
    {
        let mut db = cosmic_text::fontdb::Database::new();
        for name in &[
            "Roboto-Regular.ttf",
            "NotoSans-Regular.ttf",
            "DroidSans.ttf",
            "DroidSans-Bold.ttf",
            "DroidSansMono.ttf",
        ] {
            let path = format!("/system/fonts/{name}");
            if std::path::Path::new(&path).exists() {
                db.load_font_file(path).ok();
            }
        }
        return FontSystem::new_with_locale_and_db("en-US".to_string(), db);
    }

    #[cfg(target_os = "ios")]
    {
        let mut db = cosmic_text::fontdb::Database::new();
        db.load_fonts_dir("/System/Library/Fonts");
        db.load_fonts_dir("/System/Library/Fonts/Core");
        return FontSystem::new_with_locale_and_db("en-US".to_string(), db);
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        FontSystem::new()
    }
}

fn load_slug_font(font_system: &FontSystem) -> Result<(String, SlugFontAtlas)> {
    #[cfg(target_os = "android")]
    let families = [
        DbFamily::Name("Roboto"),
        DbFamily::Name("Noto Sans"),
        DbFamily::Name("Droid Sans"),
    ];
    #[cfg(not(target_os = "android"))]
    let families = [
        DbFamily::SansSerif,
        DbFamily::Name("DejaVu Sans"),
        DbFamily::Name("Arial"),
        DbFamily::Name("Liberation Sans"),
        DbFamily::Name("Helvetica"),
    ];

    let query = DbQuery {
        families: &families,
        weight: DbWeight::NORMAL,
        stretch: DbStretch::Normal,
        style: DbStyle::Normal,
    };

    let face_id = font_system
        .db()
        .query(&query)
        .or_else(|| font_system.db().faces().next().map(|face| face.id))
        .ok_or_else(|| anyhow!("no font face available for slug text rendering"))?;

    let family_name = font_system
        .db()
        .face(face_id)
        .and_then(|face| face.families.first().map(|family| family.0.clone()))
        .ok_or_else(|| anyhow!("matched font face has no family name"))?;

    let atlas = font_system
        .db()
        .with_face_data(face_id, |data, face_index| {
            build_font_atlas(data, face_index)
        })
        .ok_or_else(|| anyhow!("failed to access matched font bytes"))?
        .map_err(|err| anyhow!(err))?;

    Ok((family_name, atlas))
}

#[allow(clippy::too_many_arguments)]
fn create_slug_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &'static str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    bytes: &[u8],
    bytes_per_row: u32,
) -> wgpu::Texture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(bytes_per_row),
            rows_per_image: None,
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    texture
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
        let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else {
            wgpu::PresentMode::AutoVsync
        };

        surface.configure(
            &device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: surface_format,
                width: size.width.max(1),
                height: size.height.max(1),
                present_mode,
                alpha_mode: wgpu::CompositeAlphaMode::Auto,
                view_formats: view_formats.clone(),
                desired_maximum_frame_latency: 1,
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
        let font_system = create_font_system();
        let swash_cache = SwashCache::new();
        let (slug_font_family, slug_atlas) = load_slug_font(&font_system)?;
        let slug_curve_texture = create_slug_texture(
            &device,
            &queue,
            "slug curves",
            4096,
            slug_atlas.curve_height.max(1),
            wgpu::TextureFormat::Rgba32Float,
            bytemuck::cast_slice(&slug_atlas.curve_texels),
            4096 * 16,
        );
        let slug_band_texture = create_slug_texture(
            &device,
            &queue,
            "slug bands",
            4096,
            slug_atlas.band_height.max(1),
            wgpu::TextureFormat::Rg32Uint,
            bytemuck::cast_slice(&slug_atlas.band_texels),
            4096 * 8,
        );
        let slug_curve_view = slug_curve_texture.create_view(&Default::default());
        let slug_band_view = slug_band_texture.create_view(&Default::default());

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
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
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
                    resource: wgpu::BindingResource::TextureView(&slug_curve_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&slug_band_view),
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
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<SlugVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x4,
                        1 => Float32x4,
                        2 => Float32x4,
                        3 => Float32x4,
                        4 => Float32x4,
                        5 => Float32x4,
                        6 => Float32x4,
                        7 => Float32x4,
                        8 => Float32x2,
                        9 => Uint32,
                        10 => Float32,
                    ],
                }],
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
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });
        let texrect_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("texrect pl"),
            bind_group_layouts: &[Some(&rect_bgl), Some(&texrect_bgl)],
            immediate_size: 0,
        });

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

        #[cfg(feature = "xr")]
        let xr_page_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("xr page bgl"),
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
        #[cfg(feature = "xr")]
        let xr_rect_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("xr rect shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(XR_RECT_SHADER)),
        });
        #[cfg(feature = "xr")]
        let xr_rect_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("xr rect pl"),
                bind_group_layouts: &[Some(&xr_page_bgl)],
                immediate_size: 0,
            });
        #[cfg(feature = "xr")]
        let xr_rect_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("xr rect pipeline"),
            layout: Some(&xr_rect_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &xr_rect_shader,
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
                module: &xr_rect_shader,
                entry_point: Some("fs_rect"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
            multisample: wgpu::MultisampleState {
                count: crate::XR_NATIVE_MSAA_SAMPLES,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });
        #[cfg(feature = "xr")]
        let xr_glyph_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("xr glyph bgl"),
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
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        #[cfg(feature = "xr")]
        let xr_glyph_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("xr glyph shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(XR_GLYPH_SHADER)),
        });
        #[cfg(feature = "xr")]
        let xr_glyph_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("xr glyph pl"),
                bind_group_layouts: &[Some(&xr_glyph_bgl)],
                immediate_size: 0,
            });
        #[cfg(feature = "xr")]
        let xr_glyph_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("xr glyph pipeline"),
            layout: Some(&xr_glyph_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &xr_glyph_shader,
                entry_point: Some("vs_glyph"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<SlugVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x4,
                        1 => Float32x4,
                        2 => Float32x4,
                        3 => Float32x4,
                        4 => Float32x4,
                        5 => Float32x4,
                        6 => Float32x4,
                        7 => Float32x4,
                        8 => Float32x2,
                        9 => Uint32,
                        10 => Float32,
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &xr_glyph_shader,
                entry_point: Some("fs_glyph"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
            multisample: wgpu::MultisampleState {
                count: crate::XR_NATIVE_MSAA_SAMPLES,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });
        #[cfg(feature = "xr")]
        let xr_line_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("xr line shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(XR_LINE_SHADER)),
        });
        #[cfg(feature = "xr")]
        let xr_line_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("xr line pl"),
                bind_group_layouts: &[Some(&xr_page_bgl)],
                immediate_size: 0,
            });
        #[cfg(feature = "xr")]
        let xr_line_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("xr line pipeline"),
            layout: Some(&xr_line_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &xr_line_shader,
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
                module: &xr_line_shader,
                entry_point: Some("fs_line"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
            multisample: wgpu::MultisampleState {
                count: crate::XR_NATIVE_MSAA_SAMPLES,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });
        #[cfg(feature = "xr")]
        let xr_texrect_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("xr texrect shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(XR_TEXRECT_SHADER)),
        });
        #[cfg(feature = "xr")]
        let xr_texrect_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("xr texrect pl"),
            bind_group_layouts: &[Some(&xr_page_bgl), Some(&texrect_bgl)],
            immediate_size: 0,
        });
        #[cfg(feature = "xr")]
        let xr_texrect_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("xr texrect pipeline"),
            layout: Some(&xr_texrect_pl),
            vertex: wgpu::VertexState {
                module: &xr_texrect_shader,
                entry_point: Some("vs_texrect"),
                compilation_options: Default::default(),
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<XrTexRectInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![
                            2 => Float32x4,
                            3 => Float32x4,
                            4 => Float32x4,
                            5 => Float32x4,
                            6 => Float32x4,
                            7 => Uint32,
                            8 => Float32,
                        ],
                    },
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &xr_texrect_shader,
                entry_point: Some("fs_texrect"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
            multisample: wgpu::MultisampleState {
                count: crate::XR_NATIVE_MSAA_SAMPLES,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });

        #[cfg(feature = "xr")]
        let xr_panel_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("xr panel bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
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
        #[cfg(feature = "xr")]
        let xr_panel_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("xr panel shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(XR_PANEL_SHADER)),
        });
        #[cfg(feature = "xr")]
        let xr_panel_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("xr panel pl"),
            bind_group_layouts: &[Some(&xr_panel_bgl)],
            immediate_size: 0,
        });
        #[cfg(feature = "xr")]
        let xr_panel_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("xr panel pipeline"),
            layout: Some(&xr_panel_pl),
            vertex: wgpu::VertexState {
                module: &xr_panel_shader,
                entry_point: Some("vs_panel"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<PanelVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &xr_panel_shader,
                entry_point: Some("fs_panel"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        #[cfg(feature = "xr")]
        let xr_mipmap_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("xr mipmap shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(XR_MIPMAP_SHADER)),
        });
        #[cfg(feature = "xr")]
        let xr_mipmap_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("xr mipmap pipeline"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &xr_mipmap_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &xr_mipmap_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: render_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        #[cfg(feature = "xr")]
        let xr_mipmap_bgl = xr_mipmap_pipeline.get_bind_group_layout(0);
        #[cfg(feature = "xr")]
        let xr_mipmap_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("xr mipmap sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        #[cfg(feature = "xr")]
        let xr_panel_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("xr panel verts"),
            contents: bytemuck::cast_slice(&[
                PanelVertex {
                    position: [-0.5, -0.5, 0.0],
                    uv: [0.0, 1.0],
                },
                PanelVertex {
                    position: [0.5, -0.5, 0.0],
                    uv: [1.0, 1.0],
                },
                PanelVertex {
                    position: [-0.5, 0.5, 0.0],
                    uv: [0.0, 0.0],
                },
                PanelVertex {
                    position: [0.5, -0.5, 0.0],
                    uv: [1.0, 1.0],
                },
                PanelVertex {
                    position: [0.5, 0.5, 0.0],
                    uv: [1.0, 0.0],
                },
                PanelVertex {
                    position: [-0.5, 0.5, 0.0],
                    uv: [0.0, 0.0],
                },
            ]),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let scale_factor = window.scale_factor();

        Ok(Self {
            window,
            instance,
            adapter,
            device,
            queue,
            surface,
            surface_format,
            present_mode,
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
            #[cfg(feature = "xr")]
            xr_page_bgl,
            #[cfg(feature = "xr")]
            xr_rect_pipeline,
            #[cfg(feature = "xr")]
            xr_glyph_bgl,
            #[cfg(feature = "xr")]
            xr_glyph_pipeline,
            #[cfg(feature = "xr")]
            xr_line_pipeline,
            #[cfg(feature = "xr")]
            xr_texrect_pipeline,
            #[cfg(feature = "xr")]
            xr_panel_pipeline,
            #[cfg(feature = "xr")]
            xr_panel_bgl,
            #[cfg(feature = "xr")]
            xr_panel_vertex_buffer,
            #[cfg(feature = "xr")]
            xr_mipmap_pipeline,
            #[cfg(feature = "xr")]
            xr_mipmap_bgl,
            #[cfg(feature = "xr")]
            xr_mipmap_sampler,
            atlas,
            slug_font_family,
            slug_atlas,
            slug_curve_texture,
            slug_band_texture,
            font_system,
            swash_cache,
            scale_factor,
            render_scale_factor: scale_factor as f32,
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
                    present_mode: self.present_mode,
                    alpha_mode: wgpu::CompositeAlphaMode::Auto,
                    view_formats,
                    desired_maximum_frame_latency: 1,
                },
            );
            self.depth_view = create_depth_view(&self.device, new_size.width, new_size.height);
        }
    }

    pub fn create_render_target(&self, width: u32, height: u32) -> OffscreenRenderTarget {
        let size = winit::dpi::PhysicalSize::new(width.max(1), height.max(1));
        let mip_level_count = size.width.max(size.height).ilog2() + 1;
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen render target"),
            size: wgpu::Extent3d {
                width: size.width,
                height: size.height,
                depth_or_array_layers: 1,
            },
            mip_level_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.render_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("offscreen render target mip0"),
            base_mip_level: 0,
            mip_level_count: Some(1),
            ..Default::default()
        });
        let sample_view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("offscreen render target sampled"),
            ..Default::default()
        });
        let depth_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen depth target"),
            size: wgpu::Extent3d {
                width: size.width,
                height: size.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth_texture.create_view(&Default::default());
        OffscreenRenderTarget {
            texture,
            view,
            sample_view,
            depth_texture,
            depth_view,
            size,
            format: self.render_format,
            mip_level_count,
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
        let depth_view = self.depth_view.clone();
        let target_size = self.size;
        let srgb_target =
            self.surface_format.is_srgb() && self.render_format == self.surface_format;
        self.render_to_view(
            &view,
            &depth_view,
            target_size,
            self.scale_factor as f32,
            srgb_target,
            static_commands,
            ghost_commands,
            clear_color,
            scroll_y,
            webgpu_canvases,
        );
        frame.present();
    }

    pub fn render_to_target(
        &mut self,
        target: &OffscreenRenderTarget,
        target_scale_factor: f32,
        static_commands: &[DrawCommand],
        ghost_commands: &[DrawCommand],
        clear_color: [f32; 4],
        scroll_y: f32,
        webgpu_canvases: &[([f32; 4], &wgpu::TextureView)],
    ) {
        self.render_to_view(
            &target.view,
            &target.depth_view,
            target.size,
            target_scale_factor,
            target.format.is_srgb(),
            static_commands,
            ghost_commands,
            clear_color,
            scroll_y,
            webgpu_canvases,
        );
        #[cfg(feature = "xr")]
        self.generate_mipmaps(&target.texture, target.format, target.mip_level_count);
    }

    pub fn render_to_view(
        &mut self,
        view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        target_size: winit::dpi::PhysicalSize<u32>,
        target_scale_factor: f32,
        srgb_target: bool,
        static_commands: &[DrawCommand],
        ghost_commands: &[DrawCommand],
        clear_color: [f32; 4],
        scroll_y: f32,
        webgpu_canvases: &[([f32; 4], &wgpu::TextureView)],
    ) {
        if (self.render_scale_factor - target_scale_factor).abs() > f32::EPSILON {
            self.render_scale_factor = target_scale_factor;
            self.static_dirty = true;
        }
        self.queue.write_buffer(
            &self.screen_buffer,
            0,
            bytemuck::cast_slice(&[
                target_size.width as f32 / self.render_scale_factor,
                target_size.height as f32 / self.render_scale_factor,
                if srgb_target { 1.0_f32 } else { 0.0_f32 },
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
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
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
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
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
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("texrect pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
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
    }

    #[cfg(feature = "xr")]
    fn generate_mipmaps(
        &self,
        texture: &wgpu::Texture,
        format: wgpu::TextureFormat,
        mip_level_count: u32,
    ) {
        if mip_level_count <= 1 {
            return;
        }

        let views = (0..mip_level_count)
            .map(|mip| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("xr panel mip"),
                    format: Some(format),
                    base_mip_level: mip,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            })
            .collect::<Vec<_>>();

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("xr mipmap encoder"),
            });

        for target_mip in 1..mip_level_count as usize {
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("xr mipmap bg"),
                layout: &self.xr_mipmap_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&views[target_mip - 1]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.xr_mipmap_sampler),
                    },
                ],
            });

            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xr mipmap pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &views[target_mip],
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rpass.set_pipeline(&self.xr_mipmap_pipeline);
            rpass.set_bind_group(0, Some(&bind_group), &[]);
            rpass.draw(0..3, 0..1);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
    }

    #[cfg(feature = "xr")]
    pub fn render_xr_panel_views(
        &self,
        left_target: &wgpu::TextureView,
        right_target: &wgpu::TextureView,
        panel_texture: &wgpu::TextureView,
        clear_color: [f32; 4],
        left_mvp: [f32; 16],
        right_mvp: [f32; 16],
    ) {
        let left_uniform = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("xr panel left uniform"),
                contents: bytemuck::bytes_of(&XrPanelUniform { mvp: left_mvp }),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let right_uniform = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("xr panel right uniform"),
                contents: bytemuck::bytes_of(&XrPanelUniform { mvp: right_mvp }),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let left_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("xr panel left bg"),
            layout: &self.xr_panel_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: left_uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(panel_texture),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.texrect_sampler),
                },
            ],
        });
        let right_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("xr panel right bg"),
            layout: &self.xr_panel_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: right_uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(panel_texture),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.texrect_sampler),
                },
            ],
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("xr panel encoder"),
            });
        for (target, bind_group) in [(left_target, &left_bg), (right_target, &right_bg)] {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xr panel pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
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
            rpass.set_pipeline(&self.xr_panel_pipeline);
            rpass.set_bind_group(0, Some(bind_group), &[]);
            rpass.set_vertex_buffer(0, self.xr_panel_vertex_buffer.slice(..));
            rpass.draw(0..6, 0..1);
        }
        self.queue.submit(std::iter::once(encoder.finish()));
    }

    #[cfg(feature = "xr")]
    pub fn render_xr_native_views(
        &mut self,
        left_target: &wgpu::TextureView,
        right_target: &wgpu::TextureView,
        msaa_color_view: Option<&wgpu::TextureView>,
        depth_view: &wgpu::TextureView,
        page_logical_size: [f32; 2],
        raster_scale_factor: f32,
        panel_size: [f32; 2],
        clear_color: [f32; 4],
        static_commands: &[DrawCommand],
        ghost_commands: &[DrawCommand],
        scroll_y: f32,
        left_mvp: [f32; 16],
        right_mvp: [f32; 16],
    ) {
        if (self.render_scale_factor - raster_scale_factor).abs() > f32::EPSILON {
            self.render_scale_factor = raster_scale_factor;
            self.static_dirty = true;
        }
        if self.static_dirty {
            self.cached_static_groups = self.build_draw_groups(static_commands);
            self.static_dirty = false;
        }

        let static_max_draw_order = self.max_draw_order(&self.cached_static_groups);
        let mut ghost_groups = self.build_draw_groups(ghost_commands);
        self.offset_draw_orders(&mut ghost_groups, static_max_draw_order + 1.0);

        let logical_width = page_logical_size[0];
        let logical_height = page_logical_size[1];
        let page_to_meter = panel_size[0] / logical_width.max(1.0);
        let max_draw_order = self
            .max_draw_order(&ghost_groups)
            .max(static_max_draw_order)
            .max(1.0);
        let draw_order_depth_step = 0.001 / max_draw_order;

        let make_uniform = |mvp| XrPageUniform {
            mvp,
            page_panel: [logical_width, logical_height, panel_size[0], panel_size[1]],
            misc: [scroll_y, 1.0, page_to_meter, draw_order_depth_step],
        };

        let left_uniform = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("xr page left uniform"),
                contents: bytemuck::bytes_of(&make_uniform(left_mvp)),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let right_uniform = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("xr page right uniform"),
                contents: bytemuck::bytes_of(&make_uniform(right_mvp)),
                usage: wgpu::BufferUsages::UNIFORM,
            });

        let left_page_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("xr page left bg"),
            layout: &self.xr_page_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: left_uniform.as_entire_binding(),
            }],
        });
        let right_page_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("xr page right bg"),
            layout: &self.xr_page_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: right_uniform.as_entire_binding(),
            }],
        });
        let curve_view = self.slug_curve_texture.create_view(&Default::default());
        let band_view = self.slug_band_texture.create_view(&Default::default());
        let left_glyph_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("xr glyph left bg"),
            layout: &self.xr_glyph_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: left_uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&curve_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&band_view),
                },
            ],
        });
        let right_glyph_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("xr glyph right bg"),
            layout: &self.xr_glyph_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: right_uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&curve_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&band_view),
                },
            ],
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("xr native page encoder"),
            });
        for (target, page_bg, glyph_bg) in [
            (left_target, &left_page_bg, &left_glyph_bg),
            (right_target, &right_page_bg, &right_glyph_bg),
        ] {
            let color_view = msaa_color_view.unwrap_or(target);
            let resolve_target = msaa_color_view.map(|_| target);
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xr native page pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color_view,
                    resolve_target,
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
                    view: depth_view,
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
            self.render_xr_groups(&self.cached_static_groups, page_bg, glyph_bg, &mut rpass);
            self.render_xr_groups(&ghost_groups, page_bg, glyph_bg, &mut rpass);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
    }

    #[cfg(feature = "xr")]
    #[allow(clippy::too_many_arguments)]
    pub fn render_xr_textured_layers(
        &self,
        left_target: &wgpu::TextureView,
        right_target: &wgpu::TextureView,
        msaa_color_view: Option<&wgpu::TextureView>,
        depth_view: &wgpu::TextureView,
        page_logical_size: [f32; 2],
        panel_size: [f32; 2],
        scroll_y: f32,
        left_mvp: [f32; 16],
        right_mvp: [f32; 16],
        layers: &[XrTextureLayer],
    ) {
        if layers.is_empty() {
            return;
        }

        let logical_width = page_logical_size[0];
        let logical_height = page_logical_size[1];
        let page_to_meter = panel_size[0] / logical_width.max(1.0);
        let max_draw_order = layers
            .iter()
            .map(|layer| layer.draw_order)
            .fold(1.0_f32, f32::max);
        let draw_order_depth_step = 0.001 / max_draw_order;
        let make_uniform = |mvp| XrPageUniform {
            mvp,
            page_panel: [logical_width, logical_height, panel_size[0], panel_size[1]],
            misc: [scroll_y, 1.0, page_to_meter, draw_order_depth_step],
        };

        let left_uniform = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("xr texrect left uniform"),
                contents: bytemuck::bytes_of(&make_uniform(left_mvp)),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let right_uniform = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("xr texrect right uniform"),
                contents: bytemuck::bytes_of(&make_uniform(right_mvp)),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let left_page_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("xr texrect left bg"),
            layout: &self.xr_page_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: left_uniform.as_entire_binding(),
            }],
        });
        let right_page_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("xr texrect right bg"),
            layout: &self.xr_page_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: right_uniform.as_entire_binding(),
            }],
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("xr textured layer encoder"),
            });
        for (target, page_bg) in [(left_target, &left_page_bg), (right_target, &right_page_bg)] {
            let color_view = msaa_color_view.unwrap_or(target);
            let resolve_target = msaa_color_view.map(|_| target);
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xr textured layer pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color_view,
                    resolve_target,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
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
            rpass.set_pipeline(&self.xr_texrect_pipeline);
            rpass.set_bind_group(0, page_bg, &[]);
            rpass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            for layer in layers {
                let instance = XrTexRectInstance {
                    rect: layer.rect,
                    transform: layer.transform,
                    flags: if layer.is_fixed { 1 } else { 0 },
                    draw_order: layer.draw_order,
                    _pad: [0; 2],
                };
                let instance_buf =
                    self.device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("xr texrect instance"),
                            contents: bytemuck::cast_slice(&[instance]),
                            usage: wgpu::BufferUsages::VERTEX,
                        });
                let tex_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("xr texrect texture bg"),
                    layout: &self.texrect_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&layer.view),
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
                    let mut vertices = Vec::new();
                    let mut indices = Vec::new();
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
                        draw_order,
                        &mut vertices,
                        &mut indices,
                    );
                    if !vertices.is_empty() {
                        draw_order += 1.0;
                        if let Some(InternalDrawGroup::Glyphs {
                            vertices: existing_vertices,
                            indices: existing_indices,
                        }) = groups.last_mut()
                        {
                            let base_index = existing_vertices.len() as u32;
                            existing_vertices.extend(vertices);
                            existing_indices
                                .extend(indices.into_iter().map(|index| index + base_index));
                        } else {
                            groups.push(InternalDrawGroup::Glyphs { vertices, indices });
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
                InternalDrawGroup::Glyphs { vertices, indices } => {
                    let vertex_buffer =
                        self.device
                            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                label: Some("slug vertices"),
                                contents: bytemuck::cast_slice(vertices),
                                usage: wgpu::BufferUsages::VERTEX,
                            });
                    let index_buffer =
                        self.device
                            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                label: Some("slug indices"),
                                contents: bytemuck::cast_slice(indices),
                                usage: wgpu::BufferUsages::INDEX,
                            });
                    rpass.set_pipeline(&self.glyph_pipeline);
                    rpass.set_bind_group(0, &self.glyph_bind_group, &[]);
                    rpass.set_vertex_buffer(0, vertex_buffer.slice(..));
                    rpass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    rpass.draw_indexed(0..indices.len() as u32, 0, 0..1);
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

    #[cfg(feature = "xr")]
    fn render_xr_groups<'a>(
        &'a self,
        groups: &[InternalDrawGroup],
        page_bind_group: &'a wgpu::BindGroup,
        glyph_bind_group: &'a wgpu::BindGroup,
        rpass: &mut wgpu::RenderPass<'a>,
    ) {
        for group in groups {
            match group {
                InternalDrawGroup::Rects(instances) => {
                    let buf = self
                        .device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("xr rect instances"),
                            contents: bytemuck::cast_slice(instances),
                            usage: wgpu::BufferUsages::VERTEX,
                        });
                    rpass.set_pipeline(&self.xr_rect_pipeline);
                    rpass.set_bind_group(0, page_bind_group, &[]);
                    rpass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                    rpass.set_vertex_buffer(1, buf.slice(..));
                    rpass.draw(0..6, 0..instances.len() as u32);
                }
                InternalDrawGroup::Glyphs { vertices, indices } => {
                    let vertex_buffer =
                        self.device
                            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                label: Some("xr slug vertices"),
                                contents: bytemuck::cast_slice(vertices),
                                usage: wgpu::BufferUsages::VERTEX,
                            });
                    let index_buffer =
                        self.device
                            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                label: Some("xr slug indices"),
                                contents: bytemuck::cast_slice(indices),
                                usage: wgpu::BufferUsages::INDEX,
                            });
                    rpass.set_pipeline(&self.xr_glyph_pipeline);
                    rpass.set_bind_group(0, glyph_bind_group, &[]);
                    rpass.set_vertex_buffer(0, vertex_buffer.slice(..));
                    rpass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    rpass.draw_indexed(0..indices.len() as u32, 0, 0..1);
                }
                InternalDrawGroup::Lines(instances) => {
                    let buf = self
                        .device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("xr line instances"),
                            contents: bytemuck::cast_slice(instances),
                            usage: wgpu::BufferUsages::VERTEX,
                        });
                    rpass.set_pipeline(&self.xr_line_pipeline);
                    rpass.set_bind_group(0, page_bind_group, &[]);
                    rpass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                    rpass.set_vertex_buffer(1, buf.slice(..));
                    rpass.draw(0..6, 0..instances.len() as u32);
                }
            }
        }
    }

    #[cfg(feature = "xr")]
    fn max_draw_order(&self, groups: &[InternalDrawGroup]) -> f32 {
        groups
            .iter()
            .map(|group| match group {
                InternalDrawGroup::Rects(instances) => instances
                    .iter()
                    .map(|instance| instance.draw_order)
                    .fold(0.0, f32::max),
                InternalDrawGroup::Glyphs { vertices, .. } => vertices
                    .iter()
                    .map(|vertex| vertex.draw_order)
                    .fold(0.0, f32::max),
                InternalDrawGroup::Lines(instances) => instances
                    .iter()
                    .map(|instance| instance.draw_order)
                    .fold(0.0, f32::max),
            })
            .fold(0.0, f32::max)
    }

    #[cfg(feature = "xr")]
    fn offset_draw_orders(&self, groups: &mut [InternalDrawGroup], offset: f32) {
        for group in groups {
            match group {
                InternalDrawGroup::Rects(instances) => {
                    for instance in instances {
                        instance.draw_order += offset;
                    }
                }
                InternalDrawGroup::Glyphs { vertices, .. } => {
                    for vertex in vertices {
                        vertex.draw_order += offset;
                    }
                }
                InternalDrawGroup::Lines(instances) => {
                    for instance in instances {
                        instance.draw_order += offset;
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
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
        draw_order: f32,
        vertices: &mut Vec<SlugVertex>,
        indices: &mut Vec<u32>,
    ) {
        let render_scale = self.render_scale_factor;
        let phys_font_size = font_size * render_scale;
        let metrics = Metrics::new(phys_font_size, phys_font_size * 1.2);
        let attrs = Attrs::new().family(Family::Name(&self.slug_font_family));
        let mut buffer = CosmicBuffer::new(&mut self.font_system, metrics);
        buffer.set_size(&mut self.font_system, Some(max_width * render_scale), None);
        buffer.set_text(&mut self.font_system, text, attrs, Shaping::Advanced);
        buffer.shape_until_scroll(&mut self.font_system, false);

        let inv_render_scale = 1.0 / render_scale;
        let dilation = inv_render_scale;
        let em_dilation = dilation / font_size.max(1.0);
        for run in buffer.layout_runs() {
            for glyph in run.glyphs.iter() {
                let physical = glyph.physical((x * render_scale, y * render_scale), 1.0);
                let Some(entry) = self.slug_atlas.glyphs.get(&physical.cache_key.glyph_id) else {
                    continue;
                };
                if entry.bbox == [0.0; 4] {
                    continue;
                }

                let baseline_x = physical.x as f32 * inv_render_scale;
                let baseline_y = (physical.y as f32 + run.line_y) * inv_render_scale;
                let [x_min, y_min, x_max, y_max] = entry.bbox;
                let left = baseline_x + x_min * font_size;
                let right = baseline_x + x_max * font_size;
                let top = baseline_y - y_max * font_size;
                let bottom = baseline_y - y_min * font_size;

                let glyph_loc_packed =
                    (entry.glyph_loc[0] & 0xFFFF) | ((entry.glyph_loc[1] & 0xFFFF) << 16);
                let band_max_packed =
                    (entry.band_max[0] & 0xFF) | ((entry.band_max[1] & 0xFF) << 16);

                let corners = [
                    (
                        left - dilation,
                        bottom + dilation,
                        x_min - em_dilation,
                        y_min - em_dilation,
                    ),
                    (
                        right + dilation,
                        bottom + dilation,
                        x_max + em_dilation,
                        y_min - em_dilation,
                    ),
                    (
                        right + dilation,
                        top - dilation,
                        x_max + em_dilation,
                        y_max + em_dilation,
                    ),
                    (
                        left - dilation,
                        top - dilation,
                        x_min - em_dilation,
                        y_max + em_dilation,
                    ),
                ];

                let base_index = vertices.len() as u32;
                for (obj_x, obj_y, em_x, em_y) in corners {
                    vertices.push(SlugVertex {
                        pos: [obj_x, obj_y, 0.0, 0.0],
                        tex: [
                            em_x,
                            em_y,
                            f32::from_bits(glyph_loc_packed),
                            f32::from_bits(band_max_packed),
                        ],
                        bnd: entry.band_transform,
                        color,
                        transform,
                        center,
                        flags: if is_fixed { 1u32 } else { 0 },
                        draw_order,
                    });
                }
                indices.extend_from_slice(&[
                    base_index,
                    base_index + 1,
                    base_index + 2,
                    base_index + 2,
                    base_index + 3,
                    base_index,
                ]);
            }
        }
    }

    pub fn measure_text(&mut self, text: &str, font_size: f32, max_width: f32) -> (f32, f32) {
        let line_height = font_size * 1.2;
        let metrics = Metrics::new(font_size, line_height);
        let attrs = Attrs::new().family(Family::Name(&self.slug_font_family));
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
