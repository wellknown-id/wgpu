use anyhow::{anyhow, bail, Result};
use serde_json::Value;

pub(crate) struct RenderPassDescriptorData {
    pub(crate) color_attachments: Vec<RenderPassColorAttachmentData>,
    pub(crate) depth_stencil_attachment: Option<RenderPassDepthStencilAttachmentData>,
}

pub(crate) struct RenderPassColorAttachmentData {
    pub(crate) view_id: u32,
    pub(crate) resolve_target_id: Option<u32>,
    pub(crate) clear_value: Option<[f64; 4]>,
    pub(crate) load_op: Option<String>,
    pub(crate) store_op: Option<String>,
}

pub(crate) struct RenderPassDepthStencilAttachmentData {
    pub(crate) view_id: u32,
    pub(crate) depth_clear_value: Option<f32>,
    pub(crate) depth_load_op: Option<String>,
    pub(crate) depth_store_op: Option<String>,
}

#[derive(Clone)]
pub(crate) struct ParsedVertexBufferLayout {
    pub(crate) array_stride: u64,
    pub(crate) step_mode: wgpu::VertexStepMode,
    pub(crate) attributes: Vec<wgpu::VertexAttribute>,
}

pub(crate) fn texture_size_from_json(value: Option<&Value>) -> Result<wgpu::Extent3d> {
    let value = value.ok_or_else(|| anyhow!("texture descriptor missing size"))?;

    if let Some(array) = value.as_array() {
        let width = array.first().and_then(Value::as_u64).unwrap_or(1) as u32;
        let height = array.get(1).and_then(Value::as_u64).unwrap_or(1) as u32;
        let depth_or_array_layers = array.get(2).and_then(Value::as_u64).unwrap_or(1) as u32;

        return Ok(wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers,
        });
    }

    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("texture size must be array or object"))?;

    Ok(wgpu::Extent3d {
        width: object.get("width").and_then(Value::as_u64).unwrap_or(1) as u32,
        height: object.get("height").and_then(Value::as_u64).unwrap_or(1) as u32,
        depth_or_array_layers: object
            .get("depthOrArrayLayers")
            .and_then(Value::as_u64)
            .unwrap_or(1) as u32,
    })
}

pub(crate) fn parse_texture_dimension(value: &str) -> Result<wgpu::TextureDimension> {
    match value {
        "1d" => Ok(wgpu::TextureDimension::D1),
        "2d" => Ok(wgpu::TextureDimension::D2),
        "3d" => Ok(wgpu::TextureDimension::D3),
        other => bail!("unsupported texture dimension {other}"),
    }
}

pub(crate) fn parse_texture_view_dimension(value: &str) -> Result<wgpu::TextureViewDimension> {
    match value {
        "1d" => Ok(wgpu::TextureViewDimension::D1),
        "2d" => Ok(wgpu::TextureViewDimension::D2),
        "2d-array" => Ok(wgpu::TextureViewDimension::D2Array),
        "cube" => Ok(wgpu::TextureViewDimension::Cube),
        "cube-array" => Ok(wgpu::TextureViewDimension::CubeArray),
        "3d" => Ok(wgpu::TextureViewDimension::D3),
        other => bail!("unsupported texture view dimension {other}"),
    }
}

pub(crate) fn parse_origin_3d(value: Option<&Value>) -> Result<wgpu::Origin3d> {
    let Some(value) = value else {
        return Ok(wgpu::Origin3d::ZERO);
    };

    if let Some(array) = value.as_array() {
        return Ok(wgpu::Origin3d {
            x: array.first().and_then(Value::as_u64).unwrap_or(0) as u32,
            y: array.get(1).and_then(Value::as_u64).unwrap_or(0) as u32,
            z: array.get(2).and_then(Value::as_u64).unwrap_or(0) as u32,
        });
    }

    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("texture origin must be array or object"))?;
    Ok(wgpu::Origin3d {
        x: object.get("x").and_then(Value::as_u64).unwrap_or(0) as u32,
        y: object.get("y").and_then(Value::as_u64).unwrap_or(0) as u32,
        z: object.get("z").and_then(Value::as_u64).unwrap_or(0) as u32,
    })
}

pub(crate) fn parse_texture_aspect(value: &str) -> Result<wgpu::TextureAspect> {
    match value {
        "all" => Ok(wgpu::TextureAspect::All),
        "stencil-only" => Ok(wgpu::TextureAspect::StencilOnly),
        "depth-only" => Ok(wgpu::TextureAspect::DepthOnly),
        other => bail!("unsupported texture aspect {other}"),
    }
}

