const GPUBufferUsage = Object.freeze({
  MAP_READ: 0x0001,
  MAP_WRITE: 0x0002,
  COPY_SRC: 0x0004,
  COPY_DST: 0x0008,
  INDEX: 0x0010,
  VERTEX: 0x0020,
  UNIFORM: 0x0040,
  STORAGE: 0x0080,
  INDIRECT: 0x0100,
  QUERY_RESOLVE: 0x0200,
});

const GPUTextureUsage = Object.freeze({
  COPY_SRC: 0x01,
  COPY_DST: 0x02,
  TEXTURE_BINDING: 0x04,
  STORAGE_BINDING: 0x08,
  RENDER_ATTACHMENT: 0x10,
});

const GPUMapMode = Object.freeze({
  READ: 0x0001,
  WRITE: 0x0002,
});

const GPUShaderStage = Object.freeze({
  VERTEX: 0x1,
  FRAGMENT: 0x2,
  COMPUTE: 0x4,
});

const GPUColorWrite = Object.freeze({
  RED: 0x1,
  GREEN: 0x2,
  BLUE: 0x4,
  ALPHA: 0x8,
  ALL: 0xf,
});

const WEBGPU_RUNTIME_VERBOSE = false;

function trace(message) {
  __hostWarn(`[webgpu-runtime] ${message}`);
}

function traceVerbose(message) {
  if (WEBGPU_RUNTIME_VERBOSE) {
    trace(message);
  }
}

class GPUSupportedFeatures extends Set {}

class GPUSupportedLimits {
  constructor(values = {}) {
    Object.assign(this, values);
  }
}

class GPU {
  async requestAdapter(options = {}) {
    trace(`requestAdapter`);
    const handle = __hostGpuRequestAdapter(JSON.stringify(options));
    if (handle === 0 || handle === null || handle === undefined) {
      return null;
    }

    return new GPUAdapter(handle);
  }

  getPreferredCanvasFormat() {
    traceVerbose(`getPreferredCanvasFormat`);
    return __hostGpuGetPreferredCanvasFormat();
  }
}

class GPUAdapter {
  constructor(handle) {
    this.__handle = handle;
    this.features = new GPUSupportedFeatures();
    this.limits = new GPUSupportedLimits();
    this.info = {};
    this.isFallbackAdapter = false;
  }

  async requestDevice(descriptor = {}) {
    trace(`requestDevice`);
    const handle = __hostGpuRequestDevice(this.__handle, JSON.stringify(descriptor));
    return new GPUDevice(handle);
  }
}

class GPUDevice {
  constructor(handle) {
    this.__handle = handle;
    this.features = new GPUSupportedFeatures();
    this.limits = new GPUSupportedLimits();
    this.queue = new GPUQueue(__hostGpuGetQueue(handle));
    this.lost = new Promise(() => {});
  }

  createBuffer(descriptor) {
    trace(`createBuffer ${JSON.stringify(descriptor)}`);
    const handle = __hostGpuCreateBuffer(
      this.__handle,
      JSON.stringify(descriptor),
    );
    return new GPUBuffer(handle, descriptor.size ?? 0);
  }

  createTexture(descriptor) {
    trace(`createTexture ${JSON.stringify(descriptor)}`);
    const handle = __hostGpuCreateTexture(
      this.__handle,
      JSON.stringify(descriptor),
    );
    return new GPUTexture(handle, descriptor);
  }

  createSampler(_descriptor = {}) {
    trace(`createSampler ${JSON.stringify(_descriptor)}`);
    const handle = __hostGpuCreateSampler(
      this.__handle,
      JSON.stringify(_descriptor),
    );
    return new GPUSampler(handle);
  }

  createShaderModule(_descriptor) {
    trace(`createShaderModule`);
    const handle = __hostGpuCreateShaderModule(
      this.__handle,
      JSON.stringify(_descriptor),
    );
    return new GPUShaderModule(handle);
  }

  createBindGroupLayout(descriptor) {
    trace(`createBindGroupLayout ${JSON.stringify(descriptor)}`);
    const handle = __hostGpuCreateBindGroupLayout(
      this.__handle,
      JSON.stringify(descriptor),
    );
    return new GPUBindGroupLayout(handle, descriptor.entries || []);
  }

  createPipelineLayout(descriptor) {
    trace(`createPipelineLayout ${JSON.stringify(descriptor)}`);
    const handle = __hostGpuCreatePipelineLayout(
      this.__handle,
      JSON.stringify({
        bindGroupLayouts: (descriptor.bindGroupLayouts || []).map(
          (layout) => layout.__handle,
        ),
      }),
    );
    return new GPUPipelineLayout(handle);
  }

