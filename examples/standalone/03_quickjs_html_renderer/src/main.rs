mod ui_overlay;

use std::{
    borrow::Cow,
    cell::RefCell,
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::Instant,
};

use anyhow::{anyhow, bail, Context as _, Result};
use bytemuck::{Pod, Zeroable};
use glam::Mat4;
use rquickjs::{
    loader::{Loader, Resolver},
    Context, Ctx, Function, Module, Object, Persistent, Runtime,
};
use serde::Deserialize;
use serde_json::Value;
use wgpu::util::DeviceExt;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, OwnedDisplayHandle},
    window::{Window, WindowId},
};

type JsResult<T> = std::result::Result<T, rquickjs::Error>;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    normal: [f32; 3],
}

impl Vertex {
    const ATTRS: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRS,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct InstanceRaw {
    matrix: [f32; 16],
}

impl InstanceRaw {
    const ATTRS: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
        2 => Float32x4,
        3 => Float32x4,
        4 => Float32x4,
        5 => Float32x4
    ];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRS,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CameraRaw {
    view_proj: [f32; 16],
}

#[derive(Clone)]
struct GeometryUpload {
    positions: Vec<f32>,
    normals: Vec<f32>,
    indices: Vec<u32>,
}

#[derive(Clone)]
struct FrameUpload {
    projection_matrix: [f32; 16],
    view_matrix: [f32; 16],
    world_matrix: [f32; 16],
    instance_matrices: Vec<f32>,
    count: u32,
    clear_color: [f32; 4],
}

#[derive(Default)]
struct JsHostState {
    geometry: Option<GeometryUpload>,
    geometry_dirty: bool,
    pending_frame: Option<FrameUpload>,
    animation_loop: Option<Persistent<Function<'static>>>,
    resize_listeners: Vec<Persistent<Function<'static>>>,
    now_ms: f64,
}

struct JsEngine {
    runtime: Runtime,
    context: Context,
    host: Rc<RefCell<JsHostState>>,
}

impl JsEngine {
    fn new(
        asset_dir: PathBuf,
        gpu: Rc<RefCell<GpuState>>,
        width: u32,
        height: u32,
        filename: &str,
        search: &str,
    ) -> Result<Self> {
        let runtime = Runtime::new()?;
        runtime.set_loader(
            FsResolver {
                root: asset_dir.clone(),
            },
            FsLoader,
        );

        let context = Context::full(&runtime)?;
        let host = Rc::new(RefCell::new(JsHostState::default()));

        context.with(|ctx| {
            install_host_api(
                ctx.clone(),
                host.clone(),
                asset_dir.clone(),
                gpu.clone(),
                width,
                height,
                search,
            )?;

            let html = fs::read_to_string(asset_dir.join(filename))
                .map_err(|err| rquickjs::Error::new_loading_message(filename, err.to_string()))?;
            let script = extract_module_script(&html)?;
            let entry_name = asset_dir
                .join(filename)
                .with_extension("inline.js")
                .display()
                .to_string();
            Module::evaluate(ctx, entry_name, script)?.finish::<()>()
        })?;

        Ok(Self {
            runtime,
            context,
            host,
        })
    }

    fn dispatch_mouse_event(&mut self, ev_type: &str, x: f64, y: f64) -> Result<()> {
        self.context.with(|ctx| -> Result<()> {
            let target = ctx
                .globals()
                .get::<_, Object>("__activeCanvas")
                .or_else(|_| ctx.globals().get::<_, Object>("document"));

            if let Ok(document) = target {
                let has_pointer = ctx.globals().contains_key("PointerEvent").unwrap_or(false);
                let fallback = if has_pointer { "PointerEvent" } else { "Event" };
                if let Ok(event) = ctx.eval::<Object, _>(format!("new {fallback}('{ev_type}')")) {
                    let _ = event.set("clientX", x);
                    let _ = event.set("clientY", y);
                    let _ = event.set("pageX", x);
                    let _ = event.set("pageY", y);
                    let _ = event.set("pointerId", 1);
                    let _ = event.set("pointerType", "mouse");
                    let _ = event.set("button", if ev_type == "pointermove" { -1 } else { 0 }); // Use -1 for pointermove to avoid pseudo-clicks
                    let _ = event.set("isPrimary", true);
                    let _ = event.set("ctrlKey", false);
                    let _ = event.set("metaKey", false);
                    let _ = event.set("shiftKey", false);

                    let _ = ctx.globals().set("__tempEventTarget", document);
                    let _ = ctx.globals().set("__tempEvent", event);
                    if let Err(e) =
                        ctx.eval::<(), _>("__tempEventTarget.dispatchEvent(__tempEvent)")
                    {
                        if let Some(ex) = ctx.catch().into_exception() {
                            if let Some(msg) = ex.message() {
                                println!("JS Exception: {}", msg);
                            }
                        } else {
                            println!("Dispatch invoke error: {:?}", e);
                        }
                    }
                } else {
                    println!("Failed to eval new PointerEvent");
                }
            } else {
                println!("Failed to find target");
            }
            Ok(())
        })
    }

    fn set_time_ms(&mut self, now_ms: f64) {
        self.host.borrow_mut().now_ms = now_ms;
    }

    fn tick(&mut self) -> Result<()> {
        let callback = self.host.borrow().animation_loop.clone();
        if let Some(callback) = callback {
            if let Err(e) = self.context.with(|ctx| -> JsResult<()> {
                let callback = callback.restore(&ctx)?;
                let now_ms = self.host.borrow().now_ms;
                callback.call::<_, ()>((now_ms,))?;
                Ok(())
            }) {
                self.context.with(|ctx| {
                    if let Some(js_e) = ctx.catch().into_exception() {
                        eprintln!("QuickJS Tick Exception: {:?}", js_e.message());
                        if let Some(stack) = js_e.stack() {
                            eprintln!("Stack: {}", stack);
                        }
                    }
                });
                return Err(anyhow!("QuickJS tick failed: {e}"));
            }
        }

        self.drain_jobs()?;

        Ok(())
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        let listeners = self.host.borrow().resize_listeners.clone();
        if let Err(e) = self.context.with(|ctx| -> JsResult<()> {
            let globals = ctx.globals();
            globals.set("innerWidth", width as i32)?;
            globals.set("innerHeight", height as i32)?;
            for listener in listeners {
                listener.restore(&ctx)?.call::<_, ()>(())?;
            }
            Ok(())
        }) {
            self.context.with(|ctx| {
                if let Some(js_e) = ctx.catch().into_exception() {
                    eprintln!("QuickJS Resize Exception: {:?}", js_e.message());
                    if let Some(stack) = js_e.stack() {
                        eprintln!("Stack: {}", stack);
                    }
                }
            });
            return Err(anyhow!("QuickJS resize failed: {e}"));
        }

        self.drain_jobs()?;

        Ok(())
    }

    fn drain_jobs(&mut self) -> Result<()> {
        while self.runtime.is_job_pending() {
            if let Err(err) = self.runtime.execute_pending_job() {
                self.context.with(|ctx| {
                    if let Some(js_e) = ctx.catch().into_exception() {
                        eprintln!("QuickJS Job Exception: {:?}", js_e.message());
                        if let Some(stack) = js_e.stack() {
                            eprintln!("Stack: {}", stack);
                        }
                    }
                });
                return Err(anyhow!("QuickJS pending job failed: {err}"));
            }
        }

        Ok(())
    }

    fn take_geometry(&mut self) -> Option<GeometryUpload> {
        let mut host = self.host.borrow_mut();
        if !host.geometry_dirty {
            return None;
        }
        host.geometry_dirty = false;
        host.geometry.clone()
    }

    fn take_frame(&mut self) -> Option<FrameUpload> {
        self.host.borrow_mut().pending_frame.take()
    }
}

struct FsResolver {
    root: PathBuf,
}

impl Resolver for FsResolver {
    fn resolve<'js>(&mut self, _ctx: &Ctx<'js>, base: &str, name: &str) -> JsResult<String> {
        let candidate = if name.starts_with("./") || name.starts_with("../") {
            let base = Path::new(base);
            let parent = base.parent().unwrap_or(self.root.as_path());
            parent.join(name)
        } else if name.starts_with('/') {
            self.root.join(name.trim_start_matches('/'))
        } else {
            self.root.join(name)
        };

        let resolved = fs::canonicalize(&candidate)
            .map_err(|err| rquickjs::Error::new_resolving_message(base, name, err.to_string()))?;

        Ok(resolved.display().to_string())
    }
}

struct FsLoader;

impl Loader for FsLoader {
    fn load<'js>(&mut self, ctx: &Ctx<'js>, name: &str) -> JsResult<Module<'js>> {
        let source = fs::read_to_string(name)
            .map_err(|err| rquickjs::Error::new_loading_message(name, err.to_string()))?;
        Module::declare(ctx.clone(), name, source)
    }
}

