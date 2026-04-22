use std::collections::HashMap;
use std::sync::Arc;

pub type Handle = u64;

pub struct WebGpuBridge {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    next_handle: Handle,
    shader_modules: HashMap<Handle, wgpu::ShaderModule>,
    render_pipelines: HashMap<Handle, wgpu::RenderPipeline>,
    buffers: HashMap<Handle, wgpu::Buffer>,
    canvas_textures: HashMap<String, CanvasTexture>,
    // Per-frame transient state
    active_encoder: Option<wgpu::CommandEncoder>,
    active_pass_canvas: Option<String>,
}

pub struct CanvasTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
    pub format: wgpu::TextureFormat,
    pub dirty: bool,
}

impl WebGpuBridge {
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Self {
        Self {
            device,
            queue,
            next_handle: 1,
            shader_modules: HashMap::new(),
            render_pipelines: HashMap::new(),
            buffers: HashMap::new(),
            canvas_textures: HashMap::new(),
            active_encoder: None,
            active_pass_canvas: None,
        }
    }

    fn alloc_handle(&mut self) -> Handle {
        let h = self.next_handle;
        self.next_handle += 1;
        h
    }

    pub fn create_shader_module(&mut self, wgsl_source: &str) -> Handle {
        let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("webgpu user shader"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(wgsl_source)),
        });
        let h = self.alloc_handle();
        self.shader_modules.insert(h, module);
        h
    }

    pub fn create_render_pipeline(
        &mut self,
        vs_handle: Handle,
        fs_handle: Handle,
        vs_entry: &str,
        fs_entry: &str,
        vertex_buffer_layouts: &[VertexBufferLayoutDesc],
        format: wgpu::TextureFormat,
        topology: wgpu::PrimitiveTopology,
    ) -> Handle {
        let vs_module = self.shader_modules.get(&vs_handle).expect("bad vs handle");
        let fs_module = self.shader_modules.get(&fs_handle).expect("bad fs handle");


        let wgpu_layouts: Vec<wgpu::VertexBufferLayout> = vertex_buffer_layouts
            .iter()
            .map(|l| wgpu::VertexBufferLayout {
                array_stride: l.array_stride,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &l.attributes,
            })
            .collect();

        let pipeline_layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("webgpu user pl"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });

        let pipeline = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("webgpu user pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: vs_module,
                entry_point: Some(vs_entry),
                buffers: &wgpu_layouts,
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: fs_module,
                entry_point: Some(fs_entry),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24Plus,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let h = self.alloc_handle();
        self.render_pipelines.insert(h, pipeline);
        h
    }

    pub fn create_buffer(&mut self, size: u64, usage_bits: u32) -> Handle {
        let mut usage = wgpu::BufferUsages::COPY_DST;
        if usage_bits & 0x20 != 0 {
            usage |= wgpu::BufferUsages::VERTEX;
        }
        if usage_bits & 0x40 != 0 {
            usage |= wgpu::BufferUsages::INDEX;
        }
        if usage_bits & 0x10 != 0 {
            usage |= wgpu::BufferUsages::UNIFORM;
        }

        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("webgpu user buffer"),
            size,
            usage,
            mapped_at_creation: false,
        });

        let h = self.alloc_handle();
        self.buffers.insert(h, buffer);
        h
    }

    pub fn write_buffer(&self, handle: Handle, data: &[u8]) {
        if let Some(buf) = self.buffers.get(&handle) {
            self.queue.write_buffer(buf, 0, data);
        }
    }

    pub fn configure_canvas(&mut self, canvas_id: &str, width: u32, height: u32, format: wgpu::TextureFormat) {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("webgpu canvas"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());

        let depth_tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("webgpu canvas depth"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth24Plus,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let _ = depth_tex.create_view(&Default::default());

        self.canvas_textures.insert(canvas_id.to_string(), CanvasTexture {
            texture,
            view,
            width,
            height,
            format,
            dirty: false,
        });
    }

    pub fn begin_render_pass(&mut self, canvas_id: &str, clear_r: f64, clear_g: f64, clear_b: f64, clear_a: f64) -> bool {
        let canvas = match self.canvas_textures.get(canvas_id) {
            Some(c) => c,
            None => return false,
        };

        let _ = canvas;

        let encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("webgpu user encoder"),
        });

        self.active_encoder = Some(encoder);
        self.active_pass_canvas = Some(canvas_id.to_string());
        true
    }

    pub fn render_pass_draw(
        &mut self,
        pipeline_handle: Handle,
        vertex_buffer_handle: Handle,
        vertex_count: u32,
        clear_color: [f64; 4],
    ) {
        let canvas_id = match self.active_pass_canvas.as_ref() {
            Some(id) => id.clone(),
            None => return,
        };

        let encoder = match self.active_encoder.take() {
            Some(e) => e,
            None => return,
        };

        let canvas = match self.canvas_textures.get(&canvas_id) {
            Some(c) => c,
            None => return,
        };

        let depth_tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("webgpu pass depth"),
            size: wgpu::Extent3d {
                width: canvas.width,
                height: canvas.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth24Plus,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth_tex.create_view(&Default::default());

        let mut encoder = encoder;
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("webgpu user pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &canvas.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear_color[0],
                            g: clear_color[1],
                            b: clear_color[2],
                            a: clear_color[3],
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });

            if let Some(pipeline) = self.render_pipelines.get(&pipeline_handle) {
                rpass.set_pipeline(pipeline);
            }
            if let Some(buffer) = self.buffers.get(&vertex_buffer_handle) {
                rpass.set_vertex_buffer(0, buffer.slice(..));
            }
            rpass.draw(0..vertex_count, 0..1);
        }

        self.queue.submit(std::iter::once(encoder.finish()));

        if let Some(canvas) = self.canvas_textures.get_mut(&canvas_id) {
            canvas.dirty = true;
        }

        self.active_pass_canvas = None;
    }

    pub fn get_canvas_texture_view(&self, canvas_id: &str) -> Option<&wgpu::TextureView> {
        self.canvas_textures.get(canvas_id).map(|c| &c.view)
    }

    pub fn get_canvas_size(&self, canvas_id: &str) -> Option<(u32, u32)> {
        self.canvas_textures.get(canvas_id).map(|c| (c.width, c.height))
    }

    pub fn has_canvas(&self, canvas_id: &str) -> bool {
        self.canvas_textures.contains_key(canvas_id)
    }

    pub fn canvas_ids(&self) -> Vec<String> {
        self.canvas_textures.keys().cloned().collect()
    }

    pub fn preferred_format(&self) -> wgpu::TextureFormat {
        wgpu::TextureFormat::Rgba8UnormSrgb
    }
}

pub struct VertexBufferLayoutDesc {
    pub array_stride: u64,
    pub attributes: Vec<wgpu::VertexAttribute>,
}
