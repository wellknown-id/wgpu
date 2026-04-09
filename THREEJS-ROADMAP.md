# three.js QuickJS Roadmap

Goal: run upstream `three.webgpu.js` under QuickJS with native `wgpu`, for examples that do not fundamentally require browser DOM/UI. Long term, extend that to cover loaders, textures, controls, and as much of the WebGPU example suite as is practical.

## Principles

- Keep HTML support minimal. We only need enough to bootstrap module scripts and optional canvas creation.
- Do not keep growing the current fake renderer shim. Replace it with a real WebGPU JS object model.
- Prefer spec-shaped `navigator.gpu` / `GPUDevice` / `GPUBuffer` / `GPUCanvasContext` objects over ad hoc host callbacks.
- Use upstream `three.webgpu.js` and `three.tsl.js` directly.
- Treat texture and media support as a separate major milestone; pure rendering is the first serious target.

## Runtime Layers

1. HTML and module bootstrap
2. Small browser-lite host environment
3. Real WebGPU JS API backed by native `wgpu`
4. Optional loader / image / input layers

## Browser-Lite Host Environment

Provide the minimum non-DOM globals needed by three.js:

- `globalThis`, `self`, `window`
- `console`
- `performance.now`
- `requestAnimationFrame`, `cancelAnimationFrame`
- `navigator`
- `location`
- a canvas-like object with `getContext("webgpu")`

Avoid general DOM support. If possible, always pass a canvas explicitly so three.js does not try to manufacture one through `document.createElementNS`.

## WebGPU API Milestones

### Phase 1: Renderer Boot

Expose enough API for `three.webgpu.js` to initialize a device and render simple scenes:

- `navigator.gpu.requestAdapter()`
- `navigator.gpu.getPreferredCanvasFormat()`
- `GPUAdapter.features`
- `GPUAdapter.limits`
- `GPUAdapter.requestDevice()`
- `GPUDevice.features`
- `GPUDevice.limits`
- `GPUDevice.queue`
- `GPUDevice.lost`
- `GPUDevice.createBuffer()`
- `GPUDevice.createTexture()`
- `GPUDevice.createSampler()`
- `GPUDevice.createShaderModule()`
- `GPUDevice.createBindGroupLayout()`
- `GPUDevice.createPipelineLayout()`
- `GPUDevice.createBindGroup()`
- `GPUDevice.createRenderPipeline()`
- `GPUDevice.createCommandEncoder()`
- `GPUQueue.submit()`
- `GPUQueue.writeBuffer()`
- `GPUQueue.writeTexture()`
- `GPUCanvasContext.configure()`
- `GPUCanvasContext.getCurrentTexture()`
- `GPUTexture.createView()`

Also expose the constant namespaces three.js expects, at least:

- `GPUBufferUsage`
- `GPUTextureUsage`
- `GPUMapMode`
- `GPUShaderStage`
- `GPUColorWrite`

Validation targets:

- `webgpu_instance_mesh`
- one `webgpu_materials_*` example
- one simple lighting/shadow example

### Phase 2: Broader Render Coverage

Add the remaining render-pass and resource methods needed by more complex scenes and postprocessing:

- render pass state setters
- copy commands
- async / completion helpers like `queue.onSubmittedWorkDone()`
- buffer mapping where needed
- render bundles only if real examples require them

Validation targets:

- more materials examples
- shadow examples
- one postprocessing example

### Phase 3: Compute

Add compute pipeline support:

- `GPUDevice.createComputePipeline()`
- `GPUCommandEncoder.beginComputePass()`
- `GPUComputePassEncoder.setPipeline()`
- `GPUComputePassEncoder.setBindGroup()`
- `GPUComputePassEncoder.dispatchWorkgroups()`

Validation targets:

- `webgpu_compute_particles`
- one additional compute example

### Phase 4: File Loading

Add a small non-browser fetch/file layer for local assets:

- `fetch`
- `Request`
- `Response`
- `Headers`
- `AbortController`

Start with local file-backed loading. Network support is optional.

Validation targets:

- examples that load local JSON/bin assets
- glTF-minus-textures if feasible

### Phase 5: Texture and Image Decode

This is the largest compatibility wall.

Likely requirements:

- `Blob`
- `URL.createObjectURL()` / `URL.revokeObjectURL()`
- `createImageBitmap()` or equivalent decoded-image path
- possibly `ImageData`
- likely enough `Image` / `HTMLImageElement` behavior for loaders

Validation targets:

- texture/material examples
- environment map examples
- glTF demos with textures

### Phase 6: Input and Controls

Route `winit` input into JS events:

- pointer events
- wheel
- keyboard

Validation targets:

- examples using `OrbitControls`
- interaction-heavy examples without DOM UI

## Major Risks

- Image and video support are much harder than pure rendering.
- three.js loader code is browser-shaped and can fail in subtle ways even when the top-level API looks simple.
- Some examples assume browser globals beyond rendering.
- Video/external textures should be considered stretch goals.
- WebXR should stay out of scope.

## Current Strategy

Start by making pure rendering work with real upstream `three.webgpu.js` and a real `navigator.gpu` implementation.

Do not expand the current fake `WebGPURenderer` shim further.

## Compatibility Ladder

Use these as milestones:

1. `webgpu_instance_mesh`
2. `webgpu_materials`
3. `webgpu_shadowmap`
4. `webgpu_postprocessing`
5. `webgpu_compute_particles`
6. `webgpu_loader_gltf`
7. one texture / envmap example

Every new capability should unlock at least one or two concrete examples.
