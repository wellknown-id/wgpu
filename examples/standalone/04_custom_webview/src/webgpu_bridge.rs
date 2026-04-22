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
    bind_group_layouts: HashMap<Handle, wgpu::BindGroupLayout>,
    bind_groups: HashMap<Handle, wgpu::BindGroup>,
    pipeline_layouts: HashMap<Handle, wgpu::PipelineLayout>,
    textures: HashMap<Handle, wgpu::Texture>,
    texture_views: HashMap<Handle, wgpu::TextureView>,
    canvas_textures: HashMap<String, CanvasTexture>,
    // Per-frame transient state (legacy path)
    active_encoder: Option<wgpu::CommandEncoder>,
    active_pass_canvas: Option<String>,
    active_clear_color: Option<[f64; 4]>,
}

pub struct CanvasTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub depth_view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
    pub format: wgpu::TextureFormat,
    pub dirty: bool,
}

/// A single render pass operation recorded from JS.
#[derive(Debug)]
pub enum PassOp {
    SetViewport {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        min_depth: f32,
        max_depth: f32,
    },
    SetPipeline(Handle),
    SetBindGroup {
        index: u32,
        handle: Handle,
    },
    SetVertexBuffer {
        slot: u32,
        handle: Handle,
    },
    SetIndexBuffer {
        handle: Handle,
        format: wgpu::IndexFormat,
    },
    Draw {
        vertex_count: u32,
    },
    DrawIndexed {
        index_count: u32,
    },
}

/// A full render pass recorded from JS.
#[derive(Debug)]
pub struct RecordedRenderPass {
    pub color_view: Handle,
    pub canvas_id: Option<String>,
    pub clear_color: [f64; 4],
    pub depth_view: Option<Handle>,
    pub ops: Vec<PassOp>,
}