  createBindGroup(descriptor) {
    trace(`createBindGroup ${JSON.stringify(descriptor)}`);
    const payload = {
      layout: descriptor.layout.__handle,
      entries: (descriptor.entries || []).map((entry) => ({
        binding: entry.binding,
        resource: serializeBindingResource(
          entry.resource,
          bindingKindForLayoutEntry(descriptor.layout, entry.binding),
        ),
      })),
    };
    trace(`createBindGroup payload ${JSON.stringify(payload)}`);
    const handle = __hostGpuCreateBindGroup(
      this.__handle,
      JSON.stringify(payload),
    );
    return new GPUBindGroup(handle);
  }

  createRenderPipeline(_descriptor) {
    trace(`createRenderPipeline`);
    trace(`createRenderPipeline raw targets ${JSON.stringify(_descriptor.fragment?.targets)}`);
    const payload = serializeRenderPipelineDescriptor(_descriptor);
    trace(`createRenderPipeline payload ${JSON.stringify(payload)}`);
    const handle = __hostGpuCreateRenderPipeline(
      this.__handle,
      JSON.stringify(payload),
    );
    return new GPURenderPipeline(handle);
  }

  createCommandEncoder(_descriptor = {}) {
    traceVerbose(`createCommandEncoder`);
    const handle = __hostGpuCreateCommandEncoder(this.__handle, '{}');
    return new GPUCommandEncoder(handle);
  }

  createRenderBundleEncoder(descriptor = {}) {
    trace(`createRenderBundleEncoder`);
    const handle = __hostGpuCreateRenderBundleEncoder(
      this.__handle,
      JSON.stringify(descriptor),
    );
    return new GPURenderBundleEncoder(handle);
  }

  pushErrorScope(_filter) {
    trace(`pushErrorScope`);
  }

  async popErrorScope() {
    trace(`popErrorScope`);
    return null;
  }
}

class GPUQueue {
  constructor(handle) {
    this.__handle = handle;
  }

  submit(commandBuffers) {
    traceVerbose(`queue.submit`);
    const handles = commandBuffers.map((commandBuffer) => commandBuffer.__handle);
    __hostGpuQueueSubmit(this.__handle, JSON.stringify(handles));
  }

  writeBuffer(buffer, bufferOffset, data, dataOffset = 0, size) {
    traceVerbose(`queue.writeBuffer`);
    const view = asU8View(data, dataOffset, size);
    __hostGpuQueueWriteBuffer(
      this.__handle,
      buffer.__handle,
      bufferOffset,
      Array.from(view),
    );
  }

  writeTexture(destination, data, dataLayout, size) {
    trace(`queue.writeTexture`);
    const view = asU8View(data);
    __hostGpuQueueWriteTexture(
      this.__handle,
      JSON.stringify({
        destination: {
          texture: destination.texture.__handle,
          mipLevel: destination.mipLevel ?? 0,
          origin: destination.origin ?? [0, 0, 0],
          aspect: destination.aspect ?? 'all',
        },
        data: Array.from(view),
        dataLayout: {
          offset: dataLayout?.offset ?? 0,
          bytesPerRow: dataLayout?.bytesPerRow ?? null,
          rowsPerImage: dataLayout?.rowsPerImage ?? null,
        },
        size,
      }),
    );
  }

  copyExternalImageToTexture(source, destination, size) {
    trace(`queue.copyExternalImageToTexture`);
    __hostGpuQueueCopyExternalImageToTexture(
      this.__handle,
      JSON.stringify({
        source: {
          source: source.source.__imageId,
          flipY: source.flipY ?? false,
        },
        destination: {
          texture: destination.texture.__handle,
          mipLevel: destination.mipLevel ?? 0,
          origin: destination.origin ?? [0, 0, 0],
          aspect: destination.aspect ?? 'all',
          premultipliedAlpha: destination.premultipliedAlpha ?? false,
        },
        size,
      }),
    );
  }

  async onSubmittedWorkDone() {
    return undefined;
  }
}

class GPUTexture {
  constructor(handle, descriptor = {}) {
    this.__handle = handle;
    const size = normalizeTextureSize(descriptor.size);
    this.width = size.width;
    this.height = size.height;
    this.depthOrArrayLayers = size.depthOrArrayLayers;
    this.mipLevelCount = descriptor.mipLevelCount ?? 1;
    this.sampleCount = descriptor.sampleCount ?? 1;
    this.dimension = descriptor.dimension ?? '2d';
    this.format = descriptor.format;
    this.usage = descriptor.usage ?? 0;
    this.textureBindingViewDimension =
      descriptor.textureBindingViewDimension ??
      defaultTextureBindingViewDimension(
        this.dimension,
        this.depthOrArrayLayers,
      );
  }

