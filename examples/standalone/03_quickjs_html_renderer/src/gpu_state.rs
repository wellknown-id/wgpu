use std::{borrow::Cow, collections::HashMap, path::Path, sync::Arc};

use anyhow::{anyhow, bail, Result};
use glam::Mat4;
use serde::Deserialize;
use serde_json::Value;
use wgpu::util::DeviceExt;
use winit::{event_loop::OwnedDisplayHandle, window::Window};

use crate::gpu_parsers::*;
use crate::ui_overlay;
use crate::{CameraRaw, FrameUpload, GeometryUpload, InstanceRaw, Vertex};

pub(crate) struct GpuGeometry {
    pub(crate) vertex_buffer: wgpu::Buffer,
    pub(crate) index_buffer: wgpu::Buffer,
    pub(crate) index_count: u32,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ComputePipelineDescriptorData {
    layout: Option<u32>,
    compute: ProgrammableStageData,
    label: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct ProgrammableStageData {
    module: u32,
    entry_point: Option<String>,
}

#[derive(Debug)]
pub(crate) enum ComputeCommand {
    SetPipeline {
        pipeline_id: u32,
    },
    SetBindGroup {
        index: u32,
        bind_group_id: u32,
        dynamic_offsets: Vec<u32>,
    },
    DispatchWorkgroups {
        x: u32,
        y: u32,
        z: u32,
    },
    DispatchWorkgroupsIndirect {
        buffer_id: u32,
        offset: u64,
    },
}

pub(crate) struct RecordedComputePass {
    pub(crate) label: Option<String>,
    pub(crate) commands: Vec<ComputeCommand>,
}

pub(crate) struct JsComputePass {
    pub(crate) encoder_id: u32,
    pub(crate) label: Option<String>,
    pub(crate) commands: Vec<ComputeCommand>,
}

pub(crate) enum JsTextureResource {
    Surface(wgpu::SurfaceTexture),
    Owned(wgpu::Texture),
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CopyTextureToTextureDescriptor {
    source: CopyImageTextureDetails,
    destination: CopyImageTextureDetails,
    copy_size: [u32; 3],
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CopyTextureToBufferDescriptor {
    source: CopyImageTextureDetails,
    destination: CopyImageBufferDetails,
    copy_size: [u32; 3],
}

pub(crate) struct JsCommandEncoder {
    pub(crate) encoder: wgpu::CommandEncoder,
    pub(crate) recorded_commands: Vec<RecordedEncoderCommand>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CopyImageTextureDetails {
    texture: u32,
    mip_level: u32,
    origin: [u32; 3],
    aspect: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CopyImageBufferDetails {
    buffer: u32,
    offset: u64,
    bytes_per_row: Option<u32>,
    rows_per_image: Option<u32>,
}

pub(crate) enum RecordedEncoderCommand {
    RenderPass(RecordedRenderPass),
    ComputePass(RecordedComputePass),
    CopyTextureToTexture {
        source: CopyImageTextureDetails,
        destination: CopyImageTextureDetails,
        copy_size: [u32; 3],
    },
    CopyTextureToBuffer {
        source: CopyImageTextureDetails,
        destination: CopyImageBufferDetails,
        copy_size: [u32; 3],
    },
}

pub(crate) struct JsRenderBundleEncoder {
    pub(crate) commands: Vec<RenderCommand>,
}

pub(crate) struct JsRenderBundle {
    pub(crate) commands: Vec<RenderCommand>,
}

pub(crate) struct JsRenderPass {
    pub(crate) encoder_id: u32,
    pub(crate) descriptor: RenderPassDescriptorData,
    pub(crate) commands: Vec<RenderCommand>,
}

pub(crate) struct JsCommandBuffer {
    pub(crate) command_buffer: Option<wgpu::CommandBuffer>,
}

pub(crate) struct JsBuffer {
    pub(crate) buffer: wgpu::Buffer,
    pub(crate) mapped: Option<Vec<u8>>,
    pub(crate) size: u64,
    pub(crate) shadow: Vec<u8>,
}

pub(crate) struct RecordedRenderPass {
    pub(crate) descriptor: RenderPassDescriptorData,
    pub(crate) commands: Vec<RenderCommand>,
}

pub(crate) enum RenderCommand {
    SetPipeline {
        pipeline_id: u32,
    },
    SetBindGroup {
        index: u32,
        bind_group_id: u32,
    },
    SetIndexBuffer {
        buffer_id: u32,
        format: String,
    },
    SetVertexBuffer {
        slot: u32,
        buffer_id: u32,
    },
    SetViewport {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        min_depth: f32,
        max_depth: f32,
    },
    SetScissorRect {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
    SetStencilReference {
        reference: u32,
    },
    ExecuteBundles {
        bundle_ids: Vec<u32>,
    },
    Draw {
        vertex_count: u32,
        instance_count: u32,
        first_vertex: u32,
        first_instance: u32,
    },
    DrawIndexed {
        index_count: u32,
        instance_count: u32,
        first_index: u32,
        base_vertex: i32,
        first_instance: u32,
    },
}

pub(crate) struct JsBindGroupLayout {
    pub(crate) layout: wgpu::BindGroupLayout,
}

pub(crate) struct JsPipelineLayout {
    pub(crate) layout: wgpu::PipelineLayout,
}

pub(crate) struct JsBindGroup {
    pub(crate) group: wgpu::BindGroup,
    pub(crate) descriptor_json: String,
}

pub(crate) struct JsShaderModule {
    pub(crate) module: wgpu::ShaderModule,
}

pub(crate) struct JsRenderPipeline {
    pub(crate) pipeline: wgpu::RenderPipeline,
    pub(crate) bind_group_layouts: HashMap<u32, u32>,
    pub(crate) label: Option<String>,
}

pub(crate) struct JsComputePipeline {
    pub(crate) pipeline: wgpu::ComputePipeline,
    pub(crate) bind_group_layouts: HashMap<u32, u32>,
    _label: Option<String>,
}

pub(crate) struct JsSampler {
    pub(crate) sampler: wgpu::Sampler,
}

pub(crate) struct JsImage {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba8: Vec<u8>,
}

pub(crate) struct GpuState {
    pub(crate) instance: wgpu::Instance,
    pub(crate) window: Arc<Window>,
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    pub(crate) size: winit::dpi::PhysicalSize<u32>,
    pub(crate) surface: wgpu::Surface<'static>,
    pub(crate) surface_format: wgpu::TextureFormat,
    pub(crate) camera_buffer: wgpu::Buffer,
    pub(crate) camera_bind_group: wgpu::BindGroup,
    pub(crate) pipeline: wgpu::RenderPipeline,
    pub(crate) instance_buffer: wgpu::Buffer,
    pub(crate) instance_capacity: u32,
    pub(crate) depth_view: Option<wgpu::TextureView>,
    pub(crate) geometry: Option<GpuGeometry>,
    pub(crate) js_next_id: u32,
    pub(crate) js_canvas_context_id: u32,
    pub(crate) js_current_surface_texture: Option<u32>,
    pub(crate) js_textures: HashMap<u32, JsTextureResource>,
    pub(crate) js_texture_views: HashMap<u32, wgpu::TextureView>,
    pub(crate) js_texture_view_owners: HashMap<u32, u32>,
    pub(crate) js_buffers: HashMap<u32, JsBuffer>,
    pub(crate) js_bind_group_layouts: HashMap<u32, JsBindGroupLayout>,
    pub(crate) js_pipeline_layouts: HashMap<u32, JsPipelineLayout>,
    pub(crate) js_bind_groups: HashMap<u32, JsBindGroup>,
    pub(crate) js_shader_modules: HashMap<u32, JsShaderModule>,
    pub(crate) js_render_pipelines: HashMap<u32, JsRenderPipeline>,
    pub(crate) js_compute_pipelines: HashMap<u32, JsComputePipeline>,
    pub(crate) js_samplers: HashMap<u32, JsSampler>,
    pub(crate) js_images: HashMap<u32, JsImage>,
    pub(crate) js_command_encoders: HashMap<u32, JsCommandEncoder>,
    pub(crate) js_render_bundle_encoders: HashMap<u32, JsRenderBundleEncoder>,
    pub(crate) js_render_bundles: HashMap<u32, JsRenderBundle>,
    pub(crate) js_render_passes: HashMap<u32, JsRenderPass>,
    pub(crate) js_compute_passes: HashMap<u32, JsComputePass>,
    pub(crate) js_command_buffers: HashMap<u32, JsCommandBuffer>,
    pub(crate) ui: Option<ui_overlay::UiOverlay>,
    pub(crate) mouse: (f32, f32),
}

const SHADER: &str = r#"
struct Camera {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) model_0: vec4<f32>,
    @location(3) model_1: vec4<f32>,
    @location(4) model_2: vec4<f32>,
    @location(5) model_3: vec4<f32>,
};

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) normal: vec3<f32>,
};

@vertex
fn vs_main(input: VertexIn) -> VertexOut {
    let model = mat4x4<f32>(input.model_0, input.model_1, input.model_2, input.model_3);
    let world_position = model * vec4<f32>(input.position, 1.0);
    let world_normal = normalize(mat3x3<f32>(model[0].xyz, model[1].xyz, model[2].xyz) * input.normal);

    var output: VertexOut;
    output.position = camera.view_proj * world_position;
    output.normal = world_normal;
    return output;
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    let light = normalize(vec3<f32>(0.4, 0.8, 0.3));
    let diffuse = max(dot(normalize(input.normal), light), 0.0);
    let ambient = 0.25;
    let shade = ambient + diffuse * 0.75;
    let base = vec3<f32>(0.62, 0.78, 0.98);
    return vec4<f32>(base * shade, 1.0);
}
"#;

fn infer_auto_bind_group_layout_entries(
    pipeline_id: u32,
    index: u32,
    gpu: &GpuState,
) -> Result<Vec<Value>> {
    let pipeline = gpu
        .js_render_pipelines
        .get(&pipeline_id)
        .ok_or_else(|| anyhow!("unknown render pipeline handle {pipeline_id}"))?;

    if index == 0 {
        if let Some(label) = &pipeline.label {
            if label.starts_with("mipmap-") {
                return Ok(vec![
                    serde_json::json!({ "binding": 0, "sampler": {} }),
                    serde_json::json!({ "binding": 1, "texture": {} }),
                    serde_json::json!({ "binding": 2, "buffer": {} }),
                ]);
            }
        }
    }

    Ok(Vec::new())
}

impl GpuState {
    pub(crate) async fn new(display: OwnedDisplayHandle, window: Arc<Window>) -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
            Box::new(display),
        ));
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .context("failed to request adapter")?;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .context("failed to request device")?;

        let size = window.inner_size();
        let surface = instance
            .create_surface(window.clone())
            .context("failed to create surface")?;
        let capabilities = surface.get_capabilities(&adapter);
        let surface_format = capabilities
            .formats
            .first()
            .copied()
            .ok_or_else(|| anyhow!("surface has no supported formats"))?;

        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera buffer"),
            contents: bytemuck::bytes_of(&CameraRaw {
                view_proj: Mat4::IDENTITY.to_cols_array(),
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let camera_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("camera bind group layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera bind group"),
            layout: &camera_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quickjs html renderer shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(SHADER)),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pipeline layout"),
            bind_group_layouts: &[Some(&camera_bind_group_layout)],
            immediate_size: 0,
        });

        let target_format = surface_format.add_srgb_suffix();
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("render pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: None,
                compilation_options: Default::default(),
                buffers: &[Vertex::layout(), InstanceRaw::layout()],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24Plus,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: None,
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instance buffer"),
            size: std::mem::size_of::<InstanceRaw>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let ui = ui_overlay::UiOverlay::new(&device, &queue, surface_format);

        let mut state = Self {
            instance,
            window,
            device,
            queue,
            size,
            surface,
            surface_format,
            camera_buffer,
            camera_bind_group,
            pipeline,
            instance_buffer,
            instance_capacity: 1,
            depth_view: None,
            geometry: None,
            js_next_id: 2,
            js_canvas_context_id: 1,
            js_current_surface_texture: None,
            js_textures: HashMap::new(),
            js_texture_views: HashMap::new(),
            js_texture_view_owners: HashMap::new(),
            js_buffers: HashMap::new(),
            js_bind_group_layouts: HashMap::new(),
            js_pipeline_layouts: HashMap::new(),
            js_bind_groups: HashMap::new(),
            js_shader_modules: HashMap::new(),
            js_render_pipelines: HashMap::new(),
            js_compute_pipelines: HashMap::new(),
            js_samplers: HashMap::new(),
            js_images: HashMap::new(),
            js_command_encoders: HashMap::new(),
            js_render_bundle_encoders: HashMap::new(),
            js_render_bundles: HashMap::new(),
            js_render_passes: HashMap::new(),
            js_compute_passes: HashMap::new(),
            js_command_buffers: HashMap::new(),
            ui: Some(ui),
            mouse: (0.0, 0.0),
        };

        state.configure_surface();
        state.depth_view = Some(state.create_depth_view());

        Ok(state)
    }

    pub(crate) fn window(&self) -> &Window {
        &self.window
    }

    pub(crate) fn next_js_id(&mut self) -> u32 {
        let id = self.js_next_id;
        self.js_next_id += 1;
        id
    }

    pub(crate) fn js_request_adapter(&self) -> u32 {
        1
    }

    pub(crate) fn js_request_device(&mut self, _adapter_id: u32) -> Result<u32> {
        Ok(1)
    }

    pub(crate) fn js_get_queue(&self, _device_id: u32) -> u32 {
        1
    }

    pub(crate) fn js_create_canvas_context(&mut self) -> u32 {
        self.js_canvas_context_id
    }

    pub(crate) fn js_get_preferred_canvas_format(&self) -> String {
        match self.surface_format {
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => {
                "bgra8unorm".to_string()
            }
            wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => {
                "rgba8unorm".to_string()
            }
            _ => "bgra8unorm".to_string(),
        }
    }

    pub(crate) fn js_configure_canvas_context(
        &mut self,
        _context_id: u32,
        _device_id: u32,
        _descriptor_json: &str,
    ) -> Result<()> {
        if self.size.width > 0 && self.size.height > 0 {
            self.configure_surface();
            self.depth_view = Some(self.create_depth_view());
        }
        Ok(())
    }

    pub(crate) fn js_get_current_texture(&mut self, _context_id: u32) -> Result<u32> {
        if let Some(texture_id) = self.js_current_surface_texture {
            return Ok(texture_id);
        }

        let mut retry = 0;
        let surface_texture = loop {
            match self.surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(texture)
                | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => break texture,
                wgpu::CurrentSurfaceTexture::Occluded | wgpu::CurrentSurfaceTexture::Timeout => {
                    bail!("surface is temporarily unavailable")
                }
                wgpu::CurrentSurfaceTexture::Outdated => {
                    if retry > 0 || self.size.width == 0 || self.size.height == 0 {
                        let dummy_texture = self.device.create_texture(&wgpu::TextureDescriptor {
                            label: Some("dummy outdated surface replacement"),
                            size: wgpu::Extent3d {
                                width: self.size.width.max(1),
                                height: self.size.height.max(1),
                                depth_or_array_layers: 1,
                            },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            format: self.surface_format,
                            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                                | wgpu::TextureUsages::COPY_SRC
                                | wgpu::TextureUsages::COPY_DST
                                | wgpu::TextureUsages::TEXTURE_BINDING,
                            view_formats: &[],
                        });
                        let id = self.next_js_id();
                        self.js_textures
                            .insert(id, JsTextureResource::Owned(dummy_texture));
                        self.js_current_surface_texture = Some(id);
                        return Ok(id);
                    }

                    self.configure_surface();
                    self.depth_view = Some(self.create_depth_view());
                    retry += 1;
                }
                wgpu::CurrentSurfaceTexture::Validation => {
                    bail!("surface validation failed")
                }
                wgpu::CurrentSurfaceTexture::Lost => {
                    self.surface = self.instance.create_surface(self.window.clone())?;
                    self.configure_surface();
                    self.depth_view = Some(self.create_depth_view());
                    bail!("surface was lost")
                }
            }
        };

        let id = self.next_js_id();
        self.js_textures
            .insert(id, JsTextureResource::Surface(surface_texture));
        self.js_current_surface_texture = Some(id);
        Ok(id)
    }

