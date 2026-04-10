use bytemuck::{Pod, Zeroable};
use font8x8::UnicodeFonts;
use taffy::prelude::*;
use std::collections::HashMap;
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

pub struct UiNode {
    pub taffy_id: NodeId,
    pub text: String,
    pub bg_color: [f32; 4],
    pub text_color: [f32; 4],
}

pub struct UiOverlay {
    pub pipeline: wgpu::RenderPipeline,
    pub vertex_buffer: wgpu::Buffer,
    pub bind_group: wgpu::BindGroup,
    pub screen_buffer: wgpu::Buffer,
    pub taffy: TaffyTree,
    pub nodes: HashMap<u32, UiNode>,
    pub root_node: u32,
    pub next_node_id: u32,
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

        let mut taffy = TaffyTree::new();
        let root_style = Style {
            position: Position::Absolute,
            size: Size { width: length(100.0), height: length(100.0) }, // replaced on compute_layout
            ..Default::default()
        };
        let root_taffy = taffy.new_leaf(root_style).unwrap();
        let mut nodes = HashMap::new();
        nodes.insert(0, UiNode {
            taffy_id: root_taffy,
            text: String::new(),
            bg_color: [0.0, 0.0, 0.0, 0.0],
            text_color: [0.0, 0.0, 0.0, 0.0],
        });

        Self {
            pipeline,
            vertex_buffer,
            bind_group,
            screen_buffer,
            taffy,
            nodes,
            root_node: 0,
            next_node_id: 1,
        }
    }

    pub fn create_node(&mut self) -> u32 {
        let id = self.next_node_id;
        self.next_node_id += 1;
        let taffy_id = self.taffy.new_leaf(Style::default()).unwrap();
        self.nodes.insert(id, UiNode {
            taffy_id,
            text: String::new(),
            bg_color: [0.0, 0.0, 0.0, 0.0], // transparent by default
            text_color: [1.0, 1.0, 1.0, 1.0], // white text by default
        });
        id
    }

    pub fn append_child(&mut self, parent: u32, child: u32) {
        if let (Some(p), Some(c)) = (self.nodes.get(&parent), self.nodes.get(&child)) {
            let _ = self.taffy.add_child(p.taffy_id, c.taffy_id);
        }
    }

    pub fn set_text(&mut self, id: u32, text: String) {
        if let Some(node) = self.nodes.get_mut(&id) {
            node.text = text;
        }
    }

    pub fn update_style(&mut self, id: u32, style: Style, bg_color: [f32; 4], text_color: [f32; 4]) {
        if let Some(node) = self.nodes.get_mut(&id) {
            let _ = self.taffy.set_style(node.taffy_id, style);
            node.bg_color = bg_color;
            node.text_color = text_color;
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
        if let Some(root) = self.nodes.get(&self.root_node) {
            let _ = self.taffy.compute_layout(
                root.taffy_id,
                Size {
                    width: AvailableSpace::Definite(width),
                    height: AvailableSpace::Definite(height),
                },
            );

            for (_, node) in &self.nodes {
                if let Ok(l) = self.taffy.layout(node.taffy_id) {
                    // Skip root node visually, and skip nodes with neither text nor bg
                    if node.taffy_id == root.taffy_id { continue; }

                    if node.bg_color[3] > 0.0 {
                        instances.push(UiInstance {
                            rect: [l.location.x, l.location.y, l.size.width, l.size.height],
                            color: node.bg_color,
                            glyph_index: -1,
                            has_texture: 0,
                            _pad: [0; 2],
                        });
                    }

                    if !node.text.is_empty() {
                        let mut cursor_x = l.location.x + 10.0;
                        let cursor_y = l.location.y + 10.0;
                        for c in node.text.chars() {
                            instances.push(UiInstance {
                                rect: [cursor_x, cursor_y, 8.0, 8.0],
                                color: node.text_color,
                                glyph_index: c as i32,
                                has_texture: 1,
                                _pad: [0; 2],
                            });
                            cursor_x += 8.0;
                        }
                    }
                }
            }
        }

        if instances.is_empty() {
            return;
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