  createView(descriptor = {}) {
    const handle = __hostGpuTextureCreateView(
      this.__handle,
      JSON.stringify(descriptor),
    );
    traceVerbose(`texture.createView from_tex=${this.__handle} new_view=${handle} desc=${JSON.stringify(descriptor)}`);
    console.warn(`[JS] texture.createView from_tex=${this.__handle} new_view=${handle} desc=${JSON.stringify(descriptor)}`);
    const view = new GPUTextureView(handle);
    view.dimension = descriptor.dimension ?? this.textureBindingViewDimension;
    return view;
  }

  destroy() {
    trace(`texture.destroy`);
  }
}

class ImageBitmap {
  constructor(imageId, width, height) {
    this.__imageId = imageId;
    this.width = width;
    this.height = height;
  }
}

class Blob {
  constructor(parts = [], options = {}) {
    this.__parts = parts;
    this.type = options.type ?? '';
    this.__path = parts.length === 1 && typeof parts[0] === 'string' ? parts[0] : null;
    this.__bytes = flattenBlobParts(parts);
  }

  get size() {
    return this.__bytes.byteLength;
  }

  async arrayBuffer() {
    return this.__bytes.buffer.slice(
      this.__bytes.byteOffset,
      this.__bytes.byteOffset + this.__bytes.byteLength,
    );
  }

  async text() {
    return new TextDecoder().decode(this.__bytes);
  }
}

const blobUrls = new Map();
let nextBlobUrlId = 1;

class ProgressEvent {
  constructor(type, init = {}) {
    this.type = type;
    this.lengthComputable = init.lengthComputable ?? false;
    this.loaded = init.loaded ?? 0;
    this.total = init.total ?? 0;
  }
}

class ReadableStream {
  constructor(source = {}) {
    this.__chunks = [];
    this.__error = null;
    this.__closed = false;
    this.__donePromise = new Promise((resolve) => {
      this.__resolveDone = resolve;
    });

    if (typeof source.start === 'function') {
      source.start({
        enqueue: (chunk) => {
          this.__chunks.push(toU8Bytes(chunk));
        },
        close: () => {
          this.__closed = true;
          this.__resolveDone();
        },
        error: (error) => {
          this.__error = error;
          this.__resolveDone();
        },
      });
    } else {
      this.__closed = true;
      this.__resolveDone();
    }
  }

  getReader() {
    let index = 0;
    return {
      read: async () => {
        while (index >= this.__chunks.length && !this.__closed && !this.__error) {
          await this.__donePromise;
        }

        if (this.__error) {
          throw this.__error;
        }

        if (index < this.__chunks.length) {
          return { done: false, value: this.__chunks[index++] };
        }

        return { done: true, value: undefined };
      },
    };
  }
}

class DOMParser {
  parseFromString(text, mimeType) {
    return { textContent: text, mimeType };
  }
}

class TextEncoder {
  encode(text = '') {
    const encoded = unescape(encodeURIComponent(String(text)));
    const out = new Uint8Array(encoded.length);
    for (let i = 0; i < encoded.length; i++) {
      out[i] = encoded.charCodeAt(i);
    }
    return out;
  }
}

class TextDecoder {
  constructor(_label = 'utf-8') {}

  decode(input = new Uint8Array()) {
    const bytes = toU8Bytes(input);
    let encoded = '';
    for (let i = 0; i < bytes.length; i++) {
      encoded += String.fromCharCode(bytes[i]);
    }
    return decodeURIComponent(escape(encoded));
  }
}

class Headers {
  constructor(init = undefined) {
    this.__map = new Map();

    if (init instanceof Headers) {
      for (const [key, value] of init.entries()) {
        this.set(key, value);
      }
      return;
    }

    if (Array.isArray(init)) {
      for (const [key, value] of init) {
        this.set(key, value);
      }
      return;
    }

    if (init && typeof init === 'object') {
      for (const key of Object.keys(init)) {
        this.set(key, init[key]);
      }
    }
  }

  append(name, value) {
    this.set(name, value);
  }

  delete(name) {
    this.__map.delete(String(name).toLowerCase());
  }

  get(name) {
    const value = this.__map.get(String(name).toLowerCase());
    return value === undefined ? null : value;
  }

