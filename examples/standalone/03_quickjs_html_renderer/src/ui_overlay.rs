use bytemuck::{Pod, Zeroable};
use font8x8::UnicodeFonts;
use taffy::prelude::*;
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct UiVertex {
    pub position: [f32; 2],
    pub uv: [f32; 2],
}

impl UiVertex {
    pub const ATTRS: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRS,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct UiInstance {
    pub rect: [f32; 4],
    pub color: [f32; 4],
    pub glyph_index: i32,
    pub has_texture: u32,
    pub _pad: [u32; 2],
}

impl UiInstance {
    pub const ATTRS: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
        2 => Float32x4,
        3 => Float32x4,
        4 => Sint32,
        5 => Uint32,
    ];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRS,
        }
    }
}

pub struct UiOverlay {
    pub pipeline: wgpu::RenderPipeline,
    pub vertex_buffer: wgpu::Buffer,
    pub bind_group: wgpu::BindGroup,
    pub screen_buffer: wgpu::Buffer,
    pub taffy: TaffyTree,
}

impl UiOverlay {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ui_shader"),
            source: wgpu::ShaderSource::Wgsl(
                r#"
                struct ScreenUniform {
                    size: vec2<f32>,
                    mouse: vec2<f32>,
                };
                @group(0) @binding(0) var<uniform> screen: ScreenUniform;
                @group(0) @binding(1) var font_tex: texture_2d<f32>;
                @group(0) @binding(2) var font_sampler: sampler;

                struct UiInput {
                    @location(0) pos: vec2<f32>,
                    @location(1) uv: vec2<f32>,
                    @location(2) rect: vec4<f32>,
                    @location(3) color: vec4<f32>,
                    @location(4) glyph_idx: i32,
                    @location(5) has_tex: u32,
                };

                struct UiOutput {
                    @builtin(position) pos: vec4<f32>,
                    @location(0) uv: vec2<f32>,
                    @location(1) color: vec4<f32>,
                    @location(2) glyph_idx: i32,
                    @location(3) has_tex: u32,
                };

                @vertex
                fn vs_main(in: UiInput) -> UiOutput {
                    var out: UiOutput;
                    out.uv = in.uv;
                    out.color = in.color;
                    out.glyph_idx = in.glyph_idx;
                    out.has_tex = in.has_tex;
                    let x = in.rect.x + in.pos.x * in.rect.z;
                    let y = in.rect.y + in.pos.y * in.rect.w;
                    let nx = (x / screen.size.x) * 2.0 - 1.0;
                    let ny = (1.0 - (y / screen.size.y)) * 2.0 - 1.0;
                    out.pos = vec4<f32>(nx, ny, 0.0, 1.0);
                    return out;
                }

                @fragment
                fn fs_main(in: UiOutput) -> @location(0) vec4<f32> {
                    if (in.has_tex == 0u) {
                        return in.color;
                    }
                    if (in.glyph_idx < 0) {
                        return vec4(0.0);
                    }
                    let uv_y_start = f32(in.glyph_idx) * 8.0;
                    let final_uv = vec2<f32>(in.uv.x, (uv_y_start + in.uv.y * 8.0) / 1024.0);
                    let texel = textureSample(font_tex, font_sampler, final_uv);
                    return vec4<f32>(in.color.rgb, in.color.a * texel.r);
                }
            "#
                .into(),
            ),
        });

        let screen_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screen uniform"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Generate font8x8 texture
        let mut font_data = vec![0u8; 8 * 1024 * 4];
        for i in 0..128 {
            if let Some(glyph) = font8x8::BASIC_FONTS.get(char::from_u32(i).unwrap()) {
                for (y, row) in glyph.iter().enumerate() {
                    for x in 0..8 {
                        if (row & (1 << x)) != 0 {
                            let idx = ((i as usize * 8 + y) * 8 + x) * 4;
                            font_data[idx] = 255;
                            font_data[idx + 1] = 255;
                            font_data[idx + 2] = 255;
                            font_data[idx + 3] = 255;
                        }
                    }
                }
            }
        }

        let font_tex = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("font atlas"),
                size: wgpu::Extent3d {
                    width: 8,
                    height: 1024,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &font_data,
        );
        let font_view = font_tex.create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ui bind group layout"),
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

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ui bind group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: screen_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&font_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ui pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ui render pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: None,
                compilation_options: Default::default(),
                buffers: &[UiVertex::layout(), UiInstance::layout()],
            },
            cache: None,
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: None,
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
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
        });

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ui vertex buffer"),
            contents: bytemuck::cast_slice(&[
                UiVertex {
                    position: [0.0, 0.0],
                    uv: [0.0, 0.0],
                },
                UiVertex {
                    position: [1.0, 0.0],
                    uv: [1.0, 0.0],
                },
                UiVertex {
                    position: [0.0, 1.0],
                    uv: [0.0, 1.0],
                },
                UiVertex {
                    position: [1.0, 0.0],
                    uv: [1.0, 0.0],
                },
                UiVertex {
                    position: [1.0, 1.0],
                    uv: [1.0, 1.0],
                },
                UiVertex {
                    position: [0.0, 1.0],
                    uv: [0.0, 1.0],
                },
            ]),
            usage: wgpu::BufferUsages::VERTEX,
        });

        Self {
            pipeline,
            vertex_buffer,
            bind_group,
            screen_buffer,
            taffy: TaffyTree::new(),
        }
    }

    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        width: f32,
        height: f32,
        mouse: (f32, f32),
    ) {
        queue.write_buffer(
            &self.screen_buffer,
            0,
            bytemuck::cast_slice(&[width, height, mouse.0, mouse.1]),
        );

        let mut instances = Vec::new();
        self.taffy.clear();

        // 1. Build test taffy layout mimicking <div id="info"> with some text
        let title_style = Style {
            position: Position::Absolute,
            size: Size {
                width: length(250.0),
                height: length(60.0),
            },
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            justify_content: Some(JustifyContent::Center),
            align_items: Some(AlignItems::Center),
            ..Default::default()
        };
        let wrapper = self.taffy.new_leaf(title_style).unwrap();

        self.taffy
            .compute_layout(
                wrapper,
                Size {
                    width: AvailableSpace::Definite(width),
                    height: AvailableSpace::Definite(height),
                },
            )
            .unwrap();

        let l = self.taffy.layout(wrapper).unwrap();

        // Hover state
        let hovered = mouse.0 >= l.location.x
            && mouse.0 <= l.location.x + l.size.width
            && mouse.1 >= l.location.y
            && mouse.1 <= l.location.y + l.size.height;

        let bg_color = if hovered {
            [0.0, 0.2, 0.6, 0.8]
        } else {
            [0.0, 0.0, 0.0, 0.6]
        };

        // Background box
        instances.push(UiInstance {
            rect: [l.location.x, l.location.y, l.size.width, l.size.height],
            color: bg_color,
            glyph_index: -1,
            has_texture: 0,
            _pad: [0; 2],
        });

        // Text string
        let text = "three.js Instancing";
        let text_color = if hovered {
            [1.0, 1.0, 0.0, 1.0]
        } else {
            [1.0, 1.0, 1.0, 1.0]
        };
        for (i, c) in text.chars().enumerate() {
            instances.push(UiInstance {
                rect: [
                    l.location.x + 10.0 + (i as f32 * 8.0),
                    l.location.y + 20.0,
                    8.0,
                    8.0,
                ],
                color: text_color,
                glyph_index: c as i32,
                has_texture: 1,
                _pad: [0; 2],
            });
        }

        let instance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ui instance buffer"),
            contents: bytemuck::cast_slice(&instances),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ui encoder"),
        });
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ui pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rpass.set_pipeline(&self.pipeline);
            rpass.set_bind_group(0, &self.bind_group, &[]);
            rpass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            rpass.set_vertex_buffer(1, instance_buffer.slice(..));
            rpass.draw(0..6, 0..instances.len() as u32);
        }
        queue.submit(std::iter::once(encoder.finish()));
    }
}
