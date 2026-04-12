use std::{cell::RefCell, fs, path::PathBuf, rc::Rc};

use rquickjs::{Ctx, Function, Object, Persistent};

use crate::gpu_state::GpuState;
use crate::js_engine::{resolve_asset_path, vec16, vec4, JsHostState};
use crate::{FrameUpload, GeometryUpload, JsResult};

pub(crate) fn install_host_api(
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

    let gpu_get_device_features = gpu.clone();
    globals.set(
        "__hostGpuGetDeviceFeatures",
        Function::new(ctx.clone(), move || -> Vec<String> {
            gpu_get_device_features.borrow().js_get_device_features()
        })?,
    )?;

    let gpu_get_device_limits = gpu.clone();
    globals.set(
        "__hostGpuGetDeviceLimits",
        Function::new(ctx.clone(), move || -> String {
            gpu_get_device_limits
                .borrow()
                .js_get_device_limits()
                .to_string()
        })?,
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

    let gpu_render_pass_draw_indirect = gpu.clone();
    globals.set(
        "__hostGpuRenderPassDrawIndirect",
        Function::new(
            ctx.clone(),
            move |render_pass_id: u32, buffer_id: u32, offset: u64| -> JsResult<()> {
                gpu_render_pass_draw_indirect
                    .borrow_mut()
                    .js_render_pass_draw_indirect(render_pass_id, buffer_id, offset)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPURenderPassEncoder.drawIndirect",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_render_pass_draw_indexed_indirect = gpu.clone();
    globals.set(
        "__hostGpuRenderPassDrawIndexedIndirect",
        Function::new(
            ctx.clone(),
            move |render_pass_id: u32, buffer_id: u32, offset: u64| -> JsResult<()> {
                gpu_render_pass_draw_indexed_indirect
                    .borrow_mut()
                    .js_render_pass_draw_indexed_indirect(render_pass_id, buffer_id, offset)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPURenderPassEncoder.drawIndexedIndirect",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_copy_buffer_to_buffer = gpu.clone();
    globals.set(
        "__hostGpuCommandEncoderCopyBufferToBuffer",
        Function::new(
            ctx.clone(),
            move |encoder_id: u32,
                  source_id: u32,
                  source_offset: u64,
                  dest_id: u32,
                  dest_offset: u64,
                  size: u64|
                  -> JsResult<()> {
                gpu_copy_buffer_to_buffer
                    .borrow_mut()
                    .js_command_encoder_copy_buffer_to_buffer(
                        encoder_id,
                        source_id,
                        source_offset,
                        dest_id,
                        dest_offset,
                        size,
                    )
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message(
                            "GPUCommandEncoder.copyBufferToBuffer",
                            err.to_string(),
                        )
                    })
            },
        )?,
    )?;

    let gpu_buffer_map_async = gpu.clone();
    globals.set(
        "__hostGpuBufferMapAsync",
        Function::new(
            ctx.clone(),
            move |buffer_id: u32, mode: u32, offset: u64, size: u64| -> JsResult<()> {
                gpu_buffer_map_async
                    .borrow_mut()
                    .js_buffer_map_async(buffer_id, mode, offset, size)
                    .map_err(|err| {
                        rquickjs::Error::new_loading_message("GPUBuffer.mapAsync", err.to_string())
                    })
            },
        )?,
    )?;

    let gpu_buffer_destroy = gpu.clone();
    globals.set(
        "__hostGpuBufferDestroy",
        Function::new(ctx.clone(), move |buffer_id: u32| -> JsResult<()> {
            gpu_buffer_destroy
                .borrow_mut()
                .js_buffer_destroy(buffer_id)
                .map_err(|err| {
                    rquickjs::Error::new_loading_message("GPUBuffer.destroy", err.to_string())
                })
        })?,
    )?;

    let gpu_texture_destroy = gpu.clone();
    globals.set(
        "__hostGpuTextureDestroy",
        Function::new(ctx.clone(), move |texture_id: u32| -> JsResult<()> {
            gpu_texture_destroy
                .borrow_mut()
                .js_texture_destroy(texture_id)
                .map_err(|err| {
                    rquickjs::Error::new_loading_message("GPUTexture.destroy", err.to_string())
                })
        })?,
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
        Function::new(
            ctx.clone(),
            move |parent: u32, child: u32| -> JsResult<()> {
                if let Some(ui) = ui_gpu.borrow_mut().ui.as_mut() {
                    ui.append_child(parent, child);
                }
                Ok(())
            },
        )?,
    )?;

    let ui_gpu = gpu.clone();
    globals.set(
        "__hostGpuUiSetText",
        Function::new(
            ctx.clone(),
            move |id: u32, text: std::string::String| -> JsResult<()> {
                if let Some(ui) = ui_gpu.borrow_mut().ui.as_mut() {
                    ui.set_text(id, text);
                }
                Ok(())
            },
        )?,
    )?;

    let ui_gpu = gpu.clone();
    globals.set(
        "__hostGpuUiUpdateStyle",
        Function::new(
            ctx.clone(),
            move |id: u32, style: Object<'_>| -> JsResult<()> {
                if let Some(ui) = ui_gpu.borrow_mut().ui.as_mut() {
                    let position = if style
                        .get::<&str, std::string::String>("position")
                        .unwrap_or_default()
                        == "absolute"
                    {
                        taffy::prelude::Position::Absolute
                    } else {
                        taffy::prelude::Position::Relative
                    };

                    let top = style
                        .get::<&str, f32>("top")
                        .ok()
                        .map(|v| taffy::prelude::length(v))
                        .unwrap_or(taffy::prelude::auto());
                    let left = style
                        .get::<&str, f32>("left")
                        .ok()
                        .map(|v| taffy::prelude::length(v))
                        .unwrap_or(taffy::prelude::auto());

                    let get_dimension = |name: &str| -> taffy::prelude::Dimension {
                        if let Ok(v) = style.get::<&str, f32>(name) {
                            taffy::prelude::Dimension::length(v)
                        } else if let Ok(s) = style.get::<&str, std::string::String>(name) {
                            if s.ends_with('%') {
                                s.trim_end_matches('%')
                                    .parse::<f32>()
                                    .ok()
                                    .map(|p| taffy::prelude::Dimension::percent(p / 100.0))
                                    .unwrap_or(taffy::prelude::Dimension::auto())
                            } else if s.ends_with("px") {
                                s.trim_end_matches("px")
                                    .parse::<f32>()
                                    .ok()
                                    .map(|v| taffy::prelude::Dimension::length(v))
                                    .unwrap_or(taffy::prelude::Dimension::auto())
                            } else {
                                taffy::prelude::Dimension::auto()
                            }
                        } else {
                            taffy::prelude::Dimension::auto()
                        }
                    };

                    let get_length_percentage_auto =
                        |name: &str| -> taffy::prelude::LengthPercentageAuto {
                            if let Ok(v) = style.get::<&str, f32>(name) {
                                taffy::prelude::LengthPercentageAuto::length(v)
                            } else if let Ok(s) = style.get::<&str, std::string::String>(name) {
                                if s.ends_with('%') {
                                    s.trim_end_matches('%')
                                        .parse::<f32>()
                                        .ok()
                                        .map(|p| {
                                            taffy::prelude::LengthPercentageAuto::percent(p / 100.0)
                                        })
                                        .unwrap_or(taffy::prelude::LengthPercentageAuto::auto())
                                } else if s.ends_with("px") {
                                    s.trim_end_matches("px")
                                        .parse::<f32>()
                                        .ok()
                                        .map(|v| taffy::prelude::LengthPercentageAuto::length(v))
                                        .unwrap_or(taffy::prelude::LengthPercentageAuto::auto())
                                } else {
                                    taffy::prelude::LengthPercentageAuto::auto()
                                }
                            } else {
                                taffy::prelude::LengthPercentageAuto::auto()
                            }
                        };

                    let get_length_percentage = |name: &str| -> taffy::prelude::LengthPercentage {
                        if let Ok(v) = style.get::<&str, f32>(name) {
                            taffy::prelude::LengthPercentage::length(v)
                        } else if let Ok(s) = style.get::<&str, std::string::String>(name) {
                            if s.ends_with('%') {
                                s.trim_end_matches('%')
                                    .parse::<f32>()
                                    .ok()
                                    .map(|p| taffy::prelude::LengthPercentage::percent(p / 100.0))
                                    .unwrap_or(taffy::prelude::LengthPercentage::length(0.0))
                            } else if s.ends_with("px") {
                                s.trim_end_matches("px")
                                    .parse::<f32>()
                                    .ok()
                                    .map(|v| taffy::prelude::LengthPercentage::length(v))
                                    .unwrap_or(taffy::prelude::LengthPercentage::length(0.0))
                            } else {
                                taffy::prelude::LengthPercentage::length(0.0)
                            }
                        } else {
                            taffy::prelude::LengthPercentage::length(0.0)
                        }
                    };

                    let padding = taffy::prelude::Rect {
                        left: get_length_percentage("paddingLeft"),
                        right: get_length_percentage("paddingRight"),
                        top: get_length_percentage("paddingTop"),
                        bottom: get_length_percentage("paddingBottom"),
                    };
                    let margin = taffy::prelude::Rect {
                        left: get_length_percentage_auto("marginLeft"),
                        right: get_length_percentage_auto("marginRight"),
                        top: get_length_percentage_auto("marginTop"),
                        bottom: get_length_percentage_auto("marginBottom"),
                    };

                    let flex_direction = if style
                        .get::<&str, std::string::String>("flexDirection")
                        .unwrap_or_default()
                        == "column"
                    {
                        taffy::prelude::FlexDirection::Column
                    } else {
                        taffy::prelude::FlexDirection::Row
                    };

                    let align_items = match style
                        .get::<&str, std::string::String>("alignItems")
                        .unwrap_or_default()
                        .as_str()
                    {
                        "center" => Some(taffy::prelude::AlignItems::Center),
                        "flex-start" => Some(taffy::prelude::AlignItems::FlexStart),
                        "flex-end" => Some(taffy::prelude::AlignItems::FlexEnd),
                        _ => None,
                    };

                    let justify_content = match style
                        .get::<&str, std::string::String>("justifyContent")
                        .unwrap_or_default()
                        .as_str()
                    {
                        "center" => Some(taffy::prelude::JustifyContent::Center),
                        "space-between" => Some(taffy::prelude::JustifyContent::SpaceBetween),
                        "flex-start" => Some(taffy::prelude::JustifyContent::FlexStart),
                        "flex-end" => Some(taffy::prelude::JustifyContent::FlexEnd),
                        _ => None,
                    };

                    let t_style = taffy::prelude::Style {
                        position,
                        inset: taffy::prelude::Rect {
                            top,
                            left,
                            right: taffy::prelude::auto(),
                            bottom: taffy::prelude::auto(),
                        },
                        size: taffy::prelude::Size {
                            width: get_dimension("width"),
                            height: get_dimension("height"),
                        },
                        padding,
                        margin,
                        display: taffy::prelude::Display::Flex,
                        flex_direction,
                        align_items,
                        justify_content,
                        ..Default::default()
                    };

                    let parse_color = |name: &str, default: [f32; 4]| -> [f32; 4] {
                        if let Ok(arr) = style.get::<&str, Vec<f32>>(name) {
                            return [
                                arr.get(0).copied().unwrap_or(default[0]),
                                arr.get(1).copied().unwrap_or(default[1]),
                                arr.get(2).copied().unwrap_or(default[2]),
                                arr.get(3).copied().unwrap_or(default[3]),
                            ];
                        }
                        if let Ok(string) = style.get::<&str, std::string::String>(name) {
                            if string.starts_with("rgba(") {
                                let parts: Vec<&str> = string
                                    .trim_start_matches("rgba(")
                                    .trim_end_matches(')')
                                    .split(',')
                                    .collect();
                                if parts.len() == 4 {
                                    return [
                                        parts[0].trim().parse::<f32>().unwrap_or(0.0) / 255.0,
                                        parts[1].trim().parse::<f32>().unwrap_or(0.0) / 255.0,
                                        parts[2].trim().parse::<f32>().unwrap_or(0.0) / 255.0,
                                        parts[3].trim().parse::<f32>().unwrap_or(1.0),
                                    ];
                                }
                            } else if string.starts_with('#') && string.len() >= 7 {
                                let r = u8::from_str_radix(&string[1..3], 16).unwrap_or(0) as f32
                                    / 255.0;
                                let g = u8::from_str_radix(&string[3..5], 16).unwrap_or(0) as f32
                                    / 255.0;
                                let b = u8::from_str_radix(&string[5..7], 16).unwrap_or(0) as f32
                                    / 255.0;
                                let a = if string.len() == 9 {
                                    u8::from_str_radix(&string[7..9], 16).unwrap_or(255) as f32
                                        / 255.0
                                } else {
                                    1.0
                                };
                                return [r, g, b, a];
                            }
                        }
                        default
                    };

                    let bg_color = parse_color("backgroundColor", [0.0, 0.0, 0.0, 0.0]);
                    let fg_color = parse_color("color", [1.0, 1.0, 1.0, 1.0]);

                    let font_size =
                        if let Ok(s) = style.get::<&str, std::string::String>("fontSize") {
                            s.trim_end_matches("px")
                                .trim()
                                .parse::<f32>()
                                .unwrap_or(16.0)
                        } else if let Ok(v) = style.get::<&str, f32>("fontSize") {
                            v
                        } else {
                            16.0
                        };

                    ui.update_style(id, t_style, bg_color, fg_color, font_size);
                }
                Ok(())
            },
        )?,
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