  has(name) {
    return this.__map.has(String(name).toLowerCase());
  }

  set(name, value) {
    this.__map.set(String(name).toLowerCase(), String(value));
  }

  entries() {
    return this.__map.entries();
  }

  [Symbol.iterator]() {
    return this.entries();
  }
}

class AbortSignal {
  constructor() {
    this.aborted = false;
    this.reason = undefined;
    this.__sources = null;
  }

  static any(signals = []) {
    const signal = new AbortSignal();
    signal.__sources = signals;
    signal.aborted = signals.some((candidate) => candidate?.aborted);
    signal.reason = signals.find((candidate) => candidate?.aborted)?.reason;
    return signal;
  }
}

class AbortController {
  constructor() {
    this.signal = new AbortSignal();
  }

  abort(reason = undefined) {
    this.signal.aborted = true;
    this.signal.reason = reason;
  }
}

class Request {
  constructor(input, init = {}) {
    const source = input instanceof Request ? input : null;
    this.url = source ? source.url : String(input);
    this.headers = new Headers(init.headers ?? source?.headers);
    this.credentials = init.credentials ?? source?.credentials ?? 'same-origin';
    this.signal = init.signal ?? source?.signal ?? null;
  }
}

class Response {
  constructor(input, init = {}) {
    if (input instanceof ReadableStream) {
      this.url = init.url ?? '';
      this.ok = init.ok ?? true;
      this.status = init.status ?? 200;
      this.statusText = init.statusText ?? 'OK';
      this.headers = new Headers(init.headers);
      this.body = input;
      this.__bytesPromise = readStreamBytes(input);
      return;
    }

    this.url = input;
    this.ok = init.ok ?? true;
    this.status = init.status ?? 200;
    this.statusText = init.statusText ?? 'OK';
    this.headers = new Headers(init.headers);
    this.body = createResponseBody(init.bytes);
    this.__bytesPromise = Promise.resolve(asResponseBytes(input, init.bytes));
  }

  async text() {
    return new TextDecoder().decode(await this.__bytesPromise);
  }

  async arrayBuffer() {
    const bytes = await this.__bytesPromise;
    return bytes.buffer.slice(
      bytes.byteOffset,
      bytes.byteOffset + bytes.byteLength,
    );
  }

  async json() {
    return JSON.parse(await this.text());
  }

  async blob() {
    return new Blob([await this.__bytesPromise], { type: this.headers.get('content-type') ?? '' });
  }
}

function createResponseBody(bytes) {
  if (bytes === undefined) {
    return undefined;
  }

  const chunk = toU8Bytes(bytes);
  return {
    getReader() {
      let done = false;
      return {
        async read() {
          if (done) {
            return { done: true, value: undefined };
          }

          done = true;
          return { done: false, value: chunk };
        },
      };
    },
  };
}

async function readStreamBytes(stream) {
  const reader = stream.getReader();
  const chunks = [];
  let total = 0;

  while (true) {
    const { done, value } = await reader.read();
    if (done) {
      break;
    }

    const chunk = toU8Bytes(value);
    chunks.push(chunk);
    total += chunk.byteLength;
  }

  const out = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    out.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return out;
}

function abortError(reason = undefined) {
  const error = new Error(reason ?? 'The operation was aborted');
  error.name = 'AbortError';
  return error;
}

function isAborted(signal) {
  if (!signal) {
    return false;
  }

  if (signal.aborted) {
    return true;
  }

  return Array.isArray(signal.__sources)
    ? signal.__sources.some((source) => source?.aborted)
    : false;
}

function abortReason(signal) {
  if (!signal) {
    return undefined;
  }

  if (signal.aborted) {
    return signal.reason;
  }

  if (Array.isArray(signal.__sources)) {
    return signal.__sources.find((source) => source?.aborted)?.reason;
  }

  return undefined;
}

async function fetch(input, init = undefined) {
  const request = input instanceof Request ? new Request(input, init) : new Request(input, init);
  if (isAborted(request.signal)) {
    throw abortError(abortReason(request.signal));
  }

  if (request.url.startsWith('blob:')) {
    const blob = blobUrls.get(request.url);
    if (!blob) {
      return new Response(request.url, {
        ok: false,
        status: 404,
        statusText: 'Not Found',
      });
    }

    return new Response(request.url, {
      headers: blob.type ? { 'content-type': blob.type } : undefined,
      bytes: blob.__bytes,
    });
  }

  return new Response(request.url, {
    bytes: new Uint8Array(__hostReadBytes(request.url)),
  });
}