pub(crate) fn parse_texture_format(value: &str) -> Result<wgpu::TextureFormat> {
    match value {
        "rgba8unorm" => Ok(wgpu::TextureFormat::Rgba8Unorm),
        "rgba8unorm-srgb" => Ok(wgpu::TextureFormat::Rgba8UnormSrgb),
        "bgra8unorm" => Ok(wgpu::TextureFormat::Bgra8Unorm),
        "bgra8unorm-srgb" => Ok(wgpu::TextureFormat::Bgra8UnormSrgb),
        "rgba16float" => Ok(wgpu::TextureFormat::Rgba16Float),
        "depth24plus" => Ok(wgpu::TextureFormat::Depth24Plus),
        "depth24plus-stencil8" => Ok(wgpu::TextureFormat::Depth24PlusStencil8),
        "depth32float" => Ok(wgpu::TextureFormat::Depth32Float),
        other => bail!("unsupported texture format {other}"),
    }
}

pub(crate) fn parse_vertex_buffer_layout(value: &Value) -> Result<ParsedVertexBufferLayout> {
    let array_stride = value
        .get("arrayStride")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("vertex buffer layout missing arrayStride"))?;
    let step_mode = match value
        .get("stepMode")
        .and_then(Value::as_str)
        .unwrap_or("vertex")
    {
        "vertex" => wgpu::VertexStepMode::Vertex,
        "instance" => wgpu::VertexStepMode::Instance,
        other => bail!("unsupported vertex step mode {other}"),
    };
    let attributes = value
        .get("attributes")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("vertex buffer layout missing attributes"))?
        .iter()
        .map(|attribute| {
            Ok(wgpu::VertexAttribute {
                format: parse_vertex_format(
                    attribute
                        .get("format")
                        .and_then(Value::as_str)
                        .ok_or_else(|| anyhow!("vertex attribute missing format"))?,
                )?,
                offset: attribute
                    .get("offset")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| anyhow!("vertex attribute missing offset"))?,
                shader_location: attribute
                    .get("shaderLocation")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| anyhow!("vertex attribute missing shaderLocation"))?
                    as u32,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(ParsedVertexBufferLayout {
        array_stride,
        step_mode,
        attributes,
    })
}

pub(crate) fn parse_vertex_format(value: &str) -> Result<wgpu::VertexFormat> {
    match value {
        "float32" => Ok(wgpu::VertexFormat::Float32),
        "float32x2" => Ok(wgpu::VertexFormat::Float32x2),
        "float32x3" => Ok(wgpu::VertexFormat::Float32x3),
        "float32x4" => Ok(wgpu::VertexFormat::Float32x4),
        "uint16" => Ok(wgpu::VertexFormat::Uint16),
        "uint16x2" => Ok(wgpu::VertexFormat::Uint16x2),
        "uint16x4" => Ok(wgpu::VertexFormat::Uint16x4),
        "uint32" => Ok(wgpu::VertexFormat::Uint32),
        "uint32x2" => Ok(wgpu::VertexFormat::Uint32x2),
        "uint32x3" => Ok(wgpu::VertexFormat::Uint32x3),
        "uint32x4" => Ok(wgpu::VertexFormat::Uint32x4),
        other => bail!("unsupported vertex format {other}"),
    }
}

pub(crate) fn parse_color_target_state(value: &Value) -> Result<Option<wgpu::ColorTargetState>> {
    if value.is_null() {
        return Ok(None);
    }

    let format = parse_texture_format(
        value
            .get("format")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("color target missing format"))?,
    )?;
    let is_canvas_target = value
        .get("isCanvasTarget")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    Ok(Some(wgpu::ColorTargetState {
        format: if is_canvas_target {
            match format {
                wgpu::TextureFormat::Bgra8Unorm => wgpu::TextureFormat::Bgra8UnormSrgb,
                wgpu::TextureFormat::Rgba8Unorm => wgpu::TextureFormat::Rgba8UnormSrgb,
                other => other,
            }
        } else {
            format
        },
        blend: None,
        write_mask: wgpu::ColorWrites::from_bits_truncate(
            value
                .get("writeMask")
                .and_then(Value::as_u64)
                .unwrap_or(wgpu::ColorWrites::ALL.bits() as u64)
                .try_into()?,
        ),
    }))
}