    pub(crate) fn js_create_texture(
        &mut self,
        _device_id: u32,
        descriptor_json: &str,
    ) -> Result<u32> {
        let descriptor: Value = serde_json::from_str(descriptor_json)?;

        let size = texture_size_from_json(descriptor.get("size"))?;
        let mip_level_count = descriptor
            .get("mipLevelCount")
            .and_then(Value::as_u64)
            .unwrap_or(1) as u32;
        let sample_count = descriptor
            .get("sampleCount")
            .and_then(Value::as_u64)
            .unwrap_or(1) as u32;
        let dimension = parse_texture_dimension(
            descriptor
                .get("dimension")
                .and_then(Value::as_str)
                .unwrap_or("2d"),
        )?;
        let format = parse_texture_format(
            descriptor
                .get("format")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("texture descriptor missing format"))?,
        )?;
        let usage = wgpu::TextureUsages::from_bits_truncate(
            descriptor
                .get("usage")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow!("texture descriptor missing usage"))?
                .try_into()?,
        );

        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: descriptor.get("label").and_then(Value::as_str),
            size,
            mip_level_count,
            sample_count,
            dimension,
            format,
            usage,
            view_formats: &[],
        });

        let id = self.next_js_id();
        self.js_textures
            .insert(id, JsTextureResource::Owned(texture));
        Ok(id)
    }

    pub(crate) fn js_texture_create_view(
        &mut self,
        texture_id: u32,
        descriptor_json: &str,
    ) -> Result<u32> {
        let texture = self
            .js_textures
            .get(&texture_id)
            .ok_or_else(|| anyhow!("unknown texture handle {texture_id}"))?;

        let descriptor: Value = serde_json::from_str(descriptor_json)?;
        let format = descriptor
            .get("format")
            .and_then(Value::as_str)
            .map(parse_texture_format)
            .transpose()?;
        let dimension = descriptor
            .get("dimension")
            .and_then(Value::as_str)
            .map(parse_texture_view_dimension)
            .transpose()?;
        let aspect = parse_texture_aspect(
            descriptor
                .get("aspect")
                .and_then(Value::as_str)
                .unwrap_or("all"),
        )?;
        let base_mip_level = descriptor
            .get("baseMipLevel")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;
        let mip_level_count = descriptor
            .get("mipLevelCount")
            .and_then(Value::as_u64)
            .map(|value| value as u32);
        let base_array_layer = descriptor
            .get("baseArrayLayer")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;
        let array_layer_count = descriptor
            .get("arrayLayerCount")
            .and_then(Value::as_u64)
            .map(|value| value as u32);

        let view_descriptor = wgpu::TextureViewDescriptor {
            label: descriptor.get("label").and_then(Value::as_str),
            format,
            dimension,
            usage: None,
            aspect,
            base_mip_level,
            mip_level_count,
            base_array_layer,
            array_layer_count,
        };

        let view = match texture {
            JsTextureResource::Surface(surface_texture) => {
                surface_texture.texture.create_view(&view_descriptor)
            }
            JsTextureResource::Owned(texture) => texture.create_view(&view_descriptor),
        };

        let view_id = self.next_js_id();
        self.js_texture_views.insert(view_id, view);
        self.js_texture_view_owners.insert(view_id, texture_id);
        Ok(view_id)
    }

    pub(crate) fn js_create_command_encoder(
        &mut self,
        _device_id: u32,
        _descriptor_json: &str,
    ) -> Result<u32> {
        let id = self.next_js_id();
        let encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("js command encoder"),
            });
        self.js_command_encoders.insert(
            id,
            JsCommandEncoder {
                encoder,
                recorded_commands: Vec::new(),
            },
        );
        Ok(id)
    }

    pub(crate) fn js_create_buffer(
        &mut self,
        _device_id: u32,
        descriptor_json: &str,
    ) -> Result<u32> {
        let descriptor: Value = serde_json::from_str(descriptor_json)?;
        let size = descriptor
            .get("size")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("buffer descriptor missing size"))?;
        let usage = wgpu::BufferUsages::from_bits_truncate(
            descriptor
                .get("usage")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow!("buffer descriptor missing usage"))?
                .try_into()?,
        );
        let mapped_at_creation = descriptor
            .get("mappedAtCreation")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: descriptor.get("label").and_then(Value::as_str),
            size,
            usage,
            mapped_at_creation: false,
        });

        let id = self.next_js_id();
        self.js_buffers.insert(
            id,
            JsBuffer {
                buffer,
                mapped: mapped_at_creation.then(|| vec![0; size as usize]),
                size,
                shadow: vec![0; size as usize],
            },
        );
        Ok(id)
    }

    pub(crate) fn js_create_render_bundle_encoder(
        &mut self,
        _device_id: u32,
        _descriptor_json: &str,
    ) -> Result<u32> {
        let id = self.next_js_id();
        self.js_render_bundle_encoders.insert(
            id,
            JsRenderBundleEncoder {
                commands: Vec::new(),
            },
        );
        Ok(id)
    }

    pub(crate) fn js_buffer_get_mapped_range(
        &mut self,
        buffer_id: u32,
        offset: u64,
        size: u64,
    ) -> Result<Vec<u8>> {
        let buffer = self
            .js_buffers
            .get_mut(&buffer_id)
            .ok_or_else(|| anyhow!("unknown buffer handle {buffer_id}"))?;
        let mapped = buffer
            .mapped
            .as_ref()
            .ok_or_else(|| anyhow!("buffer {buffer_id} is not mapped"))?;
        let end = offset
            .checked_add(size)
            .ok_or_else(|| anyhow!("mapped range overflow"))?;
        if end > buffer.size {
            bail!("mapped range exceeds buffer size");
        }

        Ok(mapped[offset as usize..end as usize].to_vec())
    }

    pub(crate) fn js_buffer_set_mapped_range(
        &mut self,
        buffer_id: u32,
        bytes: &[u8],
    ) -> Result<()> {
        let buffer = self
            .js_buffers
            .get_mut(&buffer_id)
            .ok_or_else(|| anyhow!("unknown buffer handle {buffer_id}"))?;
        let mapped = buffer
            .mapped
            .as_mut()
            .ok_or_else(|| anyhow!("buffer {buffer_id} is not mapped"))?;

        if bytes.len() != mapped.len() {
            bail!(
                "mapped range replacement size mismatch: expected {}, got {}",
                mapped.len(),
                bytes.len()
            );
        }

        mapped.copy_from_slice(bytes);
        Ok(())
    }

    pub(crate) fn js_buffer_unmap(&mut self, buffer_id: u32) -> Result<()> {
        let buffer = self
            .js_buffers
            .get_mut(&buffer_id)
            .ok_or_else(|| anyhow!("unknown buffer handle {buffer_id}"))?;

        if let Some(mapped) = buffer.mapped.take() {
            buffer.shadow = mapped.clone();
            self.queue.write_buffer(&buffer.buffer, 0, &mapped);
        }

        Ok(())
    }

    pub(crate) fn js_begin_render_pass(
        &mut self,
        encoder_id: u32,
        descriptor_json: &str,
    ) -> Result<u32> {
        if !self.js_command_encoders.contains_key(&encoder_id) {
            bail!("unknown command encoder handle {encoder_id}");
        }

        let descriptor = parse_render_pass_descriptor(&serde_json::from_str(descriptor_json)?)?;

        let id = self.next_js_id();
        self.js_render_passes.insert(
            id,
            JsRenderPass {
                encoder_id,
                descriptor,
                commands: Vec::new(),
            },
        );
        Ok(id)
    }

    pub(crate) fn js_render_pass_end(&mut self, render_pass_id: u32) -> Result<()> {
        let render_pass = self
            .js_render_passes
            .remove(&render_pass_id)
            .ok_or_else(|| anyhow!("unknown render pass handle {render_pass_id}"))?;
        let encoder = self
            .js_command_encoders
            .get_mut(&render_pass.encoder_id)
            .ok_or_else(|| anyhow!("unknown command encoder handle {}", render_pass.encoder_id))?;
        encoder
            .recorded_commands
            .push(RecordedEncoderCommand::RenderPass(RecordedRenderPass {
                descriptor: render_pass.descriptor,
                commands: render_pass.commands,
            }));
        Ok(())
    }

    pub(crate) fn js_command_encoder_copy_texture_to_texture(
        &mut self,
        encoder_id: u32,
        descriptor_json: &str,
    ) -> Result<()> {
        let descriptor: CopyTextureToTextureDescriptor = serde_json::from_str(descriptor_json)?;
        let encoder = self
            .js_command_encoders
            .get_mut(&encoder_id)
            .ok_or_else(|| anyhow!("unknown command encoder handle {encoder_id}"))?;

        encoder
            .recorded_commands
            .push(RecordedEncoderCommand::CopyTextureToTexture {
                source: descriptor.source,
                destination: descriptor.destination,
                copy_size: descriptor.copy_size,
            });

        Ok(())
    }

    pub(crate) fn js_command_encoder_copy_texture_to_buffer(
        &mut self,
        encoder_id: u32,
        descriptor_json: &str,
    ) -> Result<()> {
        let descriptor: CopyTextureToBufferDescriptor = serde_json::from_str(descriptor_json)?;
        let encoder = self
            .js_command_encoders
            .get_mut(&encoder_id)
            .ok_or_else(|| anyhow!("unknown command encoder handle {encoder_id}"))?;

        encoder
            .recorded_commands
            .push(RecordedEncoderCommand::CopyTextureToBuffer {
                source: descriptor.source,
                destination: descriptor.destination,
                copy_size: descriptor.copy_size,
            });

        Ok(())
    }

    pub(crate) fn js_command_encoder_finish(&mut self, encoder_id: u32) -> Result<u32> {
        if self
            .js_render_passes
            .values()
            .any(|render_pass| render_pass.encoder_id == encoder_id)
        {
            bail!("cannot finish command encoder with active render pass");
        }

        let mut encoder = self
            .js_command_encoders
            .remove(&encoder_id)
            .ok_or_else(|| anyhow!("unknown command encoder handle {encoder_id}"))?;

        for command in &encoder.recorded_commands {
            let pass = match command {
                RecordedEncoderCommand::RenderPass(pass) => pass,
                _ => continue,
            };
            let has_surface_target = pass.descriptor.color_attachments.iter().any(|attachment| {
                self.js_texture_view_owners
                    .get(&attachment.view_id)
                    .and_then(|owner| self.js_textures.get(owner))
                    .is_some_and(|texture| matches!(texture, JsTextureResource::Surface(_)))
            });

            let _draw_count = pass
                .commands
                .iter()
                .filter(|command| {
                    matches!(
                        command,
                        RenderCommand::Draw { .. } | RenderCommand::DrawIndexed { .. }
                    )
                })
                .count();

            if has_surface_target {
                let _command_kinds = pass
                    .commands
                    .iter()
                    .map(|command| match command {
                        RenderCommand::SetPipeline { .. } => "setPipeline",
                        RenderCommand::SetBindGroup { .. } => "setBindGroup",
                        RenderCommand::SetIndexBuffer { .. } => "setIndexBuffer",
                        RenderCommand::SetVertexBuffer { .. } => "setVertexBuffer",
                        RenderCommand::SetViewport { .. } => "setViewport",
                        RenderCommand::SetScissorRect { .. } => "setScissorRect",
                        RenderCommand::SetStencilReference { .. } => "setStencilReference",
                        RenderCommand::ExecuteBundles { .. } => "executeBundles",
                        RenderCommand::Draw { .. } => "draw",
                        RenderCommand::DrawIndexed { .. } => "drawIndexed",
                    })
                    .collect::<Vec<_>>();
                let _bind_group_details = pass
                    .commands
                    .iter()
                    .filter_map(|command| match command {
                        RenderCommand::SetBindGroup {
                            index,
                            bind_group_id,
                        } => {
                            let descriptor = self
                                .js_bind_groups
                                .get(bind_group_id)
                                .map(|group| group.descriptor_json.as_str())
                                .unwrap_or("<missing>");
                            Some(format!("bg{index}={bind_group_id}:{descriptor}"))
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                let _uniform_previews = pass
                    .commands
                    .iter()
                    .filter_map(|command| match command {
                        RenderCommand::SetBindGroup { bind_group_id, .. } => self
                            .js_bind_groups
                            .get(bind_group_id)
                            .and_then(|group| {
                                serde_json::from_str::<Value>(&group.descriptor_json).ok()
                            })
                            .and_then(|descriptor| {
                                descriptor.get("entries").and_then(Value::as_array).cloned()
                            })
                            .map(|entries| {
                                entries
                                    .iter()
                                    .filter_map(|entry| {
                                        let binding = entry.get("binding")?.as_u64()?;
                                        let resource = entry.get("resource")?;
                                        let buffer_id = resource.get("buffer")?.as_u64()? as u32;
                                        let preview =
                                            self.js_buffers.get(&buffer_id).map(|buffer| {
                                                buffer
                                                    .shadow
                                                    .chunks_exact(4)
                                                    .take(4)
                                                    .map(|bytes| {
                                                        f32::from_le_bytes([
                                                            bytes[0], bytes[1], bytes[2], bytes[3],
                                                        ])
                                                    })
                                                    .collect::<Vec<_>>()
                                            })?;
                                        Some(format!("binding{binding}:buf{buffer_id}:{preview:?}"))
                                    })
                                    .collect::<Vec<_>>()
                            }),
                        _ => None,
                    })
                    .flatten()
                    .collect::<Vec<_>>();
            } else if pass.descriptor.color_attachments.iter().any(|attachment| {
                self.js_texture_view_owners
                    .get(&attachment.view_id)
                    .and_then(|owner| self.js_textures.get(owner))
                    .is_some_and(|texture| matches!(texture, JsTextureResource::Owned(_)))
            }) {
                let _command_kinds = pass
                    .commands
                    .iter()
                    .map(|command| match command {
                        RenderCommand::SetPipeline { .. } => "setPipeline",
                        RenderCommand::SetBindGroup { .. } => "setBindGroup",
                        RenderCommand::SetIndexBuffer { .. } => "setIndexBuffer",
                        RenderCommand::SetVertexBuffer { .. } => "setVertexBuffer",
                        RenderCommand::SetViewport { .. } => "setViewport",
                        RenderCommand::SetScissorRect { .. } => "setScissorRect",
                        RenderCommand::SetStencilReference { .. } => "setStencilReference",
                        RenderCommand::ExecuteBundles { .. } => "executeBundles",
                        RenderCommand::Draw { .. } => "draw",
                        RenderCommand::DrawIndexed { .. } => "drawIndexed",
                    })
                    .collect::<Vec<_>>();
                let _draw_details = pass
                    .commands
                    .iter()
                    .filter_map(|command| match command {
                        RenderCommand::Draw {
                            vertex_count,
                            instance_count,
                            first_vertex,
                            first_instance,
                        } => Some(format!(
                            "draw(vc={vertex_count}, ic={instance_count}, fv={first_vertex}, fi={first_instance})"
                        )),
                        RenderCommand::DrawIndexed {
                            index_count,
                            instance_count,
                            first_index,
                            base_vertex,
                            first_instance,
                        } => Some(format!(
                            "drawIndexed(ic={index_count}, inst={instance_count}, fi={first_index}, bv={base_vertex}, firstInst={first_instance})"
                        )),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
            }
        }

        for command in &encoder.recorded_commands {
            match command {
                RecordedEncoderCommand::RenderPass(pass) => {
                    self.replay_render_pass(&mut encoder.encoder, pass)?;
                }
                RecordedEncoderCommand::CopyTextureToTexture {
                    source,
                    destination,
                    copy_size,
                } => {
                    let source_texture =
                        match self.js_textures.get(&source.texture).ok_or_else(|| {
                            anyhow!("unknown source texture handle {}", source.texture)
                        })? {
                            JsTextureResource::Owned(tex) => tex,
                            _ => bail!("unsupported surface texture source"),
                        };
                    let destination_texture =
                        match self.js_textures.get(&destination.texture).ok_or_else(|| {
                            anyhow!("unknown destination texture handle {}", destination.texture)
                        })? {
                            JsTextureResource::Owned(tex) => tex,
                            _ => bail!("unsupported surface texture destination"),
                        };

                    let aspect_fn = |a: &str| match a {
                        "stencil-only" => wgpu::TextureAspect::StencilOnly,
                        "depth-only" => wgpu::TextureAspect::DepthOnly,
                        _ => wgpu::TextureAspect::All,
                    };

                    encoder.encoder.copy_texture_to_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: source_texture,
                            mip_level: source.mip_level,
                            origin: wgpu::Origin3d {
                                x: source.origin[0],
                                y: source.origin[1],
                                z: source.origin[2],
                            },
                            aspect: aspect_fn(&source.aspect),
                        },
                        wgpu::TexelCopyTextureInfo {
                            texture: destination_texture,
                            mip_level: destination.mip_level,
                            origin: wgpu::Origin3d {
                                x: destination.origin[0],
                                y: destination.origin[1],
                                z: destination.origin[2],
                            },
                            aspect: aspect_fn(&destination.aspect),
                        },
                        wgpu::Extent3d {
                            width: copy_size[0],
                            height: copy_size[1],
                            depth_or_array_layers: copy_size[2],
                        },
                    );
                }
                RecordedEncoderCommand::CopyTextureToBuffer {
                    source,
                    destination,
                    copy_size,
                } => {
                    let source_texture =
                        match self.js_textures.get(&source.texture).ok_or_else(|| {
                            anyhow!("unknown source texture handle {}", source.texture)
                        })? {
                            JsTextureResource::Owned(tex) => tex,
                            _ => bail!("unsupported surface texture source"),
                        };
                    let destination_buffer =
                        self.js_buffers.get(&destination.buffer).ok_or_else(|| {
                            anyhow!("unknown destination buffer handle {}", destination.buffer)
                        })?;

                    let aspect_fn = |a: &str| match a {
                        "stencil-only" => wgpu::TextureAspect::StencilOnly,
                        "depth-only" => wgpu::TextureAspect::DepthOnly,
                        _ => wgpu::TextureAspect::All,
                    };

                    encoder.encoder.copy_texture_to_buffer(
                        wgpu::TexelCopyTextureInfo {
                            texture: source_texture,
                            mip_level: source.mip_level,
                            origin: wgpu::Origin3d {
                                x: source.origin[0],
                                y: source.origin[1],
                                z: source.origin[2],
                            },
                            aspect: aspect_fn(&source.aspect),
                        },
                        wgpu::TexelCopyBufferInfo {
                            buffer: &destination_buffer.buffer,
                            layout: wgpu::TexelCopyBufferLayout {
                                offset: destination.offset,
                                bytes_per_row: destination.bytes_per_row,
                                rows_per_image: destination.rows_per_image,
                            },
                        },
                        wgpu::Extent3d {
                            width: copy_size[0],
                            height: copy_size[1],
                            depth_or_array_layers: copy_size[2],
                        },
                    );
                }
                RecordedEncoderCommand::ComputePass(pass) => {
                    let mut cmd_strings = Vec::new();

                    let mut compute_pass =
                        encoder
                            .encoder
                            .begin_compute_pass(&wgpu::ComputePassDescriptor {
                                label: pass.label.as_deref(),
                                timestamp_writes: None,
                            });

                    for pass_cmd in &pass.commands {
                        cmd_strings.push(format!("{:?}", pass_cmd));
                        match pass_cmd {
                            ComputeCommand::SetPipeline { pipeline_id } => {
                                let pipeline = self.js_compute_pipelines.get(pipeline_id).unwrap();
                                compute_pass.set_pipeline(&pipeline.pipeline);
                            }
                            ComputeCommand::SetBindGroup {
                                index,
                                bind_group_id,
                                dynamic_offsets,
                            } => {
                                let bg = self.js_bind_groups.get(bind_group_id).unwrap();
                                compute_pass.set_bind_group(
                                    *index,
                                    &bg.group,
                                    dynamic_offsets.as_slice(),
                                );
                            }
                            ComputeCommand::DispatchWorkgroups { x, y, z } => {
                                compute_pass.dispatch_workgroups(*x, *y, *z);
                            }
                            ComputeCommand::DispatchWorkgroupsIndirect { buffer_id, offset } => {
                                let buffer = self.js_buffers.get(buffer_id).unwrap();
                                compute_pass.dispatch_workgroups_indirect(&buffer.buffer, *offset);
                            }
                        }
                    }
                    println!("compute pass ops: commands=[{}]", cmd_strings.join(", "));
                }
            }
        }

        let command_buffer = encoder.encoder.finish();

        let id = self.next_js_id();
        self.js_command_buffers.insert(
            id,
            JsCommandBuffer {
                command_buffer: Some(command_buffer),
            },
        );
        Ok(id)
    }

    pub(crate) fn js_queue_submit(
        &mut self,
        _queue_id: u32,
        command_buffers_json: &str,
    ) -> Result<()> {
        let ids: Vec<u32> = serde_json::from_str(command_buffers_json)?;

        let mut command_buffers = Vec::with_capacity(ids.len());

        for id in ids {
            let command_buffer = self
                .js_command_buffers
                .get_mut(&id)
                .and_then(|command_buffer| command_buffer.command_buffer.take())
                .ok_or_else(|| anyhow!("unknown command buffer handle {id}"))?;
            command_buffers.push(command_buffer);
        }

        self.queue.submit(command_buffers);
        Ok(())
    }

    pub(crate) fn present_submitted_surface_textures(&mut self) {
        self.js_current_surface_texture = None;

        let surface_texture_ids = self
            .js_textures
            .iter()
            .filter_map(|(id, texture)| match texture {
                JsTextureResource::Surface(_) => Some(*id),
                JsTextureResource::Owned(_) => None,
            })
            .collect::<Vec<_>>();

        for texture_id in &surface_texture_ids {
            if let Some(JsTextureResource::Surface(surface_texture)) =
                self.js_textures.remove(texture_id)
            {
                let view = surface_texture.texture.create_view(&Default::default());
                if let Some(ui) = self.ui.as_mut() {
                    ui.draw(
                        &self.device,
                        &self.queue,
                        &view,
                        self.size.width as f32,
                        self.size.height as f32,
                        self.mouse,
                    );
                }
                surface_texture.present();
            }
        }

        let surface_view_ids = self
            .js_texture_view_owners
            .iter()
            .filter_map(|(view_id, owner_id)| {
                surface_texture_ids.contains(owner_id).then_some(*view_id)
            })
            .collect::<Vec<_>>();

        for view_id in surface_view_ids {
            self.js_texture_view_owners.remove(&view_id);
            self.js_texture_views.remove(&view_id);
        }
    }

    fn replay_render_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pass: &RecordedRenderPass,
    ) -> Result<()> {
        let color_attachment_storage = pass
            .descriptor
            .color_attachments
            .iter()
            .map(|attachment| {
                let view = self
                    .js_texture_views
                    .get(&attachment.view_id)
                    .ok_or_else(|| anyhow!("unknown texture view handle {}", attachment.view_id))?;
                let resolve_target =
                    match attachment.resolve_target_id {
                        Some(resolve_target_id) => {
                            Some(self.js_texture_views.get(&resolve_target_id).ok_or_else(
                                || anyhow!("unknown texture view handle {resolve_target_id}"),
                            )?)
                        }
                        None => None,
                    };

                let is_surface_target = self
                    .js_texture_view_owners
                    .get(&attachment.view_id)
                    .and_then(|owner| self.js_textures.get(owner))
                    .is_some_and(|texture| matches!(texture, JsTextureResource::Surface(_)));
                let load =
                    parse_load_op_color(attachment.load_op.as_deref(), attachment.clear_value)?;
                let load = if is_surface_target
                    && attachment.clear_value.is_none()
                    && matches!(attachment.load_op.as_deref(), Some("load") | Some("clear"))
                {
                    fallback_clear_load_op_color(attachment.clear_value)
                } else {
                    load
                };

                Ok(Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target,
                    ops: wgpu::Operations {
                        load,
                        store: parse_store_op(attachment.store_op.as_deref())?,
                    },
                }))
            })
            .collect::<Result<Vec<_>>>()?;

        let depth_stencil_attachment_storage = match &pass.descriptor.depth_stencil_attachment {
            Some(attachment) => {
                let view = self
                    .js_texture_views
                    .get(&attachment.view_id)
                    .ok_or_else(|| anyhow!("unknown texture view handle {}", attachment.view_id))?;
                let is_surface_target = self
                    .js_texture_view_owners
                    .get(&attachment.view_id)
                    .and_then(|owner| self.js_textures.get(owner))
                    .is_some_and(|texture| matches!(texture, JsTextureResource::Surface(_)));
                let depth_load = parse_load_op_depth(
                    attachment.depth_load_op.as_deref(),
                    attachment.depth_clear_value,
                )?;
                let depth_load = if is_surface_target
                    && attachment.depth_load_op.as_deref() == Some("load")
                    && attachment.depth_clear_value.is_none()
                {
                    fallback_clear_load_op_depth(attachment.depth_clear_value)
                } else {
                    depth_load
                };
                Some(wgpu::RenderPassDepthStencilAttachment {
                    view,
                    depth_ops: Some(wgpu::Operations {
                        load: depth_load,
                        store: parse_store_op(attachment.depth_store_op.as_deref())?,
                    }),
                    stencil_ops: None,
                })
            }
            None => None,
        };

        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("js render pass"),
            color_attachments: &color_attachment_storage,
            depth_stencil_attachment: depth_stencil_attachment_storage,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        for command in &pass.commands {
            match command {
                RenderCommand::ExecuteBundles { bundle_ids } => {
                    for bundle_id in bundle_ids {
                        let bundle = self
                            .js_render_bundles
                            .get(bundle_id)
                            .ok_or_else(|| anyhow!("unknown render bundle handle {bundle_id}"))?;
                        for bundle_command in &bundle.commands {
                            self.apply_render_command(&mut render_pass, bundle_command)?;
                        }
                    }
                }
                other => self.apply_render_command(&mut render_pass, other)?,
            }
        }

        Ok(())
    }

    fn apply_render_command<'a>(
        &'a self,
        render_pass: &mut wgpu::RenderPass<'a>,
        command: &'a RenderCommand,
    ) -> Result<()> {
        match command {
            RenderCommand::SetPipeline { pipeline_id } => {
                let pipeline = &self
                    .js_render_pipelines
                    .get(pipeline_id)
                    .ok_or_else(|| anyhow!("unknown render pipeline handle {pipeline_id}"))?
                    .pipeline;
                render_pass.set_pipeline(pipeline);
            }
            RenderCommand::SetBindGroup {
                index,
                bind_group_id,
            } => {
                let bind_group = &self
                    .js_bind_groups
                    .get(bind_group_id)
                    .ok_or_else(|| anyhow!("unknown bind group handle {bind_group_id}"))?
                    .group;
                render_pass.set_bind_group(*index, bind_group, &[]);
            }
            RenderCommand::SetIndexBuffer { buffer_id, format } => {
                let buffer = &self
                    .js_buffers
                    .get(buffer_id)
                    .ok_or_else(|| anyhow!("unknown buffer handle {buffer_id}"))?
                    .buffer;
                render_pass.set_index_buffer(buffer.slice(..), parse_index_format(format)?);
            }
            RenderCommand::SetVertexBuffer { slot, buffer_id } => {
                let buffer = &self
                    .js_buffers
                    .get(buffer_id)
                    .ok_or_else(|| anyhow!("unknown buffer handle {buffer_id}"))?
                    .buffer;
                render_pass.set_vertex_buffer(*slot, buffer.slice(..));
            }
            RenderCommand::SetViewport {
                x,
                y,
                width,
                height,
                min_depth,
                max_depth,
            } => {
                render_pass.set_viewport(*x, *y, *width, *height, *min_depth, *max_depth);
            }
            RenderCommand::SetScissorRect {
                x,
                y,
                width,
                height,
            } => {
                render_pass.set_scissor_rect(*x, *y, *width, *height);
            }
            RenderCommand::SetStencilReference { reference } => {
                render_pass.set_stencil_reference(*reference);
            }
            RenderCommand::Draw {
                vertex_count,
                instance_count,
                first_vertex,
                first_instance,
            } => {
                render_pass.draw(
                    *first_vertex..(*first_vertex + *vertex_count),
                    *first_instance..(*first_instance + *instance_count),
                );
            }
            RenderCommand::DrawIndexed {
                index_count,
                instance_count,
                first_index,
                base_vertex,
                first_instance,
            } => {
                render_pass.draw_indexed(
                    *first_index..(*first_index + *index_count),
                    *base_vertex,
                    *first_instance..(*first_instance + *instance_count),
                );
            }
            RenderCommand::ExecuteBundles { .. } => {}
        }

        Ok(())
    }

    pub(crate) fn js_queue_write_buffer(
        &mut self,
        _queue_id: u32,
        buffer_id: u32,
        buffer_offset: u64,
        bytes: &[u8],
    ) -> Result<()> {
        let buffer = self
            .js_buffers
            .get(&buffer_id)
            .ok_or_else(|| anyhow!("unknown buffer handle {buffer_id}"))?;
        let raw_buffer = buffer.buffer.clone();
        if bytes.is_empty() {
            return Ok(());
        }

        if let Some(target) = self.js_buffers.get_mut(&buffer_id) {
            let start = buffer_offset as usize;
            let end = start + bytes.len();
            if end <= target.shadow.len() {
                target.shadow[start..end].copy_from_slice(bytes);
            }
        }

        let mut padded = bytes.to_vec();
        let aligned_len = padded
            .len()
            .next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT as usize);
        padded.resize(aligned_len, 0);

        self.queue.write_buffer(&raw_buffer, buffer_offset, &padded);
        Ok(())
    }

    pub(crate) fn js_queue_write_texture(
        &mut self,
        _queue_id: u32,
        descriptor_json: &str,
    ) -> Result<()> {
        let descriptor: Value = serde_json::from_str(descriptor_json)?;
        let destination = descriptor
            .get("destination")
            .ok_or_else(|| anyhow!("writeTexture missing destination"))?;
        let texture_id = destination
            .get("texture")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("writeTexture destination missing texture"))?
            as u32;
        let texture = match self
            .js_textures
            .get(&texture_id)
            .ok_or_else(|| anyhow!("unknown texture handle {texture_id}"))?
        {
            JsTextureResource::Owned(texture) => texture,
            JsTextureResource::Surface(_) => {
                bail!("writeTexture to surface textures is unsupported")
            }
        };

        let data = descriptor
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("writeTexture missing data"))?
            .iter()
            .map(|value| {
                value
                    .as_u64()
                    .ok_or_else(|| anyhow!("writeTexture data must be byte values"))
                    .and_then(|value| value.try_into().map_err(anyhow::Error::from))
            })
            .collect::<Result<Vec<u8>>>()?;
        let data_layout = descriptor
            .get("dataLayout")
            .ok_or_else(|| anyhow!("writeTexture missing dataLayout"))?;
        let size = texture_size_from_json(descriptor.get("size"))?;

        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: destination
                    .get("mipLevel")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32,
                origin: parse_origin_3d(destination.get("origin"))?,
                aspect: parse_texture_aspect(
                    destination
                        .get("aspect")
                        .and_then(Value::as_str)
                        .unwrap_or("all"),
                )?,
            },
            &data,
            wgpu::TexelCopyBufferLayout {
                offset: data_layout
                    .get("offset")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                bytes_per_row: data_layout
                    .get("bytesPerRow")
                    .and_then(Value::as_u64)
                    .map(|value| value as u32),
                rows_per_image: data_layout
                    .get("rowsPerImage")
                    .and_then(Value::as_u64)
                    .map(|value| value as u32),
            },
            size,
        );

        Ok(())
    }

    pub(crate) fn js_load_image(&mut self, path: &Path) -> Result<u32> {
        let image = image::open(path)?.to_rgba8();
        self.insert_js_image(image)
    }

    pub(crate) fn js_load_image_bytes(&mut self, bytes: &[u8]) -> Result<u32> {
        let image = image::load_from_memory(bytes)?.to_rgba8();
        self.insert_js_image(image)
    }

    fn insert_js_image(&mut self, image: image::RgbaImage) -> Result<u32> {
        let (width, height) = image.dimensions();

        let id = self.next_js_id();
        self.js_images.insert(
            id,
            JsImage {
                width,
                height,
                rgba8: image.into_raw(),
            },
        );
        Ok(id)
    }

    pub(crate) fn js_get_image_size(&self, image_id: u32) -> Result<(u32, u32)> {
        let image = self
            .js_images
            .get(&image_id)
            .ok_or_else(|| anyhow!("unknown image handle {image_id}"))?;
        Ok((image.width, image.height))
    }

    pub(crate) fn js_queue_copy_external_image_to_texture(
        &mut self,
        _queue_id: u32,
        descriptor_json: &str,
    ) -> Result<()> {
        let descriptor: Value = serde_json::from_str(descriptor_json)?;
        let source = descriptor
            .get("source")
            .ok_or_else(|| anyhow!("copyExternalImageToTexture missing source"))?;
        let source_id = source
            .get("source")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("copyExternalImageToTexture missing source image"))?
            as u32;
        let image = self
            .js_images
            .get(&source_id)
            .ok_or_else(|| anyhow!("unknown image handle {source_id}"))?;

        let destination = descriptor
            .get("destination")
            .ok_or_else(|| anyhow!("copyExternalImageToTexture missing destination"))?;
        let texture_id = destination
            .get("texture")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("copyExternalImageToTexture destination missing texture"))?
            as u32;
        let texture = match self
            .js_textures
            .get(&texture_id)
            .ok_or_else(|| anyhow!("unknown texture handle {texture_id}"))?
        {
            JsTextureResource::Owned(texture) => texture,
            JsTextureResource::Surface(_) => {
                bail!("copyExternalImageToTexture to surface textures is unsupported")
            }
        };

        let flip_y = source
            .get("flipY")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let data = if flip_y {
            flip_rgba_rows(&image.rgba8, image.width, image.height)
        } else {
            image.rgba8.clone()
        };

        let size = texture_size_from_json(descriptor.get("size"))?;
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: destination
                    .get("mipLevel")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32,
                origin: parse_origin_3d(destination.get("origin"))?,
                aspect: parse_texture_aspect(
                    destination
                        .get("aspect")
                        .and_then(Value::as_str)
                        .unwrap_or("all"),
                )?,
            },
            &data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(image.width * 4),
                rows_per_image: Some(image.height),
            },
            size,
        );

        Ok(())
    }

    pub(crate) fn js_render_pass_set_pipeline(
        &mut self,
        render_pass_id: u32,
        pipeline_id: u32,
    ) -> Result<()> {
        let render_pass = self
            .js_render_passes
            .get_mut(&render_pass_id)
            .ok_or_else(|| anyhow!("unknown render pass handle {render_pass_id}"))?;
        render_pass
            .commands
            .push(RenderCommand::SetPipeline { pipeline_id });
        Ok(())
    }

    pub(crate) fn js_render_bundle_set_pipeline(
        &mut self,
        bundle_id: u32,
        pipeline_id: u32,
    ) -> Result<()> {
        let bundle = self
            .js_render_bundle_encoders
            .get_mut(&bundle_id)
            .ok_or_else(|| anyhow!("unknown render bundle encoder handle {bundle_id}"))?;
        bundle
            .commands
            .push(RenderCommand::SetPipeline { pipeline_id });
        Ok(())
    }

    pub(crate) fn js_render_pass_set_bind_group(
        &mut self,
        render_pass_id: u32,
        index: u32,
        bind_group_id: u32,
    ) -> Result<()> {
        let render_pass = self
            .js_render_passes
            .get_mut(&render_pass_id)
            .ok_or_else(|| anyhow!("unknown render pass handle {render_pass_id}"))?;
        render_pass.commands.push(RenderCommand::SetBindGroup {
            index,
            bind_group_id,
        });
        Ok(())
    }

    pub(crate) fn js_render_bundle_set_bind_group(
        &mut self,
        bundle_id: u32,
        index: u32,
        bind_group_id: u32,
    ) -> Result<()> {
        let bundle = self
            .js_render_bundle_encoders
            .get_mut(&bundle_id)
            .ok_or_else(|| anyhow!("unknown render bundle encoder handle {bundle_id}"))?;
        bundle.commands.push(RenderCommand::SetBindGroup {
            index,
            bind_group_id,
        });
        Ok(())
    }

    pub(crate) fn js_render_pass_set_index_buffer(
        &mut self,
        render_pass_id: u32,
        buffer_id: u32,
        format: &str,
    ) -> Result<()> {
        let render_pass = self
            .js_render_passes
            .get_mut(&render_pass_id)
            .ok_or_else(|| anyhow!("unknown render pass handle {render_pass_id}"))?;
        render_pass.commands.push(RenderCommand::SetIndexBuffer {
            buffer_id,
            format: format.to_string(),
        });
        Ok(())
    }

    pub(crate) fn js_render_pass_set_vertex_buffer(
        &mut self,
        render_pass_id: u32,
        slot: u32,
        buffer_id: u32,
    ) -> Result<()> {
        let render_pass = self
            .js_render_passes
            .get_mut(&render_pass_id)
            .ok_or_else(|| anyhow!("unknown render pass handle {render_pass_id}"))?;
        render_pass
            .commands
            .push(RenderCommand::SetVertexBuffer { slot, buffer_id });
        Ok(())
    }

    pub(crate) fn js_render_pass_set_viewport(
        &mut self,
        render_pass_id: u32,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        min_depth: f32,
        max_depth: f32,
    ) -> Result<()> {
        let render_pass = self
            .js_render_passes
            .get_mut(&render_pass_id)
            .ok_or_else(|| anyhow!("unknown render pass handle {render_pass_id}"))?;
        render_pass.commands.push(RenderCommand::SetViewport {
            x,
            y,
            width,
            height,
            min_depth,
            max_depth,
        });
        Ok(())
    }

    pub(crate) fn js_render_pass_set_scissor_rect(
        &mut self,
        render_pass_id: u32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<()> {
        let render_pass = self
            .js_render_passes
            .get_mut(&render_pass_id)
            .ok_or_else(|| anyhow!("unknown render pass handle {render_pass_id}"))?;
        render_pass.commands.push(RenderCommand::SetScissorRect {
            x,
            y,
            width,
            height,
        });
        Ok(())
    }

    pub(crate) fn js_render_pass_set_stencil_reference(
        &mut self,
        render_pass_id: u32,
        reference: u32,
    ) -> Result<()> {
        let render_pass = self
            .js_render_passes
            .get_mut(&render_pass_id)
            .ok_or_else(|| anyhow!("unknown render pass handle {render_pass_id}"))?;
        render_pass
            .commands
            .push(RenderCommand::SetStencilReference { reference });
        Ok(())
    }

    pub(crate) fn js_render_pass_draw(
        &mut self,
        render_pass_id: u32,
        vertex_count: u32,
        instance_count: u32,
        first_vertex: u32,
        first_instance: u32,
    ) -> Result<()> {
        let render_pass = self
            .js_render_passes
            .get_mut(&render_pass_id)
            .ok_or_else(|| anyhow!("unknown render pass handle {render_pass_id}"))?;
        render_pass.commands.push(RenderCommand::Draw {
            vertex_count,
            instance_count,
            first_vertex,
            first_instance,
        });
        Ok(())
    }

    pub(crate) fn js_render_bundle_draw(
        &mut self,
        bundle_id: u32,
        vertex_count: u32,
        instance_count: u32,
        first_vertex: u32,
        first_instance: u32,
    ) -> Result<()> {
        let bundle = self
            .js_render_bundle_encoders
            .get_mut(&bundle_id)
            .ok_or_else(|| anyhow!("unknown render bundle encoder handle {bundle_id}"))?;
        bundle.commands.push(RenderCommand::Draw {
            vertex_count,
            instance_count,
            first_vertex,
            first_instance,
        });
        Ok(())
    }

    pub(crate) fn js_render_pass_draw_indexed(
        &mut self,
        render_pass_id: u32,
        index_count: u32,
        instance_count: u32,
        first_index: u32,
        base_vertex: i32,
        first_instance: u32,
    ) -> Result<()> {
        let render_pass = self
            .js_render_passes
            .get_mut(&render_pass_id)
            .ok_or_else(|| anyhow!("unknown render pass handle {render_pass_id}"))?;
        render_pass.commands.push(RenderCommand::DrawIndexed {
            index_count,
            instance_count,
            first_index,
            base_vertex,
            first_instance,
        });
        Ok(())
    }

    pub(crate) fn js_render_bundle_finish(&mut self, bundle_id: u32) -> Result<u32> {
        let bundle = self
            .js_render_bundle_encoders
            .remove(&bundle_id)
            .ok_or_else(|| anyhow!("unknown render bundle encoder handle {bundle_id}"))?;

        let id = self.next_js_id();
        self.js_render_bundles.insert(
            id,
            JsRenderBundle {
                commands: bundle.commands,
            },
        );
        Ok(id)
    }

    pub(crate) fn js_render_pass_execute_bundles(
        &mut self,
        render_pass_id: u32,
        bundle_ids_json: &str,
    ) -> Result<()> {
        let bundle_ids: Vec<u32> = serde_json::from_str(bundle_ids_json)?;
        let render_pass = self
            .js_render_passes
            .get_mut(&render_pass_id)
            .ok_or_else(|| anyhow!("unknown render pass handle {render_pass_id}"))?;
        render_pass
            .commands
            .push(RenderCommand::ExecuteBundles { bundle_ids });
        Ok(())
    }

    pub(crate) fn js_create_bind_group_layout(
        &mut self,
        _device_id: u32,
        descriptor_json: &str,
    ) -> Result<u32> {
        let descriptor: Value = serde_json::from_str(descriptor_json)?;
        let entries = descriptor
            .get("entries")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("bind group layout descriptor missing entries"))?;

        let mut native_entries = Vec::with_capacity(entries.len());
        for entry in entries {
            let binding = entry
                .get("binding")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow!("bind group layout entry missing binding"))?
                as u32;
            let visibility = wgpu::ShaderStages::from_bits_truncate(
                entry
                    .get("visibility")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| anyhow!("bind group layout entry missing visibility"))?
                    .try_into()?,
            );

            let ty = if let Some(buffer) = entry.get("buffer") {
                let buffer_type = match buffer
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("uniform")
                {
                    "uniform" => wgpu::BufferBindingType::Uniform,
                    "storage" => wgpu::BufferBindingType::Storage { read_only: false },
                    "read-only-storage" => wgpu::BufferBindingType::Storage { read_only: true },
                    other => bail!("unsupported buffer binding type {other}"),
                };

                wgpu::BindingType::Buffer {
                    ty: buffer_type,
                    has_dynamic_offset: buffer
                        .get("hasDynamicOffset")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    min_binding_size: None,
                }
            } else if let Some(sampler) = entry.get("sampler") {
                let sampler_type = match sampler
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("filtering")
                {
                    "filtering" => wgpu::SamplerBindingType::Filtering,
                    "non-filtering" => wgpu::SamplerBindingType::NonFiltering,
                    "comparison" => wgpu::SamplerBindingType::Comparison,
                    other => bail!("unsupported sampler binding type {other}"),
                };

                wgpu::BindingType::Sampler(sampler_type)
            } else if let Some(texture) = entry.get("texture") {
                let sample_type = match texture
                    .get("sampleType")
                    .and_then(Value::as_str)
                    .unwrap_or("float")
                {
                    "float" => wgpu::TextureSampleType::Float { filterable: true },
                    "unfilterable-float" => wgpu::TextureSampleType::Float { filterable: false },
                    "depth" => wgpu::TextureSampleType::Depth,
                    "sint" => wgpu::TextureSampleType::Sint,
                    "uint" => wgpu::TextureSampleType::Uint,
                    other => bail!("unsupported texture sample type {other}"),
                };
                let view_dimension = parse_texture_view_dimension(
                    texture
                        .get("viewDimension")
                        .and_then(Value::as_str)
                        .unwrap_or("2d"),
                )?;
                let multisampled = texture
                    .get("multisampled")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);

                wgpu::BindingType::Texture {
                    sample_type,
                    view_dimension,
                    multisampled,
                }
            } else {
                bail!("unsupported bind group layout entry kind");
            };

            native_entries.push(wgpu::BindGroupLayoutEntry {
                binding,
                visibility,
                ty,
                count: None,
            });
        }

        let layout = self
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: descriptor.get("label").and_then(Value::as_str),
                entries: &native_entries,
            });

        let id = self.next_js_id();
        self.js_bind_group_layouts
            .insert(id, JsBindGroupLayout { layout });
        Ok(id)
    }

    pub(crate) fn js_create_pipeline_layout(
        &mut self,
        _device_id: u32,
        descriptor_json: &str,
    ) -> Result<u32> {
        let descriptor: Value = serde_json::from_str(descriptor_json)?;
        let layout_ids = descriptor
            .get("bindGroupLayouts")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("pipeline layout descriptor missing bindGroupLayouts"))?;

        let bind_group_layouts = layout_ids
            .iter()
            .map(|value| {
                let id = value
                    .as_u64()
                    .ok_or_else(|| anyhow!("bindGroupLayouts entry must be numeric"))?
                    as u32;
                self.js_bind_group_layouts
                    .get(&id)
                    .map(|layout| Some(&layout.layout))
                    .ok_or_else(|| anyhow!("unknown bind group layout handle {id}"))
            })
            .collect::<Result<Vec<_>>>()?;

        let layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: descriptor.get("label").and_then(Value::as_str),
                bind_group_layouts: &bind_group_layouts,
                immediate_size: 0,
            });

        let id = self.next_js_id();
        self.js_pipeline_layouts
            .insert(id, JsPipelineLayout { layout });
        Ok(id)
    }

    pub(crate) fn js_create_bind_group(
        &mut self,
        _device_id: u32,
        descriptor_json: &str,
    ) -> Result<u32> {
        let descriptor: Value = serde_json::from_str(descriptor_json)?;
        let layout_id = descriptor
            .get("layout")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("bind group descriptor missing layout"))?
            as u32;
        let layout = &self
            .js_bind_group_layouts
            .get(&layout_id)
            .ok_or_else(|| anyhow!("unknown bind group layout handle {layout_id}"))?
            .layout;

        let entries = descriptor
            .get("entries")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("bind group descriptor missing entries"))?;

        let mut buffer_resources = Vec::with_capacity(entries.len());
        let mut native_entries = Vec::with_capacity(entries.len());
        for entry in entries {
            let binding = entry
                .get("binding")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow!("bind group entry missing binding"))?
                as u32;
            let resource = entry
                .get("resource")
                .ok_or_else(|| anyhow!("bind group entry missing resource"))?;
            let kind = resource
                .get("kind")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("bind group resource missing kind"))?;

            let binding_resource = match kind {
                "buffer" => {
                    let buffer_id = resource
                        .get("buffer")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| anyhow!("bind group buffer resource missing buffer"))?
                        as u32;
                    let buffer = &self
                        .js_buffers
                        .get(&buffer_id)
                        .ok_or_else(|| anyhow!("unknown buffer handle {buffer_id}"))?
                        .buffer;
                    let offset = resource.get("offset").and_then(Value::as_u64).unwrap_or(0);
                    let size = resource.get("size").and_then(Value::as_u64);

                    buffer_resources.push(wgpu::BufferBinding {
                        buffer,
                        offset,
                        size: size.and_then(wgpu::BufferSize::new),
                    });
                    wgpu::BindingResource::Buffer(buffer_resources.last().unwrap().clone())
                }
                "sampler" => {
                    let sampler_id = resource
                        .get("sampler")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| anyhow!("bind group sampler resource missing sampler"))?
                        as u32;
                    let sampler = &self
                        .js_samplers
                        .get(&sampler_id)
                        .ok_or_else(|| anyhow!("unknown sampler handle {sampler_id}"))?
                        .sampler;
                    wgpu::BindingResource::Sampler(sampler)
                }
                "textureView" => {
                    let texture_view_id = resource
                        .get("textureView")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| {
                            anyhow!("bind group texture view resource missing textureView")
                        })? as u32;
                    let texture_view = self
                        .js_texture_views
                        .get(&texture_view_id)
                        .ok_or_else(|| anyhow!("unknown texture view handle {texture_view_id}"))?;
                    wgpu::BindingResource::TextureView(texture_view)
                }
                other => bail!("unsupported bind group resource kind {other}"),
            };

            native_entries.push(wgpu::BindGroupEntry {
                binding,
                resource: binding_resource,
            });
        }

        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: descriptor.get("label").and_then(Value::as_str),
            layout,
            entries: &native_entries,
        });

        let id = self.next_js_id();
        self.js_bind_groups.insert(
            id,
            JsBindGroup {
                group,
                descriptor_json: descriptor_json.to_string(),
            },
        );
        Ok(id)
    }

    pub(crate) fn js_create_shader_module(
        &mut self,
        _device_id: u32,
        descriptor_json: &str,
    ) -> Result<u32> {
        let descriptor: Value = serde_json::from_str(descriptor_json)?;
        let code = descriptor
            .get("code")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("shader module descriptor missing code"))?;

        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: descriptor.get("label").and_then(Value::as_str),
                source: wgpu::ShaderSource::Wgsl(Cow::Owned(code.to_string())),
            });

        let id = self.next_js_id();
        self.js_shader_modules.insert(id, JsShaderModule { module });
        Ok(id)
    }

    pub(crate) fn js_create_compute_pipeline(
        &mut self,
        _device_id: u32,
        descriptor_json: &str,
    ) -> Result<u32> {
        let descriptor: ComputePipelineDescriptorData = serde_json::from_str(descriptor_json)?;

        let mut entry_point = Some("main");
        if let Some(entry) = &descriptor.compute.entry_point {
            entry_point = Some(entry.as_str());
        }

        let module = self
            .js_shader_modules
            .get(&descriptor.compute.module)
            .ok_or_else(|| anyhow!("unknown shader module"))?;

        let bind_group_layouts = HashMap::new();
        let pipeline_layout = if let Some(layout_id) = descriptor.layout {
            let layout = self.js_pipeline_layouts.get(&layout_id).unwrap();
            Some(&layout.layout)
        } else {
            None
        };

        let wgpu_descriptor = wgpu::ComputePipelineDescriptor {
            label: descriptor.label.as_deref(),
            layout: pipeline_layout,
            module: &module.module,
            entry_point,
            compilation_options: Default::default(),
            cache: None,
        };

        let pipeline = self.device.create_compute_pipeline(&wgpu_descriptor);
        let id = self.js_next_id;
        self.js_next_id += 1;

        self.js_compute_pipelines.insert(
            id,
            JsComputePipeline {
                pipeline,
                bind_group_layouts,
                _label: descriptor.label,
            },
        );

        Ok(id)
    }

    pub(crate) fn js_compute_pipeline_get_bind_group_layout(
        &mut self,
        pipeline_id: u32,
        index: u32,
    ) -> Result<Value> {
        if let Some(id) = self
            .js_compute_pipelines
            .get(&pipeline_id)
            .unwrap()
            .bind_group_layouts
            .get(&index)
        {
            return Ok(serde_json::json!({ "handle": *id }));
        }

        let layout = {
            let pipeline = self.js_compute_pipelines.get(&pipeline_id).unwrap();
            pipeline.pipeline.get_bind_group_layout(index)
        };

        let id = self.js_next_id;
        self.js_next_id += 1;
        self.js_bind_group_layouts
            .insert(id, JsBindGroupLayout { layout });

        self.js_compute_pipelines
            .get_mut(&pipeline_id)
            .unwrap()
            .bind_group_layouts
            .insert(index, id);

        Ok(serde_json::json!({ "handle": id }))
    }

    pub(crate) fn js_command_encoder_begin_compute_pass(
        &mut self,
        encoder_id: u32,
        descriptor_json: &str,
    ) -> Result<u32> {
        let label = if descriptor_json != "null" {
            let descriptor: Value = serde_json::from_str(descriptor_json)?;
            descriptor
                .get("label")
                .and_then(|v| v.as_str().map(|s| s.to_string()))
        } else {
            None
        };

        let id = self.js_next_id;
        self.js_next_id += 1;

        self.js_compute_passes.insert(
            id,
            JsComputePass {
                encoder_id,
                label,
                commands: Vec::new(),
            },
        );

        Ok(id)
    }

    pub(crate) fn js_compute_pass_encoder_set_pipeline(
        &mut self,
        pass_id: u32,
        pipeline_id: u32,
    ) -> Result<()> {
        let pass = self.js_compute_passes.get_mut(&pass_id).unwrap();
        pass.commands
            .push(ComputeCommand::SetPipeline { pipeline_id });
        Ok(())
    }

    pub(crate) fn js_compute_pass_encoder_set_bind_group(
        &mut self,
        pass_id: u32,
        index: u32,
        bind_group_id: u32,
        dynamic_offsets: Vec<u32>,
    ) -> Result<()> {
        let pass = self.js_compute_passes.get_mut(&pass_id).unwrap();
        pass.commands.push(ComputeCommand::SetBindGroup {
            index,
            bind_group_id,
            dynamic_offsets,
        });
        Ok(())
    }

    pub(crate) fn js_compute_pass_encoder_dispatch_workgroups(
        &mut self,
        pass_id: u32,
        x: u32,
        y: u32,
        z: u32,
    ) -> Result<()> {
        let pass = self.js_compute_passes.get_mut(&pass_id).unwrap();
        pass.commands
            .push(ComputeCommand::DispatchWorkgroups { x, y, z });
        Ok(())
    }

    pub(crate) fn js_compute_pass_encoder_dispatch_workgroups_indirect(
        &mut self,
        pass_id: u32,
        buffer_id: u32,
        offset: u64,
    ) -> Result<()> {
        let pass = self.js_compute_passes.get_mut(&pass_id).unwrap();
        pass.commands
            .push(ComputeCommand::DispatchWorkgroupsIndirect { buffer_id, offset });
        Ok(())
    }

    pub(crate) fn js_compute_pass_encoder_end(&mut self, pass_id: u32) -> Result<()> {
        let pass = self.js_compute_passes.remove(&pass_id).unwrap();
        let encoder = self.js_command_encoders.get_mut(&pass.encoder_id).unwrap();
        encoder
            .recorded_commands
            .push(RecordedEncoderCommand::ComputePass(RecordedComputePass {
                label: pass.label,
                commands: pass.commands,
            }));
        Ok(())
    }

    pub(crate) fn js_create_render_pipeline(
        &mut self,
        _device_id: u32,
        descriptor_json: &str,
    ) -> Result<u32> {
        let descriptor: Value = serde_json::from_str(descriptor_json)?;

        let layout_value = descriptor
            .get("layout")
            .ok_or_else(|| anyhow!("render pipeline descriptor missing layout"))?;
        let layout = match layout_value {
            Value::String(layout) if layout == "auto" => None,
            Value::Null => None,
            Value::Number(layout_id) => {
                let layout_id = layout_id
                    .as_u64()
                    .ok_or_else(|| anyhow!("pipeline layout handle must be numeric"))?
                    as u32;
                Some(
                    &self
                        .js_pipeline_layouts
                        .get(&layout_id)
                        .ok_or_else(|| anyhow!("unknown pipeline layout handle {layout_id}"))?
                        .layout,
                )
            }
            _ => bail!("unsupported render pipeline layout descriptor"),
        };

        let vertex = descriptor
            .get("vertex")
            .ok_or_else(|| anyhow!("render pipeline descriptor missing vertex state"))?;
        let vertex_module_id = vertex
            .get("module")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("vertex state missing module"))?
            as u32;
        let vertex_module = &self
            .js_shader_modules
            .get(&vertex_module_id)
            .ok_or_else(|| anyhow!("unknown shader module handle {vertex_module_id}"))?
            .module;
        let vertex_entry_point = vertex
            .get("entryPoint")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let vertex_buffers_desc = vertex
            .get("buffers")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let vertex_buffer_layouts = vertex_buffers_desc
            .iter()
            .map(parse_vertex_buffer_layout)
            .collect::<Result<Vec<_>>>()?;
        let vertex_attributes_storage = vertex_buffer_layouts
            .iter()
            .map(|layout| layout.attributes.clone())
            .collect::<Vec<_>>();
        let vertex_buffers = vertex_buffer_layouts
            .iter()
            .zip(vertex_attributes_storage.iter())
            .map(|(layout, attributes)| wgpu::VertexBufferLayout {
                array_stride: layout.array_stride,
                step_mode: layout.step_mode,
                attributes,
            })
            .collect::<Vec<_>>();

        let fragment_state = if let Some(fragment) = descriptor.get("fragment") {
            let module_id = fragment
                .get("module")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow!("fragment state missing module"))?
                as u32;
            let module = &self
                .js_shader_modules
                .get(&module_id)
                .ok_or_else(|| anyhow!("unknown shader module handle {module_id}"))?
                .module;
            let entry_point = fragment
                .get("entryPoint")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let targets = fragment
                .get("targets")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(parse_color_target_state)
                .collect::<Result<Vec<_>>>()?;

            Some((module, entry_point, targets))
        } else {
            None
        };

        let primitive = descriptor
            .get("primitive")
            .filter(|value| !value.is_null())
            .map(parse_primitive_state)
            .transpose()?
            .unwrap_or_default();
        let primitive = wgpu::PrimitiveState {
            cull_mode: None,
            ..primitive
        };
        let depth_stencil = descriptor
            .get("depthStencil")
            .filter(|value| !value.is_null())
            .map(parse_depth_stencil_state)
            .transpose()?;
        let multisample = descriptor
            .get("multisample")
            .filter(|value| !value.is_null())
            .map(parse_multisample_state)
            .transpose()?
            .unwrap_or_default();

        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: descriptor.get("label").and_then(Value::as_str),
                layout,
                vertex: wgpu::VertexState {
                    module: vertex_module,
                    entry_point: vertex_entry_point.as_deref(),
                    compilation_options: Default::default(),
                    buffers: &vertex_buffers,
                },
                primitive,
                depth_stencil,
                multisample,
                fragment: fragment_state
                    .as_ref()
                    .map(|(module, entry_point, targets)| wgpu::FragmentState {
                        module,
                        entry_point: entry_point.as_deref(),
                        compilation_options: Default::default(),
                        targets,
                    }),
                multiview_mask: None,
                cache: None,
            });

        let id = self.next_js_id();
        self.js_render_pipelines.insert(
            id,
            JsRenderPipeline {
                pipeline,
                bind_group_layouts: HashMap::new(),
                label: descriptor
                    .get("label")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
            },
        );
        Ok(id)
    }

    pub(crate) fn js_render_pipeline_get_bind_group_layout(
        &mut self,
        pipeline_id: u32,
        index: u32,
    ) -> Result<Value> {
        if let Some(layout_id) = self
            .js_render_pipelines
            .get(&pipeline_id)
            .ok_or_else(|| anyhow!("unknown render pipeline handle {pipeline_id}"))?
            .bind_group_layouts
            .get(&index)
        {
            return Ok(serde_json::json!({
                "handle": layout_id,
                "entries": [],
            }));
        }

        let layout = self
            .js_render_pipelines
            .get(&pipeline_id)
            .ok_or_else(|| anyhow!("unknown render pipeline handle {pipeline_id}"))?
            .pipeline
            .get_bind_group_layout(index);

        let layout_id = self.next_js_id();
        self.js_render_pipelines
            .get_mut(&pipeline_id)
            .ok_or_else(|| anyhow!("unknown render pipeline handle {pipeline_id}"))?
            .bind_group_layouts
            .insert(index, layout_id);
        self.js_bind_group_layouts
            .insert(layout_id, JsBindGroupLayout { layout });
        let entries = infer_auto_bind_group_layout_entries(pipeline_id, index, self)?;
        if !entries.is_empty()
            || self
                .js_render_pipelines
                .get(&pipeline_id)
                .and_then(|pipeline| pipeline.label.as_deref())
                .is_some()
        {
            eprintln!(
                "pipeline layout query: pipeline={} label={:?} index={} inferred_entries={}",
                pipeline_id,
                self.js_render_pipelines
                    .get(&pipeline_id)
                    .and_then(|pipeline| pipeline.label.as_deref()),
                index,
                serde_json::to_string(&entries)?
            );
        }
        Ok(serde_json::json!({
            "handle": layout_id,
            "entries": entries,
        }))
    }

    pub(crate) fn js_create_sampler(
        &mut self,
        _device_id: u32,
        descriptor_json: &str,
    ) -> Result<u32> {
        let descriptor: Value = serde_json::from_str(descriptor_json)?;

        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: descriptor.get("label").and_then(Value::as_str),
            address_mode_u: parse_address_mode(
                descriptor
                    .get("addressModeU")
                    .and_then(Value::as_str)
                    .unwrap_or("clamp-to-edge"),
            )?,
            address_mode_v: parse_address_mode(
                descriptor
                    .get("addressModeV")
                    .and_then(Value::as_str)
                    .unwrap_or("clamp-to-edge"),
            )?,
            address_mode_w: parse_address_mode(
                descriptor
                    .get("addressModeW")
                    .and_then(Value::as_str)
                    .unwrap_or("clamp-to-edge"),
            )?,
            mag_filter: parse_filter_mode(
                descriptor
                    .get("magFilter")
                    .and_then(Value::as_str)
                    .unwrap_or("linear"),
            )?,
            min_filter: parse_filter_mode(
                descriptor
                    .get("minFilter")
                    .and_then(Value::as_str)
                    .unwrap_or("linear"),
            )?,
            mipmap_filter: parse_mipmap_filter_mode(
                descriptor
                    .get("mipmapFilter")
                    .and_then(Value::as_str)
                    .unwrap_or("linear"),
            )?,
            lod_min_clamp: descriptor
                .get("lodMinClamp")
                .and_then(Value::as_f64)
                .unwrap_or(0.0) as f32,
            lod_max_clamp: descriptor
                .get("lodMaxClamp")
                .and_then(Value::as_f64)
                .unwrap_or(32.0) as f32,
            compare: descriptor
                .get("compare")
                .and_then(Value::as_str)
                .map(parse_compare_function)
                .transpose()?,
            anisotropy_clamp: descriptor
                .get("maxAnisotropy")
                .and_then(Value::as_u64)
                .unwrap_or(1) as u16,
            ..Default::default()
        });

        let id = self.next_js_id();
        self.js_samplers.insert(id, JsSampler { sampler });
        Ok(id)
    }

    pub(crate) fn configure_surface(&self) {
        if self.size.width == 0 || self.size.height == 0 {
            return;
        }

        self.surface.configure(
            &self.device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: self.surface_format,
                view_formats: vec![self.surface_format.add_srgb_suffix()],
                alpha_mode: wgpu::CompositeAlphaMode::Auto,
                width: self.size.width,
                height: self.size.height,
                desired_maximum_frame_latency: 2,
                present_mode: wgpu::PresentMode::AutoVsync,
            },
        );
    }

    pub(crate) fn create_depth_view(&self) -> wgpu::TextureView {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth texture"),
            size: wgpu::Extent3d {
                width: self.size.width.max(1),
                height: self.size.height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth24Plus,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        texture.create_view(&wgpu::TextureViewDescriptor::default())
    }

    pub(crate) fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        self.size = size;
        if self.size.width == 0 || self.size.height == 0 {
            return;
        }
        self.configure_surface();
        self.depth_view = Some(self.create_depth_view());
    }

    pub(crate) fn upload_geometry(&mut self, geometry: GeometryUpload) -> Result<()> {
        if !geometry.positions.len().is_multiple_of(3) {
            bail!("position array length must be divisible by 3");
        }
        if geometry.normals.len() != geometry.positions.len() {
            bail!("normal array length must match position array length");
        }

        let mut vertices = Vec::with_capacity(geometry.positions.len() / 3);
        for (position, normal) in geometry
            .positions
            .chunks_exact(3)
            .zip(geometry.normals.chunks_exact(3))
        {
            vertices.push(Vertex {
                position: [position[0], position[1], position[2]],
                normal: [normal[0], normal[1], normal[2]],
            });
        }

        let vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("geometry vertices"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let index_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("geometry indices"),
                contents: bytemuck::cast_slice(&geometry.indices),
                usage: wgpu::BufferUsages::INDEX,
            });

        self.geometry = Some(GpuGeometry {
            vertex_buffer,
            index_buffer,
            index_count: geometry.indices.len() as u32,
        });

        Ok(())
    }

    fn ensure_instance_capacity(&mut self, count: u32) {
        if count <= self.instance_capacity {
            return;
        }

        self.instance_capacity = count.next_power_of_two();
        self.instance_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instance buffer"),
            size: self.instance_capacity as u64 * std::mem::size_of::<InstanceRaw>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
    }

    pub(crate) fn render(&mut self, frame: Option<FrameUpload>) {
        let Some(frame) = frame else {
            return;
        };
        if self.size.width == 0 || self.size.height == 0 {
            return;
        }

        self.ensure_instance_capacity(frame.count.max(1));

        let Some(geometry) = self.geometry.as_ref() else {
            return;
        };

        let world = Mat4::from_cols_array(&frame.world_matrix);
        let mut instances = Vec::with_capacity(frame.count as usize);
        for matrix in frame
            .instance_matrices
            .chunks_exact(16)
            .take(frame.count as usize)
        {
            let model = world * Mat4::from_cols_array(matrix.try_into().unwrap());
            instances.push(InstanceRaw {
                matrix: model.to_cols_array(),
            });
        }
        self.queue
            .write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&instances));

        let view_proj = Mat4::from_cols_array(&frame.projection_matrix)
            * Mat4::from_cols_array(&frame.view_matrix);
        self.queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::bytes_of(&CameraRaw {
                view_proj: view_proj.to_cols_array(),
            }),
        );

        let surface_texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) => texture,
            wgpu::CurrentSurfaceTexture::Occluded | wgpu::CurrentSurfaceTexture::Timeout => return,
            wgpu::CurrentSurfaceTexture::Suboptimal(_) | wgpu::CurrentSurfaceTexture::Outdated => {
                self.configure_surface();
                self.depth_view = Some(self.create_depth_view());
                return;
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                unreachable!("validation errors would already have surfaced")
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface = self.instance.create_surface(self.window.clone()).unwrap();
                self.configure_surface();
                self.depth_view = Some(self.create_depth_view());
                return;
            }
        };

        let Some(depth_view) = self.depth_view.as_ref() else {
            return;
        };

        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor {
                format: Some(self.surface_format.add_srgb_suffix()),
                ..Default::default()
            });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("render encoder"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: frame.clear_color[0] as f64,
                            g: frame.clear_color[1] as f64,
                            b: frame.clear_color[2] as f64,
                            a: frame.clear_color[3] as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            pass.set_vertex_buffer(0, geometry.vertex_buffer.slice(..));
            pass.set_vertex_buffer(1, self.instance_buffer.slice(..));
            pass.set_index_buffer(geometry.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..geometry.index_count, 0, 0..frame.count);
        }

        self.queue.submit([encoder.finish()]);
        self.window.pre_present_notify();
        surface_texture.present();
    }
}

use anyhow::Context as _;