async function createImageBitmap(source) {
  let imageId;
  if (source instanceof Blob && source.__path !== null) {
    imageId = __hostLoadImage(source.__path);
  } else if (source instanceof Blob) {
    imageId = __hostLoadImageBytes(Array.from(source.__bytes));
  } else {
    imageId = __hostLoadImage(source);
  }
  const [width, height] = __hostGetImageSize(imageId);
  return new ImageBitmap(imageId, width, height);
}

function flattenBlobParts(parts) {
  const chunks = [];
  let total = 0;

  for (const part of parts) {
    const bytes = toU8Bytes(part);
    chunks.push(bytes);
    total += bytes.byteLength;
  }

  const out = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    out.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return out;
}

function toU8Bytes(value) {
  if (value instanceof Uint8Array) {
    return value;
  }

  if (ArrayBuffer.isView(value)) {
    return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  }

  if (value instanceof ArrayBuffer) {
    return new Uint8Array(value);
  }

  if (value instanceof Blob) {
    return value.__bytes;
  }

  if (typeof value === 'string') {
    return new TextEncoder().encode(value);
  }

  return new TextEncoder().encode(String(value));
}

function asResponseBytes(url, bytes) {
  if (bytes instanceof Uint8Array) {
    return bytes;
  }

  if (ArrayBuffer.isView(bytes)) {
    return new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }

  if (bytes instanceof ArrayBuffer) {
    return new Uint8Array(bytes);
  }

  if (typeof url === 'string' && url.startsWith('blob:')) {
    return new Uint8Array();
  }

  return new TextEncoder().encode(__hostReadText(url));
}

const URL = {
  createObjectURL(blob) {
    const url = `blob:quickjs-${nextBlobUrlId++}`;
    blobUrls.set(url, blob);
    return url;
  },

  revokeObjectURL(url) {
    blobUrls.delete(String(url));
  },
};

class GPUBuffer {
  constructor(handle, size) {
    this.__handle = handle;
    this.size = size;
    this.__mappedRange = null;
  }

  getMappedRange(offset = 0, size = this.size - offset) {
    trace(`buffer.getMappedRange`);
    if (this.__mappedRange === null) {
      const values = __hostGpuBufferGetMappedRange(this.__handle, 0, this.size);
      this.__mappedRange = new Uint8Array(values).buffer;
    }
    if (offset === 0 && size === this.size) {
      return this.__mappedRange;
    }

    trace(`buffer.getMappedRange fallback copy offset=${offset} size=${size}`);
    return this.__mappedRange.slice(offset, offset + size);
  }

  unmap() {
    trace(`buffer.unmap`);
    if (this.__mappedRange !== null) {
      __hostGpuBufferSetMappedRange(
        this.__handle,
        Array.from(new Uint8Array(this.__mappedRange)),
      );
      this.__mappedRange = null;
    }
    __hostGpuBufferUnmap(this.__handle);
  }
}

class GPUBindGroupLayout {
  constructor(handle, entries) {
    this.__handle = handle;
    this.__entries = entries || [];
    this.__bindingKinds = new Map(
      (entries || []).map((entry) => [
        entry.binding,
        entry.buffer
          ? 'buffer'
          : entry.sampler
            ? 'sampler'
            : entry.texture
              ? 'texture'
              : null,
      ]),
    );
  }
}

class GPUPipelineLayout {
  constructor(handle) {
    this.__handle = handle;
  }
}

class GPUBindGroup {
  constructor(handle) {
    this.__handle = handle;
  }
}

class GPUShaderModule {
  constructor(handle) {
    this.__handle = handle;
  }

  async getCompilationInfo() {
    return { messages: [] };
  }
}

class GPURenderPipeline {
  constructor(handle) {
    this.__handle = handle;
  }

  getBindGroupLayout(index) {
    const info = JSON.parse(
      __hostGpuRenderPipelineGetBindGroupLayout(this.__handle, index),
    );
    return new GPUBindGroupLayout(info.handle, info.entries || []);
  }
}

class GPUSampler {
  constructor(handle) {
    this.__handle = handle;
    this.__kind = 'sampler';
  }
}

class GPUTextureView {
  constructor(handle) {
    this.__handle = handle;
    this.__kind = 'textureView';
  }
}

class GPUCommandEncoder {
  constructor(handle) {
    this.__handle = handle;
  }

  beginRenderPass(descriptor) {
    traceVerbose(`beginRenderPass`);
    const handle = __hostGpuBeginRenderPass(
      this.__handle,
      JSON.stringify(serializeRenderPassDescriptor(descriptor)),
    );
    return new GPURenderPassEncoder(handle);
  }

