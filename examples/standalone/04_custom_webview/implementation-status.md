# Custom Webview - Implementation Status

This document tracks the web platform APIs polyfilled by the JS bridge
(`src/js_bridge.rs`) and known gaps.

The HTML demo pages use only standard web APIs. The `__host*` functions are
internal plumbing hidden behind the polyfill layer.

## DOM

| API                                    | Status  | Notes                                                     |
| -------------------------------------- | ------- | --------------------------------------------------------- |
| `document.getElementById(id)`          | Done    | Returns cached element proxy (identity-stable)            |
| `document.addEventListener(event, cb)` | Done    | Global event listeners                                    |
| `document.createElement(tag)`          | Stub    | Creates a detached element; cannot be added to live DOM   |
| `elem.textContent` (get/set)           | Done    | Getter returns last set value                             |
| `elem.style.X = val`                   | Done    | Proxy converts camelCase to CSS property names            |
| `elem.addEventListener(event, cb)`     | Done    | Per-element listeners, supports event bubbling            |
| `elem.setPointerCapture(id)`           | Done    |                                                           |
| `elem.releasePointerCapture(id)`       | Done    |                                                           |
| `elem.getBoundingClientRect()`         | Done    | Returns `{x, y, width, height, left, top, right, bottom}` |
| `elem.appendChild()`                   | Missing |                                                           |
| `elem.removeChild()`                   | Missing |                                                           |
| `elem.innerHTML`                       | Missing |                                                           |
| `elem.parentNode` / `children`         | Missing |                                                           |

## Events

| API                                               | Status  | Notes                            |
| ------------------------------------------------- | ------- | -------------------------------- |
| `pointerdown`                                     | Done    |                                  |
| `pointerup`                                       | Done    |                                  |
| `pointermove`                                     | Done    |                                  |
| `click`                                           | Done    | Synthesized from pointer events  |
| Event: `clientX`, `clientY`                       | Done    |                                  |
| Event: `offsetX`, `offsetY`                       | Done    | Relative to target element       |
| Event: `button`, `buttons`                        | Done    |                                  |
| Event: `currentTarget`                            | Done    | Set during bubbling              |
| Event: `target`                                   | Done    | Points to cached element object  |
| Event: `ctrlKey`, `shiftKey`, `altKey`, `metaKey` | Done    | Always false (no keyboard state) |
| Event: `stopPropagation()`                        | Done    |                                  |
| Event: `preventDefault()`                         | Done    | No-op (no default behaviors)     |
| `keydown` / `keyup`                               | Missing | No keyboard input support        |
| `input`                                           | Missing | No text input elements           |
| `focus` / `blur`                                  | Missing |                                  |

## Timers

| API                         | Status  | Notes |
| --------------------------- | ------- | ----- |
| `requestAnimationFrame(cb)` | Done    |       |
| `setTimeout(cb, ms)`        | Missing |       |
| `setInterval(cb, ms)`       | Missing |       |

## Console

| API                | Status | Notes                             |
| ------------------ | ------ | --------------------------------- |
| `console.log(msg)` | Done   | Prints to stdout / Android logcat |

## Canvas 2D (`getContext('2d')`)

| API                                    | Status  | Notes                                         |
| -------------------------------------- | ------- | --------------------------------------------- |
| `fillRect(x, y, w, h)`                 | Done    |                                               |
| `clearRect(x, y, w, h)`                | Done    |                                               |
| `strokeRect(x, y, w, h)`               | Done    |                                               |
| `beginPath()`                          | Done    |                                               |
| `arc(cx, cy, r, start, end)`           | Done    |                                               |
| `moveTo(x, y)`                         | Done    |                                               |
| `lineTo(x, y)`                         | Done    |                                               |
| `closePath()`                          | Done    |                                               |
| `fill()`                               | Done    | Full circles only                             |
| `stroke()`                             | Done    | Lines and arcs                                |
| `fillStyle` (get/set)                  | Done    | Supports `rgb()`, `rgba()`, hex, named colors |
| `strokeStyle` (get/set)                | Done    |                                               |
| `lineWidth` (get/set)                  | Done    |                                               |
| `canvas` (getter)                      | Done    | Returns parent element                        |
| `canvas.width` / `canvas.height` (set) | Done    | Re-creates backing store on resize            |
| `fillText()`                           | Stub    | No-op                                         |
| `strokeText()`                         | Stub    | No-op                                         |
| `save()` / `restore()`                 | Stub    | No-op                                         |
| `translate()` / `rotate()` / `scale()` | Stub    | No-op                                         |
| `drawImage()`                          | Missing |                                               |
| `createLinearGradient()`               | Missing |                                               |

## WebGPU (`navigator.gpu`)

| API                                           | Status  | Notes                                 |
| --------------------------------------------- | ------- | ------------------------------------- |
| `navigator.gpu.requestAdapter()`              | Done    |                                       |
| `adapter.requestDevice()`                     | Done    |                                       |
| `device.createShaderModule(desc)`             | Done    | Real WGSL compilation via wgpu        |
| `device.createRenderPipeline(desc)`           | Done    | Single vertex buffer, limited formats |
| `device.createBuffer(desc)`                   | Done    |                                       |
| `device.queue.writeBuffer(buf, offset, data)` | Done    | Supports `Float32Array`               |
| `device.queue.submit(cmdBufs)`                | Done    |                                       |
| `canvas.getContext('webgpu')`                 | Done    |                                       |
| `context.configure(config)`                   | Done    |                                       |
| `context.getCurrentTexture()`                 | Done    |                                       |
| Bind groups / uniforms                        | Missing |                                       |
| Multiple vertex buffers                       | Missing |                                       |
| Compute pipelines                             | Missing |                                       |
| Texture creation / sampling                   | Missing |                                       |

## WebXR (`navigator.xr`)

Requires the `xr` feature flag.

| API                                     | Status  | Notes                         |
| --------------------------------------- | ------- | ----------------------------- |
| `navigator.xr.isSessionSupported(mode)` | Done    | `immersive-vr` only           |
| `navigator.xr.requestSession(mode)`     | Done    | Creates native OpenXR session |
| `session.requestReferenceSpace(type)`   | Done    | Returns stub                  |
| `session.requestAnimationFrame(cb)`     | Done    |                               |
| `session.addEventListener('end', cb)`   | Done    |                               |
| `session.end()`                         | Done    | Tears down OpenXR session     |
| `frame.getViewerPose()`                 | Stub    | Returns identity matrices     |
| `XRWebGLLayer`                          | Missing |                               |