fn install_host_api(
    ctx: Ctx<'_>,
    host: Rc<RefCell<JsHostState>>,
    asset_dir: PathBuf,
    gpu: Rc<RefCell<GpuState>>,
    width: u32,
    height: u32,
    search: &str,
) -> JsResult<()> {
    let globals = ctx.globals();

    globals.set(
        "__hostLog",
        Function::new(ctx.clone(), |message: String| {
            println!("{message}");
        })?,
    )?;
    globals.set(
        "__hostWarn",
        Function::new(ctx.clone(), |message: String| {
            eprintln!("warning: {message}");
        })?,
    )?;
    globals.set(
        "__hostError",
        Function::new(ctx.clone(), |message: String| {
            eprintln!("error: {message}");
        })?,
    )?;

    let read_root = asset_dir.clone();
    globals.set(
        "__hostReadText",
        Function::new(ctx.clone(), move |path: String| -> JsResult<String> {
            let full_path = resolve_asset_path(&read_root, &path).map_err(|err| {
                rquickjs::Error::new_loading_message(path.clone(), err.to_string())
            })?;
            fs::read_to_string(full_path)
                .map_err(|err| rquickjs::Error::new_loading_message(path, err.to_string()))
        })?,
    )?;

    let read_bytes_root = asset_dir.clone();
    globals.set(
        "__hostReadBytes",
        Function::new(ctx.clone(), move |path: String| -> JsResult<Vec<u8>> {
            let full_path = resolve_asset_path(&read_bytes_root, &path).map_err(|err| {
                rquickjs::Error::new_loading_message(path.clone(), err.to_string())
            })?;
            fs::read(full_path)
                .map_err(|err| rquickjs::Error::new_loading_message(path, err.to_string()))
        })?,
    )?;

    let animation_host = host.clone();
    globals.set(
        "__hostSetAnimationLoop",
        Function::new(
            ctx.clone(),
            move |callback: Persistent<Function<'static>>| -> JsResult<()> {
                animation_host.borrow_mut().animation_loop = Some(callback);
                Ok(())
            },
        )?,
    )?;

    let resize_host = host.clone();
    globals.set(
        "__hostAddResizeListener",
        Function::new(
            ctx.clone(),
            move |callback: Persistent<Function<'static>>| -> JsResult<()> {
                resize_host.borrow_mut().resize_listeners.push(callback);
                Ok(())
            },
        )?,
    )?;

    let now_host = host.clone();
    globals.set(
        "__hostNow",
        Function::new(ctx.clone(), move || -> f64 { now_host.borrow().now_ms })?,
    )?;

    let gpu_request_adapter = gpu.clone();
    globals.set(
        "__hostGpuRequestAdapter",
        Function::new(ctx.clone(), move |_descriptor_json: String| -> u32 {
            gpu_request_adapter.borrow().js_request_adapter()
        })?,
    )?;

    let gpu_request_device = gpu.clone();
    globals.set(
        "__hostGpuRequestDevice",
        Function::new(
            ctx.clone(),
            move |adapter_id: u32, _descriptor_json: String| -> JsResult<u32> {
                gpu_request_device
                    .borrow_mut()
                    .js_request_device(adapter_id)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUAdapter.requestDevice",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_get_queue = gpu.clone();
    globals.set(
        "__hostGpuGetQueue",
        Function::new(ctx.clone(), move |device_id: u32| -> u32 {
            gpu_get_queue.borrow().js_get_queue(device_id)
        })?,
    )?;

    let gpu_canvas_context = gpu.clone();
    globals.set(
        "__hostGpuCreateCanvasContext",
        Function::new(ctx.clone(), move || -> u32 {
            gpu_canvas_context.borrow_mut().js_create_canvas_context()
        })?,
    )?;

    let gpu_preferred_format = gpu.clone();
    globals.set(
        "__hostGpuGetPreferredCanvasFormat",
        Function::new(ctx.clone(), move || -> String {
            gpu_preferred_format
                .borrow()
                .js_get_preferred_canvas_format()
        })?,
    )?;

    let gpu_configure_context = gpu.clone();
    globals.set(
        "__hostGpuConfigureCanvasContext",
        Function::new(
            ctx.clone(),
            move |context_id: u32, device_id: u32, descriptor_json: String| -> JsResult<()> {
                gpu_configure_context
                    .borrow_mut()
                    .js_configure_canvas_context(context_id, device_id, &descriptor_json)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUCanvasContext.configure",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_get_current_texture = gpu.clone();
    globals.set(
        "__hostGpuGetCurrentTexture",
        Function::new(ctx.clone(), move |context_id: u32| -> JsResult<u32> {
            gpu_get_current_texture
                .borrow_mut()
                .js_get_current_texture(context_id)
                .map_err(|err| {
                    rquickjs::Error::new_loading_message(
                        "GPUCanvasContext.getCurrentTexture",
                        err.to_string(),
                    )
                })
        })?,
    )?;

    let gpu_texture_view = gpu.clone();
    globals.set(
        "__hostGpuTextureCreateView",
        Function::new(
            ctx.clone(),
            move |texture_id: u32, descriptor_json: String| -> JsResult<u32> {
                gpu_texture_view
                    .borrow_mut()
                    .js_texture_create_view(texture_id, &descriptor_json)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUTexture.createView",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_create_texture = gpu.clone();
    globals.set(
        "__hostGpuCreateTexture",
        Function::new(
            ctx.clone(),
            move |device_id: u32, descriptor_json: String| -> JsResult<u32> {
                gpu_create_texture
                    .borrow_mut()
                    .js_create_texture(device_id, &descriptor_json)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUDevice.createTexture",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_create_command_encoder = gpu.clone();
    globals.set(
        "__hostGpuCreateCommandEncoder",
        Function::new(
            ctx.clone(),
            move |device_id: u32, descriptor_json: String| -> JsResult<u32> {
                gpu_create_command_encoder
                    .borrow_mut()
                    .js_create_command_encoder(device_id, &descriptor_json)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUDevice.createCommandEncoder",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_create_buffer = gpu.clone();
    globals.set(
        "__hostGpuCreateBuffer",
        Function::new(
            ctx.clone(),
            move |device_id: u32, descriptor_json: String| -> JsResult<u32> {
                gpu_create_buffer
                    .borrow_mut()
                    .js_create_buffer(device_id, &descriptor_json)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUDevice.createBuffer",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_buffer_get_mapped = gpu.clone();
    globals.set(
        "__hostGpuBufferGetMappedRange",
        Function::new(
            ctx.clone(),
            move |buffer_id: u32, offset: u64, size: u64| -> JsResult<Vec<u8>> {
                gpu_buffer_get_mapped
                    .borrow_mut()
                    .js_buffer_get_mapped_range(buffer_id, offset, size)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUBuffer.getMappedRange",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_buffer_set_mapped_range = gpu.clone();
    globals.set(
        "__hostGpuBufferSetMappedRange",
        Function::new(
            ctx.clone(),
            move |buffer_id: u32, bytes: Vec<u8>| -> JsResult<()> {
                gpu_buffer_set_mapped_range
                    .borrow_mut()
                    .js_buffer_set_mapped_range(buffer_id, &bytes)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUBuffer.setMappedRange",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_buffer_unmap = gpu.clone();
    globals.set(
        "__hostGpuBufferUnmap",
        Function::new(ctx.clone(), move |buffer_id: u32| -> JsResult<()> {
            gpu_buffer_unmap
                .borrow_mut()
                .js_buffer_unmap(buffer_id)
                .map_err(|err| {
                    rquickjs::Error::new_loading_message("GPUBuffer.unmap", err.to_string())
                })
        })?,
    )?;

    let gpu_queue_write_buffer = gpu.clone();
    globals.set(
        "__hostGpuQueueWriteBuffer",
        Function::new(
            ctx.clone(),
            move |queue_id: u32,
                  buffer_id: u32,
                  buffer_offset: u64,
                  bytes: Vec<u8>|
                  -> JsResult<()> {
                gpu_queue_write_buffer
                    .borrow_mut()
                    .js_queue_write_buffer(queue_id, buffer_id, buffer_offset, &bytes)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUQueue.writeBuffer",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_queue_write_texture = gpu.clone();
    globals.set(
        "__hostGpuQueueWriteTexture",
        Function::new(
            ctx.clone(),
            move |queue_id: u32, descriptor_json: String| -> JsResult<()> {
                gpu_queue_write_texture
                    .borrow_mut()
                    .js_queue_write_texture(queue_id, &descriptor_json)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUQueue.writeTexture",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_load_image = gpu.clone();
    let image_root = asset_dir.clone();
    globals.set(
        "__hostLoadImage",
        Function::new(ctx.clone(), move |path: String| -> JsResult<u32> {
            let full_path = resolve_asset_path(&image_root, &path).map_err(|err| {
                rquickjs::Error::new_loading_message(path.clone(), err.to_string())
            })?;
            gpu_load_image
                .borrow_mut()
                .js_load_image(&full_path)
                .map_err(|err| rquickjs::Error::new_loading_message(path, err.to_string()))
        })?,
    )?;

    let gpu_load_image_bytes = gpu.clone();
    globals.set(
        "__hostLoadImageBytes",
        Function::new(ctx.clone(), move |bytes: Vec<u8>| -> JsResult<u32> {
            gpu_load_image_bytes
                .borrow_mut()
                .js_load_image_bytes(&bytes)
                .map_err(|err| rquickjs::Error::new_loading_message("ImageBitmap", err.to_string()))
        })?,
    )?;

    let gpu_get_image_size = gpu.clone();
    globals.set(
        "__hostGetImageSize",
        Function::new(ctx.clone(), move |image_id: u32| -> JsResult<Vec<u32>> {
            gpu_get_image_size
                .borrow()
                .js_get_image_size(image_id)
                .map(|(width, height)| vec![width, height])
                .map_err(|err| rquickjs::Error::new_loading_message("ImageBitmap", err.to_string()))
        })?,
    )?;

    let gpu_copy_external_image = gpu.clone();
    globals.set(
        "__hostGpuQueueCopyExternalImageToTexture",
        Function::new(
            ctx.clone(),
            move |queue_id: u32, descriptor_json: String| -> JsResult<()> {
                gpu_copy_external_image
                    .borrow_mut()
                    .js_queue_copy_external_image_to_texture(queue_id, &descriptor_json)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUQueue.copyExternalImageToTexture",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_begin_render_pass = gpu.clone();
    globals.set(
        "__hostGpuBeginRenderPass",
        Function::new(
            ctx.clone(),
            move |encoder_id: u32, descriptor_json: String| -> JsResult<u32> {
                gpu_begin_render_pass
                    .borrow_mut()
                    .js_begin_render_pass(encoder_id, &descriptor_json)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUCommandEncoder.beginRenderPass",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_render_pass_end = gpu.clone();
    globals.set(
        "__hostGpuRenderPassEnd",
        Function::new(ctx.clone(), move |render_pass_id: u32| -> JsResult<()> {
            gpu_render_pass_end
                .borrow_mut()
                .js_render_pass_end(render_pass_id)
                .map_err(|err| {
                    rquickjs::Error::new_loading_message(
                        "GPURenderPassEncoder.end",
                        err.to_string(),
                    )
                })
        })?,
    )?;

    let gpu_render_pass_set_pipeline = gpu.clone();
    globals.set(
        "__hostGpuRenderPassSetPipeline",
        Function::new(
            ctx.clone(),
            move |render_pass_id: u32, pipeline_id: u32| -> JsResult<()> {
                gpu_render_pass_set_pipeline
                    .borrow_mut()
                    .js_render_pass_set_pipeline(render_pass_id, pipeline_id)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPURenderPassEncoder.setPipeline",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_render_pass_set_bind_group = gpu.clone();
    globals.set(
        "__hostGpuRenderPassSetBindGroup",
        Function::new(
            ctx.clone(),
            move |render_pass_id: u32, index: u32, bind_group_id: u32| -> JsResult<()> {
                gpu_render_pass_set_bind_group
                    .borrow_mut()
                    .js_render_pass_set_bind_group(render_pass_id, index, bind_group_id)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPURenderPassEncoder.setBindGroup",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_render_pass_set_index_buffer = gpu.clone();
    globals.set(
        "__hostGpuRenderPassSetIndexBuffer",
        Function::new(
            ctx.clone(),
            move |render_pass_id: u32, buffer_id: u32, format: String| -> JsResult<()> {
                gpu_render_pass_set_index_buffer
                    .borrow_mut()
                    .js_render_pass_set_index_buffer(render_pass_id, buffer_id, &format)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPURenderPassEncoder.setIndexBuffer",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_render_pass_set_vertex_buffer = gpu.clone();
    globals.set(
        "__hostGpuRenderPassSetVertexBuffer",
        Function::new(
            ctx.clone(),
            move |render_pass_id: u32, slot: u32, buffer_id: u32| -> JsResult<()> {
                gpu_render_pass_set_vertex_buffer
                    .borrow_mut()
                    .js_render_pass_set_vertex_buffer(render_pass_id, slot, buffer_id)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPURenderPassEncoder.setVertexBuffer",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_render_pass_set_viewport = gpu.clone();
    globals.set(
        "__hostGpuRenderPassSetViewport",
        Function::new(
            ctx.clone(),
            move |render_pass_id: u32,
                  x: f32,
                  y: f32,
                  width: f32,
                  height: f32,
                  min_depth: f32,
                  max_depth: f32|
                  -> JsResult<()> {
                gpu_render_pass_set_viewport
                    .borrow_mut()
                    .js_render_pass_set_viewport(
                        render_pass_id,
                        x,
                        y,
                        width,
                        height,
                        min_depth,
                        max_depth,
                    )
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPURenderPassEncoder.setViewport",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_render_pass_set_scissor_rect = gpu.clone();
    globals.set(
        "__hostGpuRenderPassSetScissorRect",
        Function::new(
            ctx.clone(),
            move |render_pass_id: u32, x: u32, y: u32, width: u32, height: u32| -> JsResult<()> {
                gpu_render_pass_set_scissor_rect
                    .borrow_mut()
                    .js_render_pass_set_scissor_rect(render_pass_id, x, y, width, height)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPURenderPassEncoder.setScissorRect",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_render_pass_set_stencil_reference = gpu.clone();
    globals.set(
        "__hostGpuRenderPassSetStencilReference",
        Function::new(
            ctx.clone(),
            move |render_pass_id: u32, reference: u32| -> JsResult<()> {
                gpu_render_pass_set_stencil_reference
                    .borrow_mut()
                    .js_render_pass_set_stencil_reference(render_pass_id, reference)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPURenderPassEncoder.setStencilReference",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_render_pass_draw = gpu.clone();
    globals.set(
        "__hostGpuRenderPassDraw",
        Function::new(
            ctx.clone(),
            move |render_pass_id: u32,
                  vertex_count: u32,
                  instance_count: u32,
                  first_vertex: u32,
                  first_instance: u32|
                  -> JsResult<()> {
                gpu_render_pass_draw
                    .borrow_mut()
                    .js_render_pass_draw(
                        render_pass_id,
                        vertex_count,
                        instance_count,
                        first_vertex,
                        first_instance,
                    )
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPURenderPassEncoder.draw",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_render_pass_draw_indexed = gpu.clone();
    globals.set(
        "__hostGpuRenderPassDrawIndexed",
        Function::new(
            ctx.clone(),
            move |render_pass_id: u32,
                  index_count: u32,
                  instance_count: u32,
                  first_index: u32,
                  base_vertex: i32,
                  first_instance: u32|
                  -> JsResult<()> {
                gpu_render_pass_draw_indexed
                    .borrow_mut()
                    .js_render_pass_draw_indexed(
                        render_pass_id,
                        index_count,
                        instance_count,
                        first_index,
                        base_vertex,
                        first_instance,
                    )
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPURenderPassEncoder.drawIndexed",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;
    let gpu_command_copy_texture_to_texture = gpu.clone();
    globals.set(
        "__hostGpuCommandEncoderCopyTextureToTexture",
        Function::new(
            ctx.clone(),
            move |encoder_id: u32, descriptor_json: String| -> JsResult<()> {
                gpu_command_copy_texture_to_texture
                    .borrow_mut()
                    .js_command_encoder_copy_texture_to_texture(encoder_id, &descriptor_json)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUCommandEncoder.copyTextureToTexture",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_command_copy_texture_to_buffer = gpu.clone();
    globals.set(
        "__hostGpuCommandEncoderCopyTextureToBuffer",
        Function::new(
            ctx.clone(),
            move |encoder_id: u32, descriptor_json: String| -> JsResult<()> {
                gpu_command_copy_texture_to_buffer
                    .borrow_mut()
                    .js_command_encoder_copy_texture_to_buffer(encoder_id, &descriptor_json)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUCommandEncoder.copyTextureToBuffer",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_command_encoder_finish = gpu.clone();
    globals.set(
        "__hostGpuCommandEncoderFinish",
        Function::new(ctx.clone(), move |encoder_id: u32| -> JsResult<u32> {
            gpu_command_encoder_finish
                .borrow_mut()
                .js_command_encoder_finish(encoder_id)
                .map_err(|err| {
                    rquickjs::Error::new_loading_message(
                        "GPUCommandEncoder.finish",
                        err.to_string(),
                    )
                })
        })?,
    )?;

    let gpu_create_render_bundle_encoder = gpu.clone();
    globals.set(
        "__hostGpuCreateRenderBundleEncoder",
        Function::new(
            ctx.clone(),
            move |device_id: u32, descriptor_json: String| -> JsResult<u32> {
                gpu_create_render_bundle_encoder
                    .borrow_mut()
                    .js_create_render_bundle_encoder(device_id, &descriptor_json)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUDevice.createRenderBundleEncoder",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_render_bundle_set_pipeline = gpu.clone();
    globals.set(
        "__hostGpuRenderBundleSetPipeline",
        Function::new(
            ctx.clone(),
            move |bundle_id: u32, pipeline_id: u32| -> JsResult<()> {
                gpu_render_bundle_set_pipeline
                    .borrow_mut()
                    .js_render_bundle_set_pipeline(bundle_id, pipeline_id)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPURenderBundleEncoder.setPipeline",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_render_bundle_set_bind_group = gpu.clone();
    globals.set(
        "__hostGpuRenderBundleSetBindGroup",
        Function::new(
            ctx.clone(),
            move |bundle_id: u32, index: u32, bind_group_id: u32| -> JsResult<()> {
                gpu_render_bundle_set_bind_group
                    .borrow_mut()
                    .js_render_bundle_set_bind_group(bundle_id, index, bind_group_id)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPURenderBundleEncoder.setBindGroup",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_render_bundle_draw = gpu.clone();
    globals.set(
        "__hostGpuRenderBundleDraw",
        Function::new(
            ctx.clone(),
            move |bundle_id: u32,
                  vertex_count: u32,
                  instance_count: u32,
                  first_vertex: u32,
                  first_instance: u32|
                  -> JsResult<()> {
                gpu_render_bundle_draw
                    .borrow_mut()
                    .js_render_bundle_draw(
                        bundle_id,
                        vertex_count,
                        instance_count,
                        first_vertex,
                        first_instance,
                    )
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPURenderBundleEncoder.draw",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_render_bundle_finish = gpu.clone();
    globals.set(
        "__hostGpuRenderBundleFinish",
        Function::new(ctx.clone(), move |bundle_id: u32| -> JsResult<u32> {
            gpu_render_bundle_finish
                .borrow_mut()
                .js_render_bundle_finish(bundle_id)
                .map_err(|err| {
                    rquickjs::Error::new_loading_message(
                        "GPURenderBundleEncoder.finish",
                        err.to_string(),
                    )
                })
        })?,
    )?;

    let gpu_render_pass_execute_bundles = gpu.clone();
    globals.set(
        "__hostGpuRenderPassExecuteBundles",
        Function::new(
            ctx.clone(),
            move |render_pass_id: u32, bundle_ids_json: String| -> JsResult<()> {
                gpu_render_pass_execute_bundles
                    .borrow_mut()
                    .js_render_pass_execute_bundles(render_pass_id, &bundle_ids_json)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPURenderPassEncoder.executeBundles",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_queue_submit = gpu.clone();
    globals.set(
        "__hostGpuQueueSubmit",
        Function::new(
            ctx.clone(),
            move |queue_id: u32, command_buffers_json: String| -> JsResult<()> {
                gpu_queue_submit
                    .borrow_mut()
                    .js_queue_submit(queue_id, &command_buffers_json)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message("GPUQueue.submit", err.to_string())
                    })
            },
        )?,
    )?;

    let gpu_create_bind_group_layout = gpu.clone();
    globals.set(
        "__hostGpuCreateBindGroupLayout",
        Function::new(
            ctx.clone(),
            move |device_id: u32, descriptor_json: String| -> JsResult<u32> {
                gpu_create_bind_group_layout
                    .borrow_mut()
                    .js_create_bind_group_layout(device_id, &descriptor_json)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUDevice.createBindGroupLayout",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_create_pipeline_layout = gpu.clone();
    globals.set(
        "__hostGpuCreatePipelineLayout",
        Function::new(
            ctx.clone(),
            move |device_id: u32, descriptor_json: String| -> JsResult<u32> {
                gpu_create_pipeline_layout
                    .borrow_mut()
                    .js_create_pipeline_layout(device_id, &descriptor_json)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUDevice.createPipelineLayout",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_create_bind_group = gpu.clone();
    globals.set(
        "__hostGpuCreateBindGroup",
        Function::new(
            ctx.clone(),
            move |device_id: u32, descriptor_json: String| -> JsResult<u32> {
                gpu_create_bind_group
                    .borrow_mut()
                    .js_create_bind_group(device_id, &descriptor_json)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUDevice.createBindGroup",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_create_shader_module = gpu.clone();
    globals.set(
        "__hostGpuCreateShaderModule",
        Function::new(
            ctx.clone(),
            move |device_id: u32, descriptor_json: String| -> JsResult<u32> {
                gpu_create_shader_module
                    .borrow_mut()
                    .js_create_shader_module(device_id, &descriptor_json)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUDevice.createShaderModule",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_create_render_pipeline = gpu.clone();
    globals.set(
        "__hostGpuCreateRenderPipeline",
        Function::new(
            ctx.clone(),
            move |device_id: u32, descriptor_json: String| -> JsResult<u32> {
                gpu_create_render_pipeline
                    .borrow_mut()
                    .js_create_render_pipeline(device_id, &descriptor_json)
                    .map_err(|err| {
                        eprintln!("createRenderPipeline descriptor: {descriptor_json}");
                        eprintln!("createRenderPipeline error: {err:#}");
                        rquickjs::Error::new_loading_message(
                            "GPUDevice.createRenderPipeline",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_create_sampler = gpu.clone();
    globals.set(
        "__hostGpuCreateSampler",
        Function::new(
            ctx.clone(),
            move |device_id: u32, descriptor_json: String| -> JsResult<u32> {
                gpu_create_sampler
                    .borrow_mut()
                    .js_create_sampler(device_id, &descriptor_json)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUDevice.createSampler",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_render_pipeline_get_bind_group_layout = gpu.clone();
    globals.set(
        "__hostGpuRenderPipelineGetBindGroupLayout",
        Function::new(
            ctx.clone(),
            move |pipeline_id: u32, index: u32| -> JsResult<String> {
                gpu_render_pipeline_get_bind_group_layout
                    .borrow_mut()
                    .js_render_pipeline_get_bind_group_layout(pipeline_id, index)
                    .and_then(|value| serde_json::to_string(&value).map_err(anyhow::Error::from))
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPURenderPipeline.getBindGroupLayout",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_create_compute_pipeline = gpu.clone();
    globals.set(
        "__hostGpuCreateComputePipeline",
        Function::new(
            ctx.clone(),
            move |device_id: u32, descriptor_json: String| -> JsResult<u32> {
                gpu_create_compute_pipeline
                    .borrow_mut()
                    .js_create_compute_pipeline(device_id, &descriptor_json)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUDevice.createComputePipeline",
                            format!("{err:#}"),
                        )
                    })
            },
        ),
    )?;

    let gpu_compute_pipeline_get_bind_group_layout = gpu.clone();
    globals.set(
        "__hostGpuComputePipelineGetBindGroupLayout",
        Function::new(
            ctx.clone(),
            move |pipeline_id: u32, index: u32| -> JsResult<String> {
                gpu_compute_pipeline_get_bind_group_layout
                    .borrow_mut()
                    .js_compute_pipeline_get_bind_group_layout(pipeline_id, index)
                    .and_then(|value| serde_json::to_string(&value).map_err(anyhow::Error::from))
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUComputePipeline.getBindGroupLayout",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_begin_compute_pass = gpu.clone();
    globals.set(
        "__hostGpuBeginComputePass",
        Function::new(
            ctx.clone(),
            move |encoder_id: u32, descriptor_json: String| -> JsResult<u32> {
                gpu_begin_compute_pass
                    .borrow_mut()
                    .js_command_encoder_begin_compute_pass(encoder_id, &descriptor_json)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUCommandEncoder.beginComputePass",
                            format!("{err:#}"),
                        )
                    })
            },
        ),
    )?;

    let gpu_compute_pass_encoder_set_pipeline = gpu.clone();
    globals.set(
        "__hostGpuComputePassEncoderSetPipeline",
        Function::new(
            ctx.clone(),
            move |pass_id: u32, pipeline_id: u32| -> JsResult<()> {
                gpu_compute_pass_encoder_set_pipeline
                    .borrow_mut()
                    .js_compute_pass_encoder_set_pipeline(pass_id, pipeline_id)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUComputePassEncoder.setPipeline",
                            format!("{err:#}"),
                        )
                    })
            },
        ),
    )?;

    let gpu_compute_pass_encoder_set_bind_group = gpu.clone();
    globals.set(
        "__hostGpuComputePassEncoderSetBindGroup",
        Function::new(
            ctx.clone(),
            move |pass_id: u32,
                  index: u32,
                  bind_group_id: u32,
                  dynamic_offsets: Vec<u32>|
                  -> JsResult<()> {
                gpu_compute_pass_encoder_set_bind_group
                    .borrow_mut()
                    .js_compute_pass_encoder_set_bind_group(
                        pass_id,
                        index,
                        bind_group_id,
                        dynamic_offsets,
                    )
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUComputePassEncoder.setBindGroup",
                            format!("{err:#}"),
                        )
                    })
            },
        ),
    )?;

    let gpu_compute_pass_encoder_dispatch_workgroups = gpu.clone();
    globals.set(
        "__hostGpuComputePassEncoderDispatchWorkgroups",
        Function::new(
            ctx.clone(),
            move |pass_id: u32, x: u32, y: u32, z: u32| -> JsResult<()> {
                gpu_compute_pass_encoder_dispatch_workgroups
                    .borrow_mut()
                    .js_compute_pass_encoder_dispatch_workgroups(pass_id, x, y, z)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUComputePassEncoder.dispatchWorkgroups",
                            format!("{err:#}"),
                        )
                    })
            },
        ),
    )?;

    let gpu_compute_pass_encoder_dispatch_workgroups_indirect = gpu.clone();
    globals.set(
        "__hostGpuComputePassEncoderDispatchWorkgroupsIndirect",
        Function::new(
            ctx.clone(),
            move |pass_id: u32, buffer_id: u32, offset: u32| -> JsResult<()> {
                gpu_compute_pass_encoder_dispatch_workgroups_indirect
                    .borrow_mut()
                    .js_compute_pass_encoder_dispatch_workgroups_indirect(
                        pass_id,
                        buffer_id,
                        offset as u64,
                    )
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUComputePassEncoder.dispatchWorkgroupsIndirect",
                            format!("{err:#}"),
                        )
                    })
            },
        ),
    )?;

    let gpu_compute_pass_encoder_end = gpu.clone();
    globals.set(
        "__hostGpuComputePassEncoderEnd",
        Function::new(ctx.clone(), move |pass_id: u32| -> JsResult<()> {
            gpu_compute_pass_encoder_end
                .borrow_mut()
                .js_compute_pass_encoder_end(pass_id)
                .map_err(|err| {
                    rquickjs::Error::new_loading_message(
                        "GPUComputePassEncoder.end",
                        format!("{err:#}"),
                    )
                })
        }),
    )?;

    let geometry_host = host.clone();
    globals.set(
        "__hostSetGeometry",
        Function::new(ctx.clone(), move |geometry: Object<'_>| -> JsResult<()> {
            let positions: Vec<f32> = geometry.get("positions")?;
            let normals: Vec<f32> = geometry.get("normals")?;
            let indices: Vec<u32> = geometry.get("indices")?;

            geometry_host.borrow_mut().geometry = Some(GeometryUpload {
                positions,
                normals,
                indices,
            });
            geometry_host.borrow_mut().geometry_dirty = true;

            Ok(())
        })?,
    )?;

    let frame_host = host.clone();
    globals.set(
        "__hostRenderFrame",
        Function::new(ctx.clone(), move |frame: Object<'_>| -> JsResult<()> {
            let projection_matrix = vec16(
                frame.get::<_, Vec<f32>>("projectionMatrix")?,
                "projectionMatrix",
            )?;
            let view_matrix = vec16(frame.get::<_, Vec<f32>>("viewMatrix")?, "viewMatrix")?;
            let world_matrix = vec16(frame.get::<_, Vec<f32>>("worldMatrix")?, "worldMatrix")?;
            let clear_color = vec4(frame.get::<_, Vec<f32>>("clearColor")?, "clearColor")?;
            let instance_matrices: Vec<f32> = frame.get("instanceMatrices")?;
            let count: u32 = frame.get("count")?;

            frame_host.borrow_mut().pending_frame = Some(FrameUpload {
                projection_matrix,
                view_matrix,
                world_matrix,
                instance_matrices,
                count,
                clear_color,
            });

            Ok(())
        })?,
    )?;

    let ui_gpu = gpu.clone();
    globals.set(
        "__hostGpuUiCreateNode",
        Function::new(ctx.clone(), move || -> JsResult<u32> {
            if let Some(ui) = ui_gpu.borrow_mut().ui.as_mut() {
                Ok(ui.create_node())
            } else {
                Ok(0)
            }
        })?,
    )?;

    let ui_gpu = gpu.clone();
    globals.set(
        "__hostGpuUiAppendChild",
        Function::new(ctx.clone(), move |parent: u32, child: u32| -> JsResult<()> {
            if let Some(ui) = ui_gpu.borrow_mut().ui.as_mut() {
                ui.append_child(parent, child);
            }
            Ok(())
        })?,
    )?;

    let ui_gpu = gpu.clone();
    globals.set(
        "__hostGpuUiSetText",
        Function::new(ctx.clone(), move |id: u32, text: std::string::String| -> JsResult<()> {
            if let Some(ui) = ui_gpu.borrow_mut().ui.as_mut() {
                ui.set_text(id, text);
            }
            Ok(())
        })?,
    )?;

    let ui_gpu = gpu.clone();
    globals.set(
        "__hostGpuUiUpdateStyle",
        Function::new(ctx.clone(), move |id: u32, style: Object<'_>| -> JsResult<()> {
            if let Some(ui) = ui_gpu.borrow_mut().ui.as_mut() {
                let position = if style.get::<&str, std::string::String>("position").unwrap_or_default() == "absolute" {
                    taffy::prelude::Position::Absolute
                } else {
                    taffy::prelude::Position::Relative
                };
                
                let top = style.get::<&str, f32>("top").ok().map(|v| taffy::prelude::length(v)).unwrap_or(taffy::prelude::auto());
                let left = style.get::<&str, f32>("left").ok().map(|v| taffy::prelude::length(v)).unwrap_or(taffy::prelude::auto());

                let get_dimension = |name: &str| -> taffy::prelude::Dimension {
                    if let Ok(v) = style.get::<&str, f32>(name) {
                        taffy::prelude::Dimension::length(v)
                    } else if let Ok(s) = style.get::<&str, std::string::String>(name) {
                        if s.ends_with('%') {
                            s.trim_end_matches('%').parse::<f32>().ok().map(|p| taffy::prelude::Dimension::percent(p / 100.0)).unwrap_or(taffy::prelude::Dimension::auto())
                        } else if s.ends_with("px") {
                            s.trim_end_matches("px").parse::<f32>().ok().map(|v| taffy::prelude::Dimension::length(v)).unwrap_or(taffy::prelude::Dimension::auto())
                        } else {
                            taffy::prelude::Dimension::auto()
                        }
                    } else {
                        taffy::prelude::Dimension::auto()
                    }
                };

                let flex_direction = if style.get::<&str, std::string::String>("flexDirection").unwrap_or_default() == "column" {
                    taffy::prelude::FlexDirection::Column
                } else {
                    taffy::prelude::FlexDirection::Row
                };

                let t_style = taffy::prelude::Style {
                    position,
                    inset: taffy::prelude::Rect { top, left, right: taffy::prelude::auto(), bottom: taffy::prelude::auto() },
                    size: taffy::prelude::Size {
                        width: get_dimension("width"),
                        height: get_dimension("height"),
                    },
                    display: taffy::prelude::Display::Flex,
                    flex_direction,
                    ..Default::default()
                };

                // color
                let bg: Vec<f32> = style.get::<&str, Vec<f32>>("backgroundColor").unwrap_or_else(|_| vec![0.0, 0.0, 0.0, 0.0]);
                let fg: Vec<f32> = style.get::<&str, Vec<f32>>("color").unwrap_or_else(|_| vec![1.0, 1.0, 1.0, 1.0]);

                let bg_color = [bg.get(0).copied().unwrap_or(0.0), bg.get(1).copied().unwrap_or(0.0), bg.get(2).copied().unwrap_or(0.0), bg.get(3).copied().unwrap_or(0.0)];
                let fg_color = [fg.get(0).copied().unwrap_or(1.0), fg.get(1).copied().unwrap_or(1.0), fg.get(2).copied().unwrap_or(1.0), fg.get(3).copied().unwrap_or(1.0)];

                ui.update_style(id, t_style, bg_color, fg_color);
            }
            Ok(())
        })?,
    )?;

    let search = serde_json::to_string(search)
        .map_err(|err| rquickjs::Error::new_into_js_message("rust", "string", err.to_string()))?;

    ctx.eval::<(), _>(format!(
        r#"
globalThis.window = globalThis;
globalThis.self = globalThis;
globalThis.location = {{ search: {search} }};
globalThis.innerWidth = {width};
globalThis.innerHeight = {height};
globalThis.devicePixelRatio = 1;
globalThis.navigator = {{ userAgent: 'quickjs-wgpu', gpu: null }};
globalThis.performance = {{ now() {{ return __hostNow(); }} }};
globalThis.localStorage = {{
  getItem() {{ return null; }},
  setItem() {{}},
  removeItem() {{}}
}};
function __createEventTarget(parent = null) {{
  const listeners = new Map();
  return {{
    __parent: parent,
    addEventListener(type, listener) {{
      if (typeof listener !== 'function') {{
        return;
      }}
      const list = listeners.get(type) || [];
      list.push(listener);
      listeners.set(type, list);
    }},
    removeEventListener(type, listener) {{
      const list = listeners.get(type) || [];
      listeners.set(type, list.filter((candidate) => candidate !== listener));
    }},
    dispatchEvent(event) {{
      const type = event?.type;
      if (!type) {{
        return true;
      }}
      const payload = {{
        preventDefault() {{}},
        ...event,
        target: event?.target ?? this,
        currentTarget: this,
      }};
      for (const listener of listeners.get(type) || []) {{
        listener.call(this, payload);
      }}
      return true;
    }},
    getRootNode() {{
      return this.__parent?.getRootNode ? this.__parent.getRootNode() : this;
    }},
  }};
}}
class HTMLImageElement {{
  constructor() {{
    this.tagName = 'IMG';
    this.style = {{}};
    this.crossOrigin = null;
    this.width = 0;
    this.height = 0;
    this.naturalWidth = 0;
    this.naturalHeight = 0;
    this.complete = false;
    this.onload = null;
    this.onerror = null;
    this.__src = '';
    this.__imageId = undefined;
    this.__listeners = new Map();
  }}

  addEventListener(type, listener) {{
    if (typeof listener !== 'function') {{
      return;
    }}
    const listeners = this.__listeners.get(type) || [];
    listeners.push(listener);
    this.__listeners.set(type, listeners);
  }}

  removeEventListener(type, listener) {{
    const listeners = this.__listeners.get(type);
    if (!listeners) {{
      return;
    }}
    this.__listeners.set(type, listeners.filter((candidate) => candidate !== listener));
  }}

  __dispatch(type, event) {{
    const payload = event || {{ type, target: this, currentTarget: this }};
    const listeners = this.__listeners.get(type) || [];
    for (const listener of [ ...listeners ]) {{
      listener.call(this, payload);
    }}
    const handler = type === 'load' ? this.onload : type === 'error' ? this.onerror : null;
    if (typeof handler === 'function') {{
      handler.call(this, payload);
    }}
  }}

  get src() {{
    return this.__src;
  }}

  set src(value) {{
    this.__src = String(value);

    const finishLoad = (imageId) => {{
      const [ width, height ] = __hostGetImageSize(imageId);
      this.__imageId = imageId;
      this.width = width;
      this.height = height;
      this.naturalWidth = width;
      this.naturalHeight = height;
      this.complete = true;
      this.__dispatch('load');
    }};

    const failLoad = (error) => {{
      this.__imageId = undefined;
      this.width = 0;
      this.height = 0;
      this.naturalWidth = 0;
      this.naturalHeight = 0;
      this.complete = false;
      this.__dispatch('error', {{ type: 'error', target: this, currentTarget: this, error }});
    }};

    if (this.__src.startsWith('blob:')) {{
      fetch(this.__src)
        .then((response) => response.arrayBuffer())
        .then((buffer) => __hostLoadImageBytes(Array.from(new Uint8Array(buffer))))
        .then(finishLoad, failLoad);
      return;
    }}

    try {{
      finishLoad(__hostLoadImage(this.__src));
    }} catch (error) {{
      failLoad(error);
    }}
  }}

  decode() {{
    return this.complete
      ? Promise.resolve()
      : Promise.reject(new Error('Image is not loaded'));
  }}
}}

function createDocumentElement(tag) {{
  const normalized = String(tag).toLowerCase();
  if (normalized === 'img') {{
    return new HTMLImageElement();
  }}

  return {{
    tagName: String(tag).toUpperCase(),
    style: {{}},
    width: innerWidth,
    height: innerHeight,
  }};
}}

globalThis.HTMLImageElement = HTMLImageElement;
globalThis.Image = HTMLImageElement;
globalThis.document = {{
  ...__createEventTarget(),
  hidden: false,
  body: {{
    appendChild(node) {{
      return node;
    }}
  }},
  createElement(tag) {{
    return createDocumentElement(tag);
  }},
  createElementNS(_ns, tag) {{
    return createDocumentElement(tag);
  }}
}};
globalThis.console = {{
  log(...args) {{ __hostLog(args.map((value) => String(value)).join(' ')); }},
  warn(...args) {{ __hostWarn(args.map((value) => String(value)).join(' ')); }},
  error(...args) {{ __hostError(args.map((value) => String(value)).join(' ')); }}
}};
globalThis.addEventListener = function(type, listener) {{
  if (type === 'resize') {{
    __hostAddResizeListener(listener);
  }}
}};
globalThis.removeEventListener = function() {{}};
globalThis.requestAnimationFrame = function(callback) {{
  __hostSetAnimationLoop(callback);
  return 1;
}};
globalThis.cancelAnimationFrame = function() {{}};
"#
    ))?;

    Ok(())
}

fn resolve_asset_path(root: &Path, path: &str) -> Result<PathBuf> {
    let root = fs::canonicalize(root)?;
    let requested = path.trim_start_matches("./");
    let full_path = fs::canonicalize(root.join(requested))?;
    if !full_path.starts_with(&root) {
        bail!("asset path escapes asset directory: {path}");
    }
    Ok(full_path)
}

fn extract_module_script(html: &str) -> JsResult<String> {
    let start = html.find("<script type=\"module\">").ok_or_else(|| {
        rquickjs::Error::new_loading_message("index.html", "missing module script")
    })?;
    let start = start + "<script type=\"module\">".len();
    let end = html[start..].find("</script>").ok_or_else(|| {
        rquickjs::Error::new_loading_message("index.html", "unterminated module script")
    })?;
    Ok(html[start..start + end].trim().to_string())
}

fn vec16(values: Vec<f32>, name: &'static str) -> JsResult<[f32; 16]> {
    values.try_into().map_err(|_| {
        rquickjs::Error::new_from_js_message(
            "Array",
            name,
            format!("expected 16 values for {name}"),
        )
    })
}

fn vec4(values: Vec<f32>, name: &'static str) -> JsResult<[f32; 4]> {
    values.try_into().map_err(|_| {
        rquickjs::Error::new_from_js_message("Array", name, format!("expected 4 values for {name}"))
    })
}

struct GpuGeometry {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct ComputePipelineDescriptorData {
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
enum ComputeCommand {
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

struct RecordedComputePass {
    label: Option<String>,
    commands: Vec<ComputeCommand>,
}

struct JsComputePass {
    encoder_id: u32,
    label: Option<String>,
    commands: Vec<ComputeCommand>,
}

enum JsTextureResource {
    Surface(wgpu::SurfaceTexture),
    Owned(wgpu::Texture),
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct CopyTextureToTextureDescriptor {
    source: CopyImageTextureDetails,
    destination: CopyImageTextureDetails,
    copy_size: [u32; 3],
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct CopyTextureToBufferDescriptor {
    source: CopyImageTextureDetails,
    destination: CopyImageBufferDetails,
    copy_size: [u32; 3],
}

struct JsCommandEncoder {
    encoder: wgpu::CommandEncoder,
    recorded_commands: Vec<RecordedEncoderCommand>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct CopyImageTextureDetails {
    texture: u32,
    mip_level: u32,
    origin: [u32; 3],
    aspect: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct CopyImageBufferDetails {
    buffer: u32,
    offset: u64,
    bytes_per_row: Option<u32>,
    rows_per_image: Option<u32>,
}

enum RecordedEncoderCommand {
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

struct JsRenderBundleEncoder {
    commands: Vec<RenderCommand>,
}

struct JsRenderBundle {
    commands: Vec<RenderCommand>,
}

struct JsRenderPass {
    encoder_id: u32,
    descriptor: RenderPassDescriptorData,
    commands: Vec<RenderCommand>,
}

struct JsCommandBuffer {
    command_buffer: Option<wgpu::CommandBuffer>,
}

struct JsBuffer {
    buffer: wgpu::Buffer,
    mapped: Option<Vec<u8>>,
    size: u64,
    shadow: Vec<u8>,
}

struct RenderPassDescriptorData {
    color_attachments: Vec<RenderPassColorAttachmentData>,
    depth_stencil_attachment: Option<RenderPassDepthStencilAttachmentData>,
}

struct RenderPassColorAttachmentData {
    view_id: u32,
    resolve_target_id: Option<u32>,
    clear_value: Option<[f64; 4]>,
    load_op: Option<String>,
    store_op: Option<String>,
}

struct RenderPassDepthStencilAttachmentData {
    view_id: u32,
    depth_clear_value: Option<f32>,
    depth_load_op: Option<String>,
    depth_store_op: Option<String>,
}

struct RecordedRenderPass {
    descriptor: RenderPassDescriptorData,
    commands: Vec<RenderCommand>,
}

enum RenderCommand {
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

struct JsBindGroupLayout {
    layout: wgpu::BindGroupLayout,
}

struct JsPipelineLayout {
    layout: wgpu::PipelineLayout,
}

struct JsBindGroup {
    group: wgpu::BindGroup,
    descriptor_json: String,
}

struct JsShaderModule {
    module: wgpu::ShaderModule,
}

struct JsRenderPipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layouts: HashMap<u32, u32>,
    label: Option<String>,
}

struct JsComputePipeline {
    pipeline: wgpu::ComputePipeline,
    bind_group_layouts: HashMap<u32, u32>,
    label: Option<String>,
}

struct JsSampler {
    sampler: wgpu::Sampler,
}

struct JsImage {
    width: u32,
    height: u32,
    rgba8: Vec<u8>,
}

struct GpuState {
    instance: wgpu::Instance,
    window: Arc<Window>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    size: winit::dpi::PhysicalSize<u32>,
    surface: wgpu::Surface<'static>,
    surface_format: wgpu::TextureFormat,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
    instance_buffer: wgpu::Buffer,
    instance_capacity: u32,
    depth_view: Option<wgpu::TextureView>,
    geometry: Option<GpuGeometry>,
    js_next_id: u32,
    js_canvas_context_id: u32,
    js_current_surface_texture: Option<u32>,
    js_textures: HashMap<u32, JsTextureResource>,
    js_texture_views: HashMap<u32, wgpu::TextureView>,
    js_texture_view_owners: HashMap<u32, u32>,
    js_buffers: HashMap<u32, JsBuffer>,
    js_bind_group_layouts: HashMap<u32, JsBindGroupLayout>,
    js_pipeline_layouts: HashMap<u32, JsPipelineLayout>,
    js_bind_groups: HashMap<u32, JsBindGroup>,
    js_shader_modules: HashMap<u32, JsShaderModule>,
    js_render_pipelines: HashMap<u32, JsRenderPipeline>,
    js_compute_pipelines: HashMap<u32, JsComputePipeline>,
    js_samplers: HashMap<u32, JsSampler>,
    js_images: HashMap<u32, JsImage>,
    js_command_encoders: HashMap<u32, JsCommandEncoder>,
    js_render_bundle_encoders: HashMap<u32, JsRenderBundleEncoder>,
    js_render_bundles: HashMap<u32, JsRenderBundle>,
    js_render_passes: HashMap<u32, JsRenderPass>,
    js_compute_passes: HashMap<u32, JsComputePass>,
    js_command_buffers: HashMap<u32, JsCommandBuffer>,
    ui: Option<ui_overlay::UiOverlay>,
    mouse: (f32, f32),
}

impl GpuState {
    async fn new(display: OwnedDisplayHandle, window: Arc<Window>) -> Result<Self> {
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

    fn window(&self) -> &Window {
        &self.window
    }

    fn next_js_id(&mut self) -> u32 {
        let id = self.js_next_id;
        self.js_next_id += 1;
        id
    }

    fn js_request_adapter(&self) -> u32 {
        1
    }

    fn js_request_device(&mut self, _adapter_id: u32) -> Result<u32> {
        Ok(1)
    }

    fn js_get_queue(&self, _device_id: u32) -> u32 {
        1
    }

    fn js_create_canvas_context(&mut self) -> u32 {
        self.js_canvas_context_id
    }

    fn js_get_preferred_canvas_format(&self) -> String {
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

    fn js_configure_canvas_context(
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

    fn js_get_current_texture(&mut self, _context_id: u32) -> Result<u32> {
        if let Some(texture_id) = self.js_current_surface_texture {
            return Ok(texture_id);
        }

        let mut retry = 0;
        let surface_texture = loop {
            match self.surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(texture) | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => break texture,
                wgpu::CurrentSurfaceTexture::Occluded | wgpu::CurrentSurfaceTexture::Timeout => {
                    bail!("surface is temporarily unavailable")
                }
                wgpu::CurrentSurfaceTexture::Outdated => {
                    if retry > 0 {
                        // Sometimes X11/Wayland quirks cause endless Outdated events if size is totally squashed/hidden
                        bail!("surface is perpetually outdated");
                    }
                    if self.size.width > 0 && self.size.height > 0 {
                        self.configure_surface();
                        self.depth_view = Some(self.create_depth_view());
                    } else {
                        bail!("surface size is 0x0");
                    }
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

    fn js_create_texture(&mut self, _device_id: u32, descriptor_json: &str) -> Result<u32> {
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

    fn js_texture_create_view(&mut self, texture_id: u32, descriptor_json: &str) -> Result<u32> {
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

    fn js_create_command_encoder(
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

    fn js_create_buffer(&mut self, _device_id: u32, descriptor_json: &str) -> Result<u32> {
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

    fn js_create_render_bundle_encoder(
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

    fn js_buffer_get_mapped_range(
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

    fn js_buffer_set_mapped_range(&mut self, buffer_id: u32, bytes: &[u8]) -> Result<()> {
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

    fn js_buffer_unmap(&mut self, buffer_id: u32) -> Result<()> {
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

    fn js_begin_render_pass(&mut self, encoder_id: u32, descriptor_json: &str) -> Result<u32> {
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

    fn js_render_pass_end(&mut self, render_pass_id: u32) -> Result<()> {
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

    fn js_command_encoder_copy_texture_to_texture(
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

    fn js_command_encoder_copy_texture_to_buffer(
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

    fn js_command_encoder_finish(&mut self, encoder_id: u32) -> Result<u32> {
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

    fn js_queue_submit(&mut self, _queue_id: u32, command_buffers_json: &str) -> Result<()> {
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

    fn present_submitted_surface_textures(&mut self) {
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

    fn js_queue_write_buffer(
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

    fn js_queue_write_texture(&mut self, _queue_id: u32, descriptor_json: &str) -> Result<()> {
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

    fn js_load_image(&mut self, path: &Path) -> Result<u32> {
        let image = image::open(path)?.to_rgba8();
        self.insert_js_image(image)
    }

    fn js_load_image_bytes(&mut self, bytes: &[u8]) -> Result<u32> {
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

    fn js_get_image_size(&self, image_id: u32) -> Result<(u32, u32)> {
        let image = self
            .js_images
            .get(&image_id)
            .ok_or_else(|| anyhow!("unknown image handle {image_id}"))?;
        Ok((image.width, image.height))
    }

    fn js_queue_copy_external_image_to_texture(
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

    fn js_render_pass_set_pipeline(&mut self, render_pass_id: u32, pipeline_id: u32) -> Result<()> {
        let render_pass = self
            .js_render_passes
            .get_mut(&render_pass_id)
            .ok_or_else(|| anyhow!("unknown render pass handle {render_pass_id}"))?;
        render_pass
            .commands
            .push(RenderCommand::SetPipeline { pipeline_id });
        Ok(())
    }

    fn js_render_bundle_set_pipeline(&mut self, bundle_id: u32, pipeline_id: u32) -> Result<()> {
        let bundle = self
            .js_render_bundle_encoders
            .get_mut(&bundle_id)
            .ok_or_else(|| anyhow!("unknown render bundle encoder handle {bundle_id}"))?;
        bundle
            .commands
            .push(RenderCommand::SetPipeline { pipeline_id });
        Ok(())
    }

    fn js_render_pass_set_bind_group(
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

    fn js_render_bundle_set_bind_group(
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

    fn js_render_pass_set_index_buffer(
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

    fn js_render_pass_set_vertex_buffer(
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

    fn js_render_pass_set_viewport(
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

    fn js_render_pass_set_scissor_rect(
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

    fn js_render_pass_set_stencil_reference(
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

    fn js_render_pass_draw(
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

    fn js_render_bundle_draw(
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

    fn js_render_pass_draw_indexed(
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

    fn js_render_bundle_finish(&mut self, bundle_id: u32) -> Result<u32> {
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

    fn js_render_pass_execute_bundles(
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

    fn js_create_bind_group_layout(
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

    fn js_create_pipeline_layout(&mut self, _device_id: u32, descriptor_json: &str) -> Result<u32> {
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

    fn js_create_bind_group(&mut self, _device_id: u32, descriptor_json: &str) -> Result<u32> {
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

    fn js_create_shader_module(&mut self, _device_id: u32, descriptor_json: &str) -> Result<u32> {
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

    fn js_create_compute_pipeline(
        &mut self,
        _device_id: u32,
        descriptor_json: &str,
    ) -> Result<u32> {
        let descriptor: ComputePipelineDescriptorData = serde_json::from_str(descriptor_json)?;

        // Find shader module
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
                label: descriptor.label,
            },
        );

        Ok(id)
    }

    fn js_compute_pipeline_get_bind_group_layout(
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

    fn js_command_encoder_begin_compute_pass(
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

    fn js_compute_pass_encoder_set_pipeline(
        &mut self,
        pass_id: u32,
        pipeline_id: u32,
    ) -> Result<()> {
        let pass = self.js_compute_passes.get_mut(&pass_id).unwrap();
        pass.commands
            .push(ComputeCommand::SetPipeline { pipeline_id });
        Ok(())
    }

    fn js_compute_pass_encoder_set_bind_group(
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

    fn js_compute_pass_encoder_dispatch_workgroups(
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

    fn js_compute_pass_encoder_dispatch_workgroups_indirect(
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

    fn js_compute_pass_encoder_end(&mut self, pass_id: u32) -> Result<()> {
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

    fn js_create_render_pipeline(&mut self, _device_id: u32, descriptor_json: &str) -> Result<u32> {
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

    fn js_render_pipeline_get_bind_group_layout(
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

    fn js_create_sampler(&mut self, _device_id: u32, descriptor_json: &str) -> Result<u32> {
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

    fn configure_surface(&self) {
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

    fn create_depth_view(&self) -> wgpu::TextureView {
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

    fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        self.size = size;
        if self.size.width == 0 || self.size.height == 0 {
            return;
        }
        self.configure_surface();
        self.depth_view = Some(self.create_depth_view());
    }

    fn upload_geometry(&mut self, geometry: GeometryUpload) -> Result<()> {
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

    fn render(&mut self, frame: Option<FrameUpload>) {
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
                label: Some("render pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
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

fn texture_size_from_json(value: Option<&Value>) -> Result<wgpu::Extent3d> {
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

fn parse_texture_dimension(value: &str) -> Result<wgpu::TextureDimension> {
    match value {
        "1d" => Ok(wgpu::TextureDimension::D1),
        "2d" => Ok(wgpu::TextureDimension::D2),
        "3d" => Ok(wgpu::TextureDimension::D3),
        other => bail!("unsupported texture dimension {other}"),
    }
}

fn parse_texture_view_dimension(value: &str) -> Result<wgpu::TextureViewDimension> {
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

fn parse_origin_3d(value: Option<&Value>) -> Result<wgpu::Origin3d> {
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

fn parse_texture_aspect(value: &str) -> Result<wgpu::TextureAspect> {
    match value {
        "all" => Ok(wgpu::TextureAspect::All),
        "stencil-only" => Ok(wgpu::TextureAspect::StencilOnly),
        "depth-only" => Ok(wgpu::TextureAspect::DepthOnly),
        other => bail!("unsupported texture aspect {other}"),
    }
}

fn parse_texture_format(value: &str) -> Result<wgpu::TextureFormat> {
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

#[derive(Clone)]
struct ParsedVertexBufferLayout {
    array_stride: u64,
    step_mode: wgpu::VertexStepMode,
    attributes: Vec<wgpu::VertexAttribute>,
}

fn parse_vertex_buffer_layout(value: &Value) -> Result<ParsedVertexBufferLayout> {
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

fn parse_vertex_format(value: &str) -> Result<wgpu::VertexFormat> {
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

fn parse_color_target_state(value: &Value) -> Result<Option<wgpu::ColorTargetState>> {
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

fn parse_primitive_state(value: &Value) -> Result<wgpu::PrimitiveState> {
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

fn parse_depth_stencil_state(value: &Value) -> Result<wgpu::DepthStencilState> {
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

fn parse_compare_function(value: &str) -> Result<wgpu::CompareFunction> {
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

fn parse_address_mode(value: &str) -> Result<wgpu::AddressMode> {
    match value {
        "clamp-to-edge" => Ok(wgpu::AddressMode::ClampToEdge),
        "repeat" => Ok(wgpu::AddressMode::Repeat),
        "mirror-repeat" => Ok(wgpu::AddressMode::MirrorRepeat),
        other => bail!("unsupported address mode {other}"),
    }
}

fn parse_filter_mode(value: &str) -> Result<wgpu::FilterMode> {
    match value {
        "nearest" => Ok(wgpu::FilterMode::Nearest),
        "linear" => Ok(wgpu::FilterMode::Linear),
        other => bail!("unsupported filter mode {other}"),
    }
}

fn parse_mipmap_filter_mode(value: &str) -> Result<wgpu::MipmapFilterMode> {
    match value {
        "nearest" => Ok(wgpu::MipmapFilterMode::Nearest),
        "linear" => Ok(wgpu::MipmapFilterMode::Linear),
        other => bail!("unsupported mipmap filter mode {other}"),
    }
}

fn parse_multisample_state(value: &Value) -> Result<wgpu::MultisampleState> {
    Ok(wgpu::MultisampleState {
        count: value.get("count").and_then(Value::as_u64).unwrap_or(1) as u32,
        mask: !0,
        alpha_to_coverage_enabled: value
            .get("alphaToCoverageEnabled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn parse_render_pass_descriptor(value: &Value) -> Result<RenderPassDescriptorData> {
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

fn parse_load_op_color(
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

fn fallback_clear_load_op_color(clear_value: Option<[f64; 4]>) -> wgpu::LoadOp<wgpu::Color> {
    let clear_value = clear_value.unwrap_or([0.125, 0.25, 0.375, 1.0]);
    wgpu::LoadOp::Clear(wgpu::Color {
        r: clear_value[0],
        g: clear_value[1],
        b: clear_value[2],
        a: clear_value[3],
    })
}

fn fallback_clear_load_op_depth(clear_value: Option<f32>) -> wgpu::LoadOp<f32> {
    wgpu::LoadOp::Clear(clear_value.unwrap_or(1.0))
}

fn parse_load_op_depth(
    load_op: Option<&str>,
    clear_value: Option<f32>,
) -> Result<wgpu::LoadOp<f32>> {
    match load_op.unwrap_or("load") {
        "load" => Ok(wgpu::LoadOp::Load),
        "clear" => Ok(wgpu::LoadOp::Clear(clear_value.unwrap_or(1.0))),
        other => bail!("unsupported depth load op {other}"),
    }
}

fn parse_store_op(store_op: Option<&str>) -> Result<wgpu::StoreOp> {
    match store_op.unwrap_or("store") {
        "store" => Ok(wgpu::StoreOp::Store),
        "discard" => Ok(wgpu::StoreOp::Discard),
        other => bail!("unsupported store op {other}"),
    }
}

fn parse_index_format(format: &str) -> Result<wgpu::IndexFormat> {
    match format {
        "uint16" => Ok(wgpu::IndexFormat::Uint16),
        "uint32" => Ok(wgpu::IndexFormat::Uint32),
        other => bail!("unsupported index format {other}"),
    }
}

fn flip_rgba_rows(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let row_len = width as usize * 4;
    let mut flipped = vec![0; data.len()];

    for y in 0..height as usize {
        let src = y * row_len;
        let dst = (height as usize - 1 - y) * row_len;
        flipped[dst..dst + row_len].copy_from_slice(&data[src..src + row_len]);
    }

    flipped
}

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

#[derive(Default)]
struct App {
    gpu: Option<Rc<RefCell<GpuState>>>,
    js: Option<JsEngine>,
    start_time: Option<Instant>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("QuickJS HTML Renderer")
                        .with_inner_size(winit::dpi::PhysicalSize::new(1280, 720)),
                )
                .unwrap(),
        );

        let gpu = Rc::new(RefCell::new(
            pollster::block_on(GpuState::new(
                event_loop.owned_display_handle(),
                window.clone(),
            ))
            .unwrap(),
        ));

        let asset_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets");
        let arg = std::env::args()
            .nth(1)
            .unwrap_or_else(|| "index.html".to_string());

        let (filename, search) = if let Some(idx) = arg.find('?') {
            let search = format!("?{}", &arg[idx + 1..]);
            let filename = arg[..idx].to_string();
            (filename, search)
        } else {
            (arg, String::new())
        };

        let js = JsEngine::new(asset_dir, gpu.clone(), 1280, 720, &filename, &search).unwrap();

        self.start_time = Some(Instant::now());
        self.gpu = Some(gpu);
        self.js = Some(js);
        window.request_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        let Some(js) = self.js.as_mut() else {
            return;
        };

        match event {
            WindowEvent::CursorMoved { position, .. } => {
                gpu.borrow_mut().mouse = (position.x as f32, position.y as f32);
                let _ = js.dispatch_mouse_event("pointermove", position.x, position.y);
            }
            WindowEvent::MouseInput {
                state,
                button: winit::event::MouseButton::Left,
                ..
            } => {
                let ev_type = if state == winit::event::ElementState::Pressed {
                    "pointerdown"
                } else {
                    "pointerup"
                };
                let mouse = gpu.borrow().mouse;
                let _ = js.dispatch_mouse_event(ev_type, mouse.0 as f64, mouse.1 as f64);
            }
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                gpu.borrow_mut().resize(size);
                js.resize(size.width, size.height).unwrap();
                gpu.borrow().window().request_redraw();
            }
            WindowEvent::RedrawRequested => {
                if let Some(start_time) = self.start_time {
                    let ms = start_time.elapsed().as_secs_f64() * 1000.0;
                    js.set_time_ms(ms);
                }

                if let Err(err) = js.tick() {
                    eprintln!("QuickJS tick failed: {err:#}");
                    event_loop.exit();
                    return;
                }
                gpu.borrow_mut().present_submitted_surface_textures();

                if let Some(geometry) = js.take_geometry() {
                    gpu.borrow_mut().upload_geometry(geometry).unwrap();
                }
                gpu.borrow_mut().render(js.take_frame());
                gpu.borrow().window().request_redraw();
            }
            _ => {}
        }
    }
}

fn main() {
    env_logger::init();

    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::default();
    event_loop.run_app(&mut app).unwrap();
}