pub(crate) fn parse_primitive_state(value: &Value) -> Result<wgpu::PrimitiveState> {
    let mut primitive = wgpu::PrimitiveState::default();

    if let Some(topology) = value.get("topology").and_then(Value::as_str) {
        primitive.topology = match topology {
            "triangle-list" => wgpu::PrimitiveTopology::TriangleList,
            "triangle-strip" => wgpu::PrimitiveTopology::TriangleStrip,
            "line-list" => wgpu::PrimitiveTopology::LineList,
            "line-strip" => wgpu::PrimitiveTopology::LineStrip,
            "point-list" => wgpu::PrimitiveTopology::PointList,
            other => bail!("unsupported primitive topology {other}"),
        };
    }

    if let Some(front_face) = value.get("frontFace").and_then(Value::as_str) {
        primitive.front_face = match front_face {
            "ccw" => wgpu::FrontFace::Ccw,
            "cw" => wgpu::FrontFace::Cw,
            other => bail!("unsupported front face {other}"),
        };
    }

    primitive.cull_mode = match value.get("cullMode").and_then(Value::as_str) {
        Some("front") => Some(wgpu::Face::Front),
        Some("back") => Some(wgpu::Face::Back),
        Some("none") | None => None,
        Some(other) => bail!("unsupported cull mode {other}"),
    };

    Ok(primitive)
}

pub(crate) fn parse_depth_stencil_state(value: &Value) -> Result<wgpu::DepthStencilState> {
    Ok(wgpu::DepthStencilState {
        format: parse_texture_format(
            value
                .get("format")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("depthStencil missing format"))?,
        )?,
        depth_write_enabled: Some(
            value
                .get("depthWriteEnabled")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        ),
        depth_compare: Some(parse_compare_function(
            value
                .get("depthCompare")
                .and_then(Value::as_str)
                .unwrap_or("less"),
        )?),
        stencil: Default::default(),
        bias: Default::default(),
    })
}

pub(crate) fn parse_compare_function(value: &str) -> Result<wgpu::CompareFunction> {
    match value {
        "never" => Ok(wgpu::CompareFunction::Never),
        "less" => Ok(wgpu::CompareFunction::Less),
        "equal" => Ok(wgpu::CompareFunction::Equal),
        "less-equal" => Ok(wgpu::CompareFunction::LessEqual),
        "greater" => Ok(wgpu::CompareFunction::Greater),
        "not-equal" => Ok(wgpu::CompareFunction::NotEqual),
        "greater-equal" => Ok(wgpu::CompareFunction::GreaterEqual),
        "always" => Ok(wgpu::CompareFunction::Always),
        other => bail!("unsupported compare function {other}"),
    }
}

pub(crate) fn parse_address_mode(value: &str) -> Result<wgpu::AddressMode> {
    match value {
        "clamp-to-edge" => Ok(wgpu::AddressMode::ClampToEdge),
        "repeat" => Ok(wgpu::AddressMode::Repeat),
        "mirror-repeat" => Ok(wgpu::AddressMode::MirrorRepeat),
        other => bail!("unsupported address mode {other}"),
    }
}

pub(crate) fn parse_filter_mode(value: &str) -> Result<wgpu::FilterMode> {
    match value {
        "nearest" => Ok(wgpu::FilterMode::Nearest),
        "linear" => Ok(wgpu::FilterMode::Linear),
        other => bail!("unsupported filter mode {other}"),
    }
}

pub(crate) fn parse_mipmap_filter_mode(value: &str) -> Result<wgpu::MipmapFilterMode> {
    match value {
        "nearest" => Ok(wgpu::MipmapFilterMode::Nearest),
        "linear" => Ok(wgpu::MipmapFilterMode::Linear),
        other => bail!("unsupported mipmap filter mode {other}"),
    }
}