  finish() {
    traceVerbose(`commandEncoder.finish`);
    const handle = __hostGpuCommandEncoderFinish(this.__handle);
    return new GPUCommandBuffer(handle);
  }
}

class GPURenderPassEncoder {
  constructor(handle) {
    this.__handle = handle;
  }

  setPipeline(pipeline) {
    traceVerbose(`pass.setPipeline`);
    __hostGpuRenderPassSetPipeline(this.__handle, pipeline.__handle);
  }

  setBindGroup(index, group) {
    traceVerbose(`pass.setBindGroup`);
    __hostGpuRenderPassSetBindGroup(this.__handle, index, group.__handle);
  }

  setIndexBuffer(buffer, format) {
    traceVerbose(`pass.setIndexBuffer`);
    __hostGpuRenderPassSetIndexBuffer(this.__handle, buffer.__handle, format);
  }

  setVertexBuffer(slot, buffer) {
    traceVerbose(`pass.setVertexBuffer`);
    __hostGpuRenderPassSetVertexBuffer(this.__handle, slot, buffer.__handle);
  }

  setViewport(_x, _y, _width, _height, _minDepth, _maxDepth) {
    traceVerbose(`pass.setViewport`);
    __hostGpuRenderPassSetViewport(
      this.__handle,
      _x,
      _y,
      _width,
      _height,
      _minDepth,
      _maxDepth,
    );
  }

  setScissorRect(_x, _y, _width, _height) {
    traceVerbose(`pass.setScissorRect`);
    __hostGpuRenderPassSetScissorRect(
      this.__handle,
      _x,
      _y,
      _width,
      _height,
    );
  }

  setStencilReference(_reference) {
    traceVerbose(`pass.setStencilReference`);
    __hostGpuRenderPassSetStencilReference(this.__handle, _reference);
  }

  draw(vertexCount, instanceCount = 1, firstVertex = 0, firstInstance = 0) {
    traceVerbose(`pass.draw`);
    __hostGpuRenderPassDraw(
      this.__handle,
      vertexCount,
      instanceCount,
      firstVertex,
      firstInstance,
    );
  }

  drawIndexed(
    indexCount,
    instanceCount = 1,
    firstIndex = 0,
    baseVertex = 0,
    firstInstance = 0,
  ) {
    traceVerbose(`pass.drawIndexed`);
    __hostGpuRenderPassDrawIndexed(
      this.__handle,
      indexCount,
      instanceCount,
      firstIndex,
      baseVertex,
      firstInstance,
    );
  }

  end() {
    traceVerbose(`pass.end`);
    __hostGpuRenderPassEnd(this.__handle);
  }

  executeBundles(bundles) {
    traceVerbose(`pass.executeBundles`);
    __hostGpuRenderPassExecuteBundles(
      this.__handle,
      JSON.stringify(bundles.map((bundle) => bundle.__handle)),
    );
  }
}

class GPUCommandBuffer {
  constructor(handle) {
    this.__handle = handle;
  }
}

class GPURenderBundleEncoder {
  constructor(handle) {
    this.__handle = handle;
  }

  setPipeline(pipeline) {
    trace(`bundle.setPipeline`);
    __hostGpuRenderBundleSetPipeline(this.__handle, pipeline.__handle);
  }

  setBindGroup(index, group) {
    trace(`bundle.setBindGroup`);
    __hostGpuRenderBundleSetBindGroup(this.__handle, index, group.__handle);
  }

  draw(vertexCount, instanceCount = 1, firstVertex = 0, firstInstance = 0) {
    trace(`bundle.draw`);
    __hostGpuRenderBundleDraw(
      this.__handle,
      vertexCount,
      instanceCount,
      firstVertex,
      firstInstance,
    );
  }

  finish() {
    trace(`bundle.finish`);
    const handle = __hostGpuRenderBundleFinish(this.__handle);
    return new GPURenderBundle(handle);
  }
}

class GPURenderBundle {
  constructor(handle) {
    this.__handle = handle;
  }
}