/// A full command buffer recorded from JS.
#[derive(Debug)]
pub struct RecordedCommandBuffer {
    pub passes: Vec<RecordedRenderPass>,
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
            bind_group_layouts: HashMap::new(),
            bind_groups: HashMap::new(),
            pipeline_layouts: HashMap::new(),
            textures: HashMap::new(),
            texture_views: HashMap::new(),
            canvas_textures: HashMap::new(),
            active_encoder: None,
            active_pass_canvas: None,
            active_clear_color: None,
        }
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    fn alloc_handle(&mut self) -> Handle {
        let h = self.next_handle;
        self.next_handle += 1;
        h
    }

    pub fn create_shader_module(&mut self, wgsl_source: &str) -> Handle {
        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("webgpu user shader"),
                source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(wgsl_source)),
            });
        let h = self.alloc_handle();
        self.shader_modules.insert(h, module);
        h
    }

    pub fn create_bind_group_layout(&mut self, entries: &[wgpu::BindGroupLayoutEntry]) -> Handle {
        let bgl = self
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("webgpu user bgl"),
                entries,
            });
        let h = self.alloc_handle();
        self.bind_group_layouts.insert(h, bgl);
        h
    }

    pub fn create_bind_group(
        &mut self,
        layout_handle: Handle,
        buffer_bindings: &[(u32, Handle)],
    ) -> Handle {
        let layout = self
            .bind_group_layouts
            .get(&layout_handle)
            .expect("bad bgl handle");

        let entries: Vec<wgpu::BindGroupEntry> = buffer_bindings
            .iter()
            .map(|(binding, buf_handle)| {
                let buffer = self.buffers.get(buf_handle).expect("bad buffer handle");
                wgpu::BindGroupEntry {
                    binding: *binding,
                    resource: buffer.as_entire_binding(),
                }
            })
            .collect();

        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("webgpu user bg"),
            layout,
            entries: &entries,
        });

        let h = self.alloc_handle();
        self.bind_groups.insert(h, bg);
        h
    }

    pub fn create_pipeline_layout(&mut self, bgl_handles: &[Handle]) -> Handle {
        let bgls: Vec<Option<&wgpu::BindGroupLayout>> = bgl_handles
            .iter()
            .map(|h| {
                Some(
                    self.bind_group_layouts
                        .get(h)
                        .expect("bad bgl handle in pipeline layout"),
                )
            })
            .collect();

        let pl = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("webgpu user pl"),
                bind_group_layouts: &bgls,
                immediate_size: 0,
            });

        let h = self.alloc_handle();
        self.pipeline_layouts.insert(h, pl);
        h
    }

    pub fn create_texture(
        &mut self,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> Handle {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("webgpu user texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        });
        let h = self.alloc_handle();
        self.textures.insert(h, texture);
        h
    }

    pub fn create_texture_view(&mut self, texture_handle: Handle) -> Handle {
        let texture = self
            .textures
            .get(&texture_handle)
            .expect("bad texture handle");
        let view = texture.create_view(&Default::default());
        let h = self.alloc_handle();
        self.texture_views.insert(h, view);
        h
    }

    /// Register an externally-created texture (e.g. from XR swapchain).
    pub fn register_texture(&mut self, texture: wgpu::Texture) -> Handle {
        let h = self.alloc_handle();
        self.textures.insert(h, texture);
        h
    }

    /// Register an externally-created texture view.
    pub fn register_texture_view(&mut self, view: wgpu::TextureView) -> Handle {
        let h = self.alloc_handle();
        self.texture_views.insert(h, view);
        h
    }

    /// Remove a registered texture (e.g. when XR swapchain images are released).
    pub fn unregister_texture(&mut self, handle: Handle) {
        self.textures.remove(&handle);
    }

    pub fn unregister_texture_view(&mut self, handle: Handle) {
        self.texture_views.remove(&handle);
    }

    pub fn create_render_pipeline_ext(
        &mut self,
        vs_handle: Handle,
        fs_handle: Handle,
        vs_entry: &str,
        fs_entry: &str,
        vertex_buffer_layouts: &[VertexBufferLayoutDesc],
        pipeline_layout_handle: Handle,
        color_format: wgpu::TextureFormat,
        depth_stencil: Option<wgpu::DepthStencilState>,
        cull_mode: Option<wgpu::Face>,
    ) -> Handle {
        let vs_module = self.shader_modules.get(&vs_handle).expect("bad vs handle");
        let fs_module = self.shader_modules.get(&fs_handle).expect("bad fs handle");
        let pl = if pipeline_layout_handle != 0 {
            self.pipeline_layouts.get(&pipeline_layout_handle)
        } else {
            None
        };

        let wgpu_layouts: Vec<wgpu::VertexBufferLayout> = vertex_buffer_layouts
            .iter()
            .map(|l| wgpu::VertexBufferLayout {
                array_stride: l.array_stride,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &l.attributes,
            })
            .collect();

        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("webgpu user pipeline"),
                layout: pl,
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
                        format: color_format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                // Canvas textures always have a depth buffer, so if no explicit
                // depth_stencil was provided and we're using auto layout, add one.
                depth_stencil: depth_stencil.or_else(|| {
                    if pipeline_layout_handle == 0 {
                        Some(wgpu::DepthStencilState {
                            format: wgpu::TextureFormat::Depth24Plus,
                            depth_write_enabled: Some(true),
                            depth_compare: Some(wgpu::CompareFunction::Less),
                            stencil: wgpu::StencilState::default(),
                            bias: wgpu::DepthBiasState::default(),
                        })
                    } else {
                        None
                    }
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });

        let h = self.alloc_handle();
        self.render_pipelines.insert(h, pipeline);
        h
    }

    /// Replay recorded command buffers from JS.
    pub fn replay_commands(&mut self, command_buffers: &[RecordedCommandBuffer]) {
        for cb in command_buffers {
            for pass in &cb.passes {
                // Resolve the color view: either from a registered texture view handle
                // or from a canvas texture (webgpu.html path)
                let canvas_view_holder;
                let color_view = if pass.color_view != 0 {
                    match self.texture_views.get(&pass.color_view) {
                        Some(v) => v,
                        None => {
                            log::warn!("replay: missing color view handle {}", pass.color_view);
                            continue;
                        }
                    }
                } else if let Some(ref cid) = pass.canvas_id {
                    match self.canvas_textures.get(cid) {
                        Some(ct) => {
                            canvas_view_holder = &ct.view;
                            canvas_view_holder
                        }
                        None => {
                            log::warn!("replay: missing canvas {}", cid);
                            continue;
                        }
                    }
                } else {
                    log::warn!("replay: no color view or canvas");
                    continue;
                };

                let depth_view_ref;
                let depth_attachment = if let Some(dh) = pass.depth_view {
                    depth_view_ref = self.texture_views.get(&dh);
                    depth_view_ref.map(|dv| wgpu::RenderPassDepthStencilAttachment {
                        view: dv,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    })
                } else if let Some(ref cid) = pass.canvas_id {
                    self.canvas_textures
                        .get(cid)
                        .map(|ct| wgpu::RenderPassDepthStencilAttachment {
                            view: &ct.depth_view,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(1.0),
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        })
                } else {
                    None
                };

                let mut encoder =
                    self.device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("webgpu replay encoder"),
                        });

                {
                    let cc = pass.clear_color;
                    let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("webgpu replay pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: color_view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color {
                                    r: cc[0],
                                    g: cc[1],
                                    b: cc[2],
                                    a: cc[3],
                                }),
                                store: wgpu::StoreOp::Store,
                            },
                            depth_slice: None,
                        })],
                        depth_stencil_attachment: depth_attachment,
                        ..Default::default()
                    });

                    for op in &pass.ops {
                        match op {
                            PassOp::SetViewport {
                                x,
                                y,
                                w,
                                h,
                                min_depth,
                                max_depth,
                            } => {
                                rpass.set_viewport(*x, *y, *w, *h, *min_depth, *max_depth);
                            }
                            PassOp::SetPipeline(handle) => {
                                if let Some(pipeline) = self.render_pipelines.get(handle) {
                                    rpass.set_pipeline(pipeline);
                                }
                            }
                            PassOp::SetBindGroup { index, handle } => {
                                if let Some(bg) = self.bind_groups.get(handle) {
                                    rpass.set_bind_group(*index, bg, &[]);
                                }
                            }
                            PassOp::SetVertexBuffer { slot, handle } => {
                                if let Some(buf) = self.buffers.get(handle) {
                                    rpass.set_vertex_buffer(*slot, buf.slice(..));
                                }
                            }
                            PassOp::SetIndexBuffer { handle, format } => {
                                if let Some(buf) = self.buffers.get(handle) {
                                    rpass.set_index_buffer(buf.slice(..), *format);
                                }
                            }
                            PassOp::Draw { vertex_count } => {
                                rpass.draw(0..*vertex_count, 0..1);
                            }
                            PassOp::DrawIndexed { index_count } => {
                                rpass.draw_indexed(0..*index_count, 0, 0..1);
                            }
                        }
                    }
                }

                self.queue.submit(std::iter::once(encoder.finish()));

                if let Some(ref cid) = pass.canvas_id {
                    if let Some(ct) = self.canvas_textures.get_mut(cid) {
                        ct.dirty = true;
                    }
                }
            }
        }
    }

    // --- Legacy canvas methods (kept for non-XR rendering) ---

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

        let pipeline_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("webgpu user pl"),
                bind_group_layouts: &[],
                immediate_size: 0,
            });

        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
        let usage = wgpu::BufferUsages::from_bits_truncate(usage_bits);

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

    pub fn configure_canvas(
        &mut self,
        canvas_id: &str,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("webgpu canvas"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
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
            size: wgpu::Extent3d {
                width,
                height,
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

        self.canvas_textures.insert(
            canvas_id.to_string(),
            CanvasTexture {
                texture,
                view,
                depth_view,
                width,
                height,
                format,
                dirty: false,
            },
        );
    }

    pub fn begin_render_pass(&mut self, canvas_id: &str, clear_color: [f64; 4]) -> bool {
        if !self.canvas_textures.contains_key(canvas_id) {
            return false;
        }

        let encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("webgpu user encoder"),
            });

        self.active_encoder = Some(encoder);
        self.active_pass_canvas = Some(canvas_id.to_string());
        self.active_clear_color = Some(clear_color);
        true
    }

    pub fn render_pass_draw(
        &mut self,
        pipeline_handle: Handle,
        vertex_buffer_handle: Handle,
        vertex_count: u32,
    ) {
        let canvas_id = match self.active_pass_canvas.take() {
            Some(id) => id,
            None => return,
        };

        let encoder = match self.active_encoder.take() {
            Some(e) => e,
            None => return,
        };

        let clear_color = self
            .active_clear_color
            .take()
            .unwrap_or([0.0, 0.0, 0.0, 1.0]);

        let canvas = match self.canvas_textures.get(&canvas_id) {
            Some(c) => c,
            None => return,
        };

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
                    view: &canvas.depth_view,
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
    }

    pub fn get_canvas_texture_view(&self, canvas_id: &str) -> Option<&wgpu::TextureView> {
        self.canvas_textures.get(canvas_id).map(|c| &c.view)
    }

    pub fn get_canvas_size(&self, canvas_id: &str) -> Option<(u32, u32)> {
        self.canvas_textures
            .get(canvas_id)
            .map(|c| (c.width, c.height))
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