pub(crate) fn parse_multisample_state(value: &Value) -> Result<wgpu::MultisampleState> {
    Ok(wgpu::MultisampleState {
        count: value.get("count").and_then(Value::as_u64).unwrap_or(1) as u32,
        mask: !0,
        alpha_to_coverage_enabled: value
            .get("alphaToCoverageEnabled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

pub(crate) fn parse_render_pass_descriptor(value: &Value) -> Result<RenderPassDescriptorData> {
    let color_attachments = value
        .get("colorAttachments")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("render pass descriptor missing colorAttachments"))?
        .iter()
        .filter(|attachment| !attachment.is_null())
        .map(|attachment| {
            Ok(RenderPassColorAttachmentData {
                view_id: attachment
                    .get("view")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| anyhow!("color attachment missing view"))?
                    as u32,
                resolve_target_id: attachment
                    .get("resolveTarget")
                    .and_then(Value::as_u64)
                    .map(|id| id as u32),
                clear_value: attachment
                    .get("clearValue")
                    .and_then(Value::as_array)
                    .map(|values| {
                        Ok::<[f64; 4], anyhow::Error>([
                            values.first().and_then(Value::as_f64).unwrap_or(0.0),
                            values.get(1).and_then(Value::as_f64).unwrap_or(0.0),
                            values.get(2).and_then(Value::as_f64).unwrap_or(0.0),
                            values.get(3).and_then(Value::as_f64).unwrap_or(1.0),
                        ])
                    })
                    .transpose()?,
                load_op: attachment
                    .get("loadOp")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                store_op: attachment
                    .get("storeOp")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let depth_stencil_attachment = value
        .get("depthStencilAttachment")
        .filter(|attachment| !attachment.is_null())
        .map(|attachment| {
            Ok::<RenderPassDepthStencilAttachmentData, anyhow::Error>(
                RenderPassDepthStencilAttachmentData {
                    view_id: attachment
                        .get("view")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| anyhow!("depth stencil attachment missing view"))?
                        as u32,
                    depth_clear_value: attachment
                        .get("depthClearValue")
                        .and_then(Value::as_f64)
                        .map(|value| value as f32),
                    depth_load_op: attachment
                        .get("depthLoadOp")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    depth_store_op: attachment
                        .get("depthStoreOp")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                },
            )
        })
        .transpose()?;

    Ok(RenderPassDescriptorData {
        color_attachments,
        depth_stencil_attachment,
    })
}

pub(crate) fn parse_load_op_color(
    load_op: Option<&str>,
    clear_value: Option<[f64; 4]>,
) -> Result<wgpu::LoadOp<wgpu::Color>> {
    match load_op.unwrap_or("load") {
        "load" => Ok(wgpu::LoadOp::Load),
        "clear" => {
            let clear_value = clear_value.unwrap_or([0.0, 0.0, 0.0, 1.0]);
            Ok(wgpu::LoadOp::Clear(wgpu::Color {
                r: clear_value[0],
                g: clear_value[1],
                b: clear_value[2],
                a: clear_value[3],
            }))
        }
        other => bail!("unsupported color load op {other}"),
    }
}

pub(crate) fn fallback_clear_load_op_color(
    clear_value: Option<[f64; 4]>,
) -> wgpu::LoadOp<wgpu::Color> {
    let clear_value = clear_value.unwrap_or([0.125, 0.25, 0.375, 1.0]);
    wgpu::LoadOp::Clear(wgpu::Color {
        r: clear_value[0],
        g: clear_value[1],
        b: clear_value[2],
        a: clear_value[3],
    })
}

pub(crate) fn fallback_clear_load_op_depth(clear_value: Option<f32>) -> wgpu::LoadOp<f32> {
    wgpu::LoadOp::Clear(clear_value.unwrap_or(1.0))
}

pub(crate) fn parse_load_op_depth(
    load_op: Option<&str>,
    clear_value: Option<f32>,
) -> Result<wgpu::LoadOp<f32>> {
    match load_op.unwrap_or("load") {
        "load" => Ok(wgpu::LoadOp::Load),
        "clear" => Ok(wgpu::LoadOp::Clear(clear_value.unwrap_or(1.0))),
        other => bail!("unsupported depth load op {other}"),
    }
}

pub(crate) fn parse_store_op(store_op: Option<&str>) -> Result<wgpu::StoreOp> {
    match store_op.unwrap_or("store") {
        "store" => Ok(wgpu::StoreOp::Store),
        "discard" => Ok(wgpu::StoreOp::Discard),
        other => bail!("unsupported store op {other}"),
    }
}

pub(crate) fn parse_index_format(format: &str) -> Result<wgpu::IndexFormat> {
    match format {
        "uint16" => Ok(wgpu::IndexFormat::Uint16),
        "uint32" => Ok(wgpu::IndexFormat::Uint32),
        other => bail!("unsupported index format {other}"),
    }
}

pub(crate) fn flip_rgba_rows(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let row_len = width as usize * 4;
    let mut flipped = vec![0; data.len()];

    for y in 0..height as usize {
        let src = y * row_len;
        let dst = (height as usize - 1 - y) * row_len;
        flipped[dst..dst + row_len].copy_from_slice(&data[src..src + row_len]);
    }

    flipped
}