function serializeRenderPassDescriptor(descriptor) {
  return {
    colorAttachments: (descriptor.colorAttachments || []).map((attachment) =>
      attachment
        ? {
            view: attachment.view ? attachment.view.__handle : null,
            resolveTarget: attachment.resolveTarget
              ? attachment.resolveTarget.__handle
              : null,
            clearValue: attachment.clearValue || null,
            loadOp: attachment.loadOp ?? null,
            storeOp: attachment.storeOp ?? null,
          }
        : null,
    ),
    depthStencilAttachment: descriptor.depthStencilAttachment
      ? {
          view: descriptor.depthStencilAttachment.view
            ? descriptor.depthStencilAttachment.view.__handle
            : null,
          depthClearValue: descriptor.depthStencilAttachment.depthClearValue ?? null,
          depthLoadOp: descriptor.depthStencilAttachment.depthLoadOp ?? null,
          depthStoreOp: descriptor.depthStencilAttachment.depthStoreOp ?? null,
          stencilClearValue:
            descriptor.depthStencilAttachment.stencilClearValue ?? null,
          stencilLoadOp: descriptor.depthStencilAttachment.stencilLoadOp ?? null,
          stencilStoreOp: descriptor.depthStencilAttachment.stencilStoreOp ?? null,
        }
      : null,
  };
}

function asU8View(data, dataOffset = 0, size) {
  if (ArrayBuffer.isView(data)) {
    const bytesPerElement = data.BYTES_PER_ELEMENT || 1;
    const byteDataOffset = dataOffset * bytesPerElement;
    const start = data.byteOffset + byteDataOffset;
    const end = size === undefined
      ? data.byteOffset + data.byteLength
      : start + size * bytesPerElement;
    return new Uint8Array(data.buffer.slice(start, end));
  }

  if (data instanceof ArrayBuffer) {
    const start = dataOffset;
    const end = size === undefined ? data.byteLength : start + size;
    return new Uint8Array(data.slice(start, end));
  }

  throw new TypeError('Unsupported writeBuffer data source');
}

function serializeRenderPipelineDescriptor(descriptor) {
  return {
    layout:
      descriptor.layout === 'auto'
        ? 'auto'
        : descriptor.layout
          ? descriptor.layout.__handle
          : null,
    vertex: {
      module: descriptor.vertex.module.__handle,
      entryPoint: descriptor.vertex.entryPoint,
      buffers: (descriptor.vertex.buffers || []).map((buffer) => ({
        arrayStride: buffer.arrayStride,
        stepMode: buffer.stepMode,
        attributes: (buffer.attributes || []).map((attribute) => ({
          shaderLocation: attribute.shaderLocation,
          offset: attribute.offset,
          format: attribute.format,
        })),
      })),
    },
    fragment: descriptor.fragment
      ? {
          module: descriptor.fragment.module.__handle,
          entryPoint: descriptor.fragment.entryPoint,
          targets: (descriptor.fragment.targets || []).map((target) =>
            target
              ? {
                  format: target.format,
                  isCanvasTarget: target.format === navigator.gpu.getPreferredCanvasFormat(),
                  writeMask: target.writeMask,
                }
              : null,
          ),
        }
      : null,
    primitive: descriptor.primitive || null,
    depthStencil: descriptor.depthStencil || null,
    multisample: descriptor.multisample || null,
  };
}

function serializeBindingResource(resource, expectedKind) {
  if (!resource) {
    return null;
  }

  if (resource.buffer) {
    return {
      kind: 'buffer',
      buffer: resource.buffer.__handle,
      offset: resource.offset ?? 0,
      size: resource.size ?? null,
    };
  }

  if (resource.__kind === 'textureView' || expectedKind === 'texture') {
    return {
      kind: 'textureView',
      textureView: resource.__handle,
    };
  }

  if (resource.__kind === 'sampler' || expectedKind === 'sampler') {
    return {
      kind: 'sampler',
      sampler: resource.__handle,
    };
  }

  throw new TypeError(`Unsupported bind group resource: ${Object.prototype.toString.call(resource)}`);
}

function bindingKindForLayoutEntry(layout, binding) {
  if (layout.__bindingKinds?.get) {
    const kind = layout.__bindingKinds.get(binding);
    if (kind) {
      return kind;
    }
  }

  const entry = (layout.__entries || []).find((entry) => entry.binding === binding);
  if (!entry) {
    return null;
  }

  if (entry.buffer) {
    return 'buffer';
  }
  if (entry.sampler) {
    return 'sampler';
  }
  if (entry.texture) {
    return 'texture';
  }

  return null;
}

class GPUCanvasContext {
  constructor(handle, canvas) {
    this.__handle = handle;
    this.canvas = canvas;
    this.__format = null;
  }

  configure(configuration) {
    trace(`context.configure`);
    this.__format = configuration.format;
    __hostGpuConfigureCanvasContext(
      this.__handle,
      configuration.device.__handle,
      JSON.stringify({
        ...configuration,
        device: undefined,
      }),
    );
  }

  getCurrentTexture() {
    traceVerbose(`context.getCurrentTexture`);
    const handle = __hostGpuGetCurrentTexture(this.__handle);
    return new GPUTexture(handle, {
      size: {
        width: this.canvas?.width ?? globalThis.innerWidth,
        height: this.canvas?.height ?? globalThis.innerHeight,
        depthOrArrayLayers: 1,
      },
      mipLevelCount: 1,
      sampleCount: 1,
      dimension: '2d',
      format: this.__format ?? navigator.gpu.getPreferredCanvasFormat(),
      usage: GPUTextureUsage.RENDER_ATTACHMENT,
    });
  }
}

function normalizeTextureSize(size) {
  if (Array.isArray(size)) {
    return {
      width: size[0] ?? 1,
      height: size[1] ?? 1,
      depthOrArrayLayers: size[2] ?? 1,
    };
  }

  return {
    width: size?.width ?? 1,
    height: size?.height ?? 1,
    depthOrArrayLayers: size?.depthOrArrayLayers ?? 1,
  };
}

function defaultTextureBindingViewDimension(dimension, depthOrArrayLayers) {
  if (dimension === '1d') {
    return '1d';
  }

  if (dimension === '3d') {
    return '3d';
  }

  return depthOrArrayLayers > 1 ? '2d-array' : '2d';
}

function createEventTarget(parent = null) {
  const listeners = new Map();

  return {
    __listeners: listeners,
    __parent: parent,
    addEventListener(type, listener) {
      if (typeof listener !== 'function') {
        return;
      }
      const list = listeners.get(type) || [];
      list.push(listener);
      listeners.set(type, list);
    },
    removeEventListener(type, listener) {
      const list = listeners.get(type) || [];
      listeners.set(type, list.filter((candidate) => candidate !== listener));
    },
    dispatchEvent(event) {
      const type = event?.type;
      if (!type) {
        return true;
      }
      const payload = {
        preventDefault() {},
        ...event,
        target: event?.target ?? this,
        currentTarget: this,
      };
      for (const listener of listeners.get(type) || []) {
        listener.call(this, payload);
      }
      return true;
    },
    getRootNode() {
      return this.__parent?.getRootNode ? this.__parent.getRootNode() : this;
    },
  };
}

function createCanvasElement() {
  const context = new GPUCanvasContext(__hostGpuCreateCanvasContext(), null);
  const root = globalThis.document ?? createEventTarget();
  const canvas = {
    ...createEventTarget(root),
    tagName: 'CANVAS',
    style: {},
    width: globalThis.innerWidth,
    height: globalThis.innerHeight,
    clientWidth: globalThis.innerWidth,
    clientHeight: globalThis.innerHeight,
    ownerDocument: root,
    setPointerCapture() {},
    releasePointerCapture() {},
    getBoundingClientRect() {
      return {
        left: 0,
        top: 0,
        width: this.clientWidth,
        height: this.clientHeight,
      };
    },
    getContext(kind) {
      if (kind !== 'webgpu') {
        return null;
      }

      context.canvas = canvas;
      return context;
    },
  };

  return canvas;
}

export function installWebGPURuntime() {
  globalThis.GPUBufferUsage = GPUBufferUsage;
  globalThis.GPUTextureUsage = GPUTextureUsage;
  globalThis.GPUMapMode = GPUMapMode;
  globalThis.GPUShaderStage = GPUShaderStage;
  globalThis.GPUColorWrite = GPUColorWrite;
  globalThis.Headers = Headers;
  globalThis.Request = Request;
  globalThis.AbortSignal = AbortSignal;
  globalThis.AbortController = AbortController;
  globalThis.ProgressEvent = ProgressEvent;
  globalThis.ReadableStream = ReadableStream;
  globalThis.DOMParser = DOMParser;
  globalThis.TextEncoder = TextEncoder;
  globalThis.TextDecoder = TextDecoder;
  globalThis.URL = URL;
  globalThis.Blob = Blob;
  globalThis.Response = Response;
  globalThis.fetch = fetch;
  globalThis.createImageBitmap = createImageBitmap;
  globalThis.ImageBitmap = ImageBitmap;
  globalThis.setTimeout = (callback, _delay = 0) => {
    Promise.resolve().then(callback);
    return 0;
  };
  globalThis.clearTimeout = () => {};

  if (!globalThis.navigator) {
    globalThis.navigator = {};
  }

  globalThis.navigator.gpu = new GPU();
  globalThis.__createCanvasElement = createCanvasElement;
}

installWebGPURuntime();
