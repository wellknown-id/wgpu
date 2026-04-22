use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use anyhow::Result;
use rquickjs::{Context, Function, Runtime};

use crate::types::CanvasDrawOp;
use crate::webgpu_bridge::WebGpuBridge;
use log;

fn unpack_color(packed: u32) -> [f32; 4] {
    [
        ((packed >> 24) & 0xFF) as f32 / 255.0,
        ((packed >> 16) & 0xFF) as f32 / 255.0,
        ((packed >> 8) & 0xFF) as f32 / 255.0,
        (packed & 0xFF) as f32 / 255.0,
    ]
}

// Parse "[[offset,formatCode,size],...]" from JS
fn serde_json_mini_parse_attrs(json: &str) -> Vec<(u64, u32, u32)> {
    let mut result = Vec::new();
    let json = json.trim();
    if json.len() < 2 { return result; }
    let s = &json[1..json.len()-1]; // strip outer []
    let mut depth = 0;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '[' => { if depth == 0 { start = i + 1; } depth += 1; }
            ']' => {
                depth -= 1;
                if depth == 0 {
                    let inner = &s[start..i];
                    let parts: Vec<&str> = inner.split(',').collect();
                    if parts.len() >= 3 {
                        let offset = parts[0].trim().parse().unwrap_or(0);
                        let fmt = parts[1].trim().parse().unwrap_or(0);
                        let size = parts[2].trim().parse().unwrap_or(0);
                        result.push((offset, fmt, size));
                    }
                }
            }
            _ => {}
        }
    }
    result
}

fn vertex_format_from_code(code: u32) -> wgpu::VertexFormat {
    match code {
        3 => wgpu::VertexFormat::Float32x3,
        4 => wgpu::VertexFormat::Float32x4,
        _ => wgpu::VertexFormat::Float32x4,
    }
}

pub struct JsBridge {
    shared: Rc<RefCell<SharedState>>,
    context: Context,
    runtime: Runtime,
}

pub struct SharedState {
    pub dom_dirty: bool,
    pub text_overrides: HashMap<String, String>,
    pub style_overrides: HashMap<String, HashMap<String, String>>,
    pub canvas_ops: HashMap<String, Vec<CanvasDrawOp>>,
    pub pointer_capture: Option<String>,
    pub pointer_down_target: Option<String>,
    pub element_rects: HashMap<String, crate::types::LayoutRect>,
    pub animation_callbacks: Vec<rquickjs::Persistent<rquickjs::Function<'static>>>,
    pub webgpu: Option<WebGpuBridge>,
}

impl JsBridge {
    pub fn new() -> Result<Self> {
        let runtime = Runtime::new()?;
        let context = Context::full(&runtime)?;

        let shared = Rc::new(RefCell::new(SharedState {
            dom_dirty: false,
            text_overrides: HashMap::new(),
            style_overrides: HashMap::new(),
            canvas_ops: HashMap::new(),
            pointer_capture: None,
            pointer_down_target: None,
            element_rects: HashMap::new(),
            animation_callbacks: Vec::new(),
            webgpu: None,
        }));

        Ok(Self {
            shared,
            context,
            runtime,
        })
    }

    pub fn eval_script(&mut self, script: &str) -> Result<()> {
        self.init_and_run(script)
    }

    fn init_and_run(&mut self, script: &str) -> Result<()> {
        let shared = self.shared.clone();
        let script = script.to_string();

        let res = self.context.with(|ctx| -> Result<()> {
            let globals = ctx.globals();

            let console = rquickjs::Object::new(ctx.clone())?;
            console.set(
                "log",
                Function::new(ctx.clone(), |_ctx: rquickjs::Ctx<'_>, msg: String| {
                    println!("[JS] {msg}");
                })?,
            )?;
            globals.set("console", console)?;

            let shared_clone = shared.clone();
            globals.set(
                "__hostSetText",
                Function::new(
                    ctx.clone(),
                    move |_ctx: rquickjs::Ctx<'_>, id: String, text: String| {
                        let mut s = shared_clone.borrow_mut();
                        s.text_overrides.insert(id, text);
                        s.dom_dirty = true;
                    },
                )?,
            )?;

            let shared_clone = shared.clone();
            globals.set(
                "__hostSetStyle",
                Function::new(
                    ctx.clone(),
                    move |_ctx: rquickjs::Ctx<'_>, id: String, prop: String, value: String| {
                        let mut s = shared_clone.borrow_mut();
                        let map = s.style_overrides.entry(id).or_default();
                        if map.get(&prop).map(|v| v.as_str()) != Some(value.as_str()) {
                            map.insert(prop, value);
                            s.dom_dirty = true;
                        }
                    },
                )?,
            )?;

            let shared_clone = shared.clone();
            globals.set(
                "__hostCanvasCreate",
                Function::new(
                    ctx.clone(),
                    move |_ctx: rquickjs::Ctx<'_>, id: String, _w: u32, _h: u32| {
                        let mut s = shared_clone.borrow_mut();
                        s.canvas_ops.entry(id).or_default();
                        s.dom_dirty = true;
                    },
                )?,
            )?;

            let shared_clone = shared.clone();
            globals.set(
                "__hostCanvasClear",
                Function::new(
                    ctx.clone(),
                    move |_ctx: rquickjs::Ctx<'_>, id: String, _rgba: u32| {
                        let mut s = shared_clone.borrow_mut();
                        if let Some(ops) = s.canvas_ops.get_mut(&id) {
                            ops.clear();
                            s.dom_dirty = true;
                        }
                    },
                )?,
            )?;

            let shared_clone = shared.clone();
            globals.set(
                "__hostCanvasFillRect",
                Function::new(
                    ctx.clone(),
                    move |_ctx: rquickjs::Ctx<'_>,
                          id: String,
                          x: i32,
                          y: i32,
                          w: i32,
                          h: i32,
                          rgba: u32| {
                        let mut s = shared_clone.borrow_mut();
                        if let Some(ops) = s.canvas_ops.get_mut(&id) {
                            ops.push(CanvasDrawOp::FillRect {
                                x: x as f32,
                                y: y as f32,
                                w: w as f32,
                                h: h as f32,
                                color: unpack_color(rgba),
                            });
                        }
                    },
                )?,
            )?;

            let shared_clone = shared.clone();
            globals.set(
                "__hostCanvasStrokeRect",
                Function::new(
                    ctx.clone(),
                    move |_ctx: rquickjs::Ctx<'_>,
                          id: String,
                          x: i32,
                          y: i32,
                          w: i32,
                          h: i32,
                          rgba_lw: u64| {
                        let lw = (rgba_lw >> 32) as i32;
                        let rgba = (rgba_lw & 0xFFFFFFFF) as u32;
                        let mut s = shared_clone.borrow_mut();
                        if let Some(ops) = s.canvas_ops.get_mut(&id) {
                            ops.push(CanvasDrawOp::StrokeRect {
                                x: x as f32,
                                y: y as f32,
                                w: w as f32,
                                h: h as f32,
                                color: unpack_color(rgba),
                                line_width: lw as f32,
                            });
                        }
                    },
                )?,
            )?;

            let shared_clone = shared.clone();
            globals.set(
                "__hostCanvasFillCircle",
                Function::new(
                    ctx.clone(),
                    move |_ctx: rquickjs::Ctx<'_>,
                          id: String,
                          cx: i32,
                          cy: i32,
                          radius: i32,
                          rgba: u32| {
                        let mut s = shared_clone.borrow_mut();
                        if let Some(ops) = s.canvas_ops.get_mut(&id) {
                            ops.push(CanvasDrawOp::FillCircle {
                                cx: cx as f32,
                                cy: cy as f32,
                                radius: radius as f32,
                                color: unpack_color(rgba),
                            });
                        }
                    },
                )?,
            )?;

            let shared_clone = shared.clone();
            globals.set(
                "__hostCanvasStrokeCircle",
                Function::new(
                    ctx.clone(),
                    move |_ctx: rquickjs::Ctx<'_>,
                          id: String,
                          cx: i32,
                          cy: i32,
                          radius: i32,
                          rgba: u32,
                          lw: f32| {
                        let mut s = shared_clone.borrow_mut();
                        if let Some(ops) = s.canvas_ops.get_mut(&id) {
                            ops.push(CanvasDrawOp::StrokeCircle {
                                cx: cx as f32,
                                cy: cy as f32,
                                radius: radius as f32,
                                color: unpack_color(rgba),
                                line_width: lw,
                            });
                        }
                    },
                )?,
            )?;

            let shared_clone = shared.clone();
            globals.set(
                "__hostCanvasLine",
                Function::new(
                    ctx.clone(),
                    move |_ctx: rquickjs::Ctx<'_>,
                          id: String,
                          x0: f32,
                          y0: f32,
                          x1: f32,
                          y1: f32,
                          rgba_lw: u64| {
                        let lw = (rgba_lw >> 32) as f32;
                        let rgba = (rgba_lw & 0xFFFFFFFF) as u32;
                        let mut s = shared_clone.borrow_mut();
                        if let Some(ops) = s.canvas_ops.get_mut(&id) {
                            ops.push(CanvasDrawOp::Line {
                                x0,
                                y0,
                                x1,
                                y1,
                                color: unpack_color(rgba),
                                line_width: lw,
                            });
                        }
                    },
                )?,
            )?;

            let shared_clone = shared.clone();
            globals.set(
                "__hostSetPointerCapture",
                Function::new(
                    ctx.clone(),
                    move |_ctx: rquickjs::Ctx<'_>, id: String| {
                        shared_clone.borrow_mut().pointer_capture = Some(id);
                    },
                )?,
            )?;

            let shared_clone = shared.clone();
            globals.set(
                "__hostReleasePointerCapture",
                Function::new(
                    ctx.clone(),
                    move |_ctx: rquickjs::Ctx<'_>| {
                        shared_clone.borrow_mut().pointer_capture = None;
                    },
                )?,
            )?;

            let shared_clone = shared.clone();
            globals.set(
                "__hostGetBoundingRect",
                Function::new(
                    ctx.clone(),
                    move |_ctx: rquickjs::Ctx<'_>, id: String| -> String {
                        let s = shared_clone.borrow();
                        if let Some(r) = s.element_rects.get(&id) {
                            format!("{},{},{},{}", r.x, r.y, r.w, r.h)
                        } else {
                            "0,0,0,0".to_string()
                        }
                    },
                )?,
            )?;

            let shared_clone = shared.clone();
            globals.set(
                "__hostRequestAnimationFrame",
                Function::new(
                    ctx.clone(),
                    move |cb: rquickjs::Function<'_>| {
                        let ctx = cb.ctx().clone();
                        let mut s = shared_clone.borrow_mut();
                        s.animation_callbacks.push(rquickjs::Persistent::save(&ctx, cb));
                    },
                )?,
            )?;

            ctx.eval::<(), _>(
                r#"
                var __listeners = {};
                var __globalListeners = {};

                function __hostAddEventListener(key, cb) {
                    if (!__listeners[key]) __listeners[key] = [];
                    __listeners[key].push(cb);
                }

                var window = globalThis;
                var requestAnimationFrame = function(cb) {
                    __hostRequestAnimationFrame(cb);
                };

                function __dispatchPointerEvent(eventType, idsStr, targetId, x, y) {
                    var ids = idsStr ? idsStr.split(',') : [];
                    var stopped = false;
                    var evt = {
                        type: eventType,
                        clientX: x,
                        clientY: y,
                        pointerId: 1,
                        pointerType: 'mouse',
                        target: {id: targetId},
                        stopPropagation: function() { stopped = true; },
                        preventDefault: function() {}
                    };
                    for (var i = 0; i < ids.length && !stopped; i++) {
                        var key = eventType + ':' + ids[i];
                        var cbs = __listeners[key];
                        if (cbs) {
                            for (var j = 0; j < cbs.length; j++) {
                                cbs[j](evt);
                            }
                        }
                    }
                    if (!stopped) {
                        var gcbs = __globalListeners[eventType] || [];
                        for (var i = 0; i < gcbs.length; i++) {
                            gcbs[i](evt);
                        }
                    }
                }

                function __parseColor(str) {
                    str = str.trim();
                    var m = str.match(/^rgba?\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*(?:,\s*([\d.]+))?\s*\)$/);
                    if (m) return [parseInt(m[1]), parseInt(m[2]), parseInt(m[3]), m[4] !== undefined ? Math.round(Math.min(1, Math.max(0, parseFloat(m[4]))) * 255) : 255];
                    if (str[0] === '#' && str.length === 7) {
                        return [parseInt(str.substr(1,2),16), parseInt(str.substr(3,2),16), parseInt(str.substr(5,2),16), 255];
                    }
                    if (str[0] === '#' && str.length === 4) {
                        var r = parseInt(str[1]+str[1],16), g = parseInt(str[2]+str[2],16), b = parseInt(str[3]+str[3],16);
                        return [r, g, b, 255];
                    }
                    var named = {
                        'white': [255,255,255,255], 'black': [0,0,0,255], 'red': [255,0,0,255],
                        'green': [0,128,0,255], 'blue': [0,0,255,255], 'yellow': [255,255,0,255],
                        'cyan': [0,255,255,255], 'magenta': [255,0,255,255], 'orange': [255,165,0,255],
                        'purple': [128,0,128,255], 'lime': [0,255,0,255], 'pink': [255,192,203,255],
                        'gray': [128,128,128,255], 'grey': [128,128,128,255],
                        'transparent': [0,0,0,0], 'tomato': [255,99,71,255],
                        'coral': [255,127,80,255], 'gold': [255,215,0,255],
                        'dodgerblue': [30,144,255,255], 'steelblue': [70,130,180,255],
                        'darkslategray': [47,79,79,255], 'teal': [0,128,128,255],
                    };
                    if (named[str.toLowerCase()]) return named[str.toLowerCase()];
                    return [0, 0, 0, 255];
                }

                function __packRgba(c) {
                    return ((c[0] & 0xFF) * 16777216) + ((c[1] & 0xFF) * 65536) + ((c[2] & 0xFF) * 256) + (c[3] & 0xFF);
                }

                var document = {
                    addEventListener: function(event, cb) {
                        if (!__globalListeners[event]) __globalListeners[event] = [];
                        __globalListeners[event].push(cb);
                    },
                    getElementById: function(id) {
                        var elem = {
                            id: id,
                            tagName: 'DIV',
                            addEventListener: function(event, cb) {
                                __hostAddEventListener(event + ':' + id, cb);
                            },
                            setPointerCapture: function(pointerId) {
                                __hostSetPointerCapture(id);
                            },
                            releasePointerCapture: function(pointerId) {
                                __hostReleasePointerCapture();
                            },
                            getBoundingClientRect: function() {
                                var s = __hostGetBoundingRect(id);
                                var p = s.split(',');
                                var x = parseFloat(p[0]), y = parseFloat(p[1]);
                                var w = parseFloat(p[2]), h = parseFloat(p[3]);
                                return {x:x, y:y, width:w, height:h, left:x, top:y, right:x+w, bottom:y+h};
                            }
                        };
                        Object.defineProperty(elem, 'textContent', {
                            set: function(v) { __hostSetText(id, '' + v); },
                            get: function() { return ''; }
                        });
                        elem.style = new Proxy({}, {
                            set: function(target, prop, value) {
                                var cssProp = prop.replace(/([A-Z])/g, '-$1').toLowerCase();
                                __hostSetStyle(id, cssProp, '' + value);
                                target[prop] = value;
                                return true;
                            }
                        });
                        elem.getContext = function(type) {
                            if (type === 'webgpu') {
                                elem.tagName = 'CANVAS';
                                var gpuCtx = {
                                    canvasId: id,
                                    _configured: false,
                                    configure: function(config) {
                                        var w = elem.width || 400;
                                        var h = elem.height || 300;
                                        __hostGpuConfigureCanvas(id, w, h);
                                        this._configured = true;
                                    },
                                    getCurrentTexture: function() {
                                        return {
                                            createView: function() { return { __canvasId: id }; }
                                        };
                                    }
                                };
                                return gpuCtx;
                            }
                            if (type !== '2d') return null;
                            elem.tagName = 'CANVAS';
                            var _fillColor = [0, 0, 0, 255];
                            var _strokeColor = [0, 0, 0, 255];
                            var _lineWidth = 1;
                            var _canvasWidth = elem.width || 400;
                            var _canvasHeight = elem.height || 300;
                            __hostCanvasCreate(id, _canvasWidth, _canvasHeight);

                            var ctx2d = {
                                get lineWidth() { return _lineWidth; },
                                set lineWidth(v) { _lineWidth = v; },
                                get fillStyle() { return 'rgba(' + _fillColor.join(',') + ')'; },
                                set fillStyle(v) { _fillColor = __parseColor(v); },
                                get strokeStyle() { return 'rgba(' + _strokeColor.join(',') + ')'; },
                                set strokeStyle(v) { _strokeColor = __parseColor(v); },

                                clearRect: function(x, y, w, h) {
                                    __hostCanvasFillRect(id, x, y, w, h, 0);
                                },
                                fillRect: function(x, y, w, h) {
                                    __hostCanvasFillRect(id, x, y, w, h, __packRgba(_fillColor));
                                },
                                strokeRect: function(x, y, w, h) {
                                    var packed = __packRgba(_strokeColor);
                                    var lw = Math.round(_lineWidth);
                                    __hostCanvasStrokeRect(id, x, y, w, h, lw * 4294967296 + packed);
                                },

                                _pathOps: [],
                                beginPath: function() { this._pathOps = []; },
                                arc: function(cx, cy, r, startAngle, endAngle, ccw) {
                                    this._pathOps.push({type: 'arc', cx: cx, cy: cy, r: r, start: startAngle, end: endAngle, ccw: ccw || false});
                                },
                                moveTo: function(x, y) {
                                    this._pathOps.push({type: 'moveTo', x: x, y: y});
                                },
                                lineTo: function(x, y) {
                                    this._pathOps.push({type: 'lineTo', x: x, y: y});
                                },
                                closePath: function() {
                                    this._pathOps.push({type: 'close'});
                                },
                                fill: function() {
                                    var packed = __packRgba(_fillColor);
                                    for (var i = 0; i < this._pathOps.length; i++) {
                                        var op = this._pathOps[i];
                                        if (op.type === 'arc') {
                                            var fullCircle = Math.abs(op.end - op.start) >= Math.PI * 2 - 0.001;
                                            if (fullCircle) {
                                                __hostCanvasFillCircle(id, Math.round(op.cx), Math.round(op.cy), Math.round(op.r), packed);
                                            }
                                        }
                                    }
                                },
                                stroke: function() {
                                    var packed = __packRgba(_strokeColor);
                                    var pts = [];
                                    for (var i = 0; i < this._pathOps.length; i++) {
                                        var op = this._pathOps[i];
                                        if (op.type === 'arc') {
                                            var fullCircle = Math.abs(op.end - op.start) >= Math.PI * 2 - 0.001;
                                            if (fullCircle) {
                                                __hostCanvasStrokeCircle(id, Math.round(op.cx), Math.round(op.cy), Math.round(op.r), packed, _lineWidth);
                                            } else {
                                                var steps = Math.max(Math.round(Math.abs(op.end - op.start) * op.r * 0.5), 12);
                                                for (var j = 0; j <= steps; j++) {
                                                    var t = op.start + (op.end - op.start) * j / steps;
                                                    pts.push({x: op.cx + Math.cos(t) * op.r, y: op.cy + Math.sin(t) * op.r});
                                                }
                                            }
                                        } else if (op.type === 'moveTo') {
                                            if (pts.length >= 2) {
                                                for (var k = 0; k < pts.length - 1; k++) {
                                                    __hostCanvasLine(id, pts[k].x, pts[k].y, pts[k+1].x, pts[k+1].y, Math.round(_lineWidth) * 4294967296 + packed);
                                                }
                                            }
                                            pts = [{x: op.x, y: op.y}];
                                        } else if (op.type === 'lineTo') {
                                            pts.push({x: op.x, y: op.y});
                                        } else if (op.type === 'close') {
                                            if (pts.length >= 2) {
                                                pts.push(pts[0]);
                                            }
                                        }
                                    }
                                    if (pts.length >= 2) {
                                        for (var k = 0; k < pts.length - 1; k++) {
                                            __hostCanvasLine(id, pts[k].x, pts[k].y, pts[k+1].x, pts[k+1].y, Math.round(_lineWidth) * 4294967296 + packed);
                                        }
                                    }
                                },

                                fillText: function() {},
                                strokeText: function() {},
                                save: function() {},
                                restore: function() {},
                                translate: function() {},
                                rotate: function() {},
                                scale: function() {},
                            };
                            return ctx2d;
                        };
                        return elem;
                    }
                };

                var navigator = { gpu: {
                    requestAdapter: function() {
                        return Promise.resolve({
                            requestDevice: function() {
                                return Promise.resolve({
                                    createShaderModule: function(desc) {
                                        var h = __hostGpuCreateShaderModule(desc.code);
                                        return { __handle: h };
                                    },
                                    createRenderPipeline: function(desc) {
                                        var vs = desc.vertex.module.__handle;
                                        var fs = desc.fragment.module.__handle;
                                        var vsEntry = desc.vertex.entryPoint || 'vs_main';
                                        var fsEntry = desc.fragment.entryPoint || 'fs_main';
                                        var bufs = desc.vertex.buffers || [];
                                        var stride = bufs.length > 0 ? bufs[0].arrayStride : 0;
                                        var attrs = [];
                                        if (bufs.length > 0 && bufs[0].attributes) {
                                            for (var i = 0; i < bufs[0].attributes.length; i++) {
                                                var a = bufs[0].attributes[i];
                                                var fmtCode = 4;
                                                if (a.format === 'float32x3') fmtCode = 3;
                                                if (a.format === 'float32x4') fmtCode = 4;
                                                attrs.push([a.offset, fmtCode, 0]);
                                            }
                                        }
                                        var attrJson = JSON.stringify(attrs);
                                        var h = __hostGpuCreatePipeline(vs, fs, vsEntry, fsEntry, stride, attrJson);
                                        return { __handle: h };
                                    },
                                    createBuffer: function(desc) {
                                        var usage = 0;
                                        if (desc.usage & 0x20) usage |= 0x20;
                                        if (desc.usage & 0x40) usage |= 0x40;
                                        if (desc.usage & 0x10) usage |= 0x10;
                                        usage |= 0x8;
                                        var h = __hostGpuCreateBuffer(desc.size, usage);
                                        return { __handle: h, size: desc.size };
                                    },
                                    createCommandEncoder: function() {
                                        return {
                                            _canvasId: null,
                                            _pipeline: null,
                                            _vbuf: null,
                                            _clearColor: [0,0,0,1],
                                            beginRenderPass: function(desc) {
                                                var self = this;
                                                if (desc.colorAttachments && desc.colorAttachments[0]) {
                                                    var ca = desc.colorAttachments[0];
                                                    if (ca.view && ca.view.__canvasId) {
                                                        self._canvasId = ca.view.__canvasId;
                                                    }
                                                    if (ca.clearValue) {
                                                        var cv = ca.clearValue;
                                                        self._clearColor = [cv.r || 0, cv.g || 0, cv.b || 0, cv.a !== undefined ? cv.a : 1];
                                                    }
                                                }
                                                return {
                                                    setPipeline: function(p) { self._pipeline = p.__handle; },
                                                    setVertexBuffer: function(slot, buf) { self._vbuf = buf.__handle; },
                                                    draw: function(count) { self._vertexCount = count; },
                                                    end: function() {}
                                                };
                                            },
                                            finish: function() { return this; }
                                        };
                                    },
                                    queue: {
                                        writeBuffer: function(buf, offset, data) {
                                            var arr = [];
                                            if (data instanceof Float32Array || Array.isArray(data)) {
                                                for (var i = 0; i < data.length; i++) arr.push(data[i]);
                                            }
                                            __hostGpuWriteBuffer(buf.__handle, arr);
                                        },
                                        submit: function(cmdBufs) {
                                            for (var i = 0; i < cmdBufs.length; i++) {
                                                var cb = cmdBufs[i];
                                                if (cb._canvasId && cb._pipeline && cb._vbuf) {
                                                    var cc = cb._clearColor;
                                                    __hostGpuDraw(cb._canvasId, cb._pipeline, cb._vbuf,
                                                                  cb._vertexCount || 3, cc[0]+','+cc[1]+','+cc[2]+','+cc[3]);
                                                }
                                            }
                                        }
                                    },
                                    __isGpuDevice: true
                                });
                            }
                        });
                    }
                }};
            "#,
            )?;

            ctx.eval::<(), _>(script.as_str())?;
            Ok(())
        });

        if let Err(e) = res {
            self.context.with(|ctx| {
                if let Some(ex) = ctx.catch().into_exception() {
                    log::error!("JS init error (exception): {:?}", ex.message());
                } else {
                    log::error!("JS init error: {:?}", e);
                }
            });
            return Err(e);
        }

        self.drain_jobs();
        Ok(())
    }

    pub fn tick(&mut self, now_ms: f64) {
        let callbacks: Vec<_> = self.shared.borrow_mut().animation_callbacks.drain(..).collect();
        if !callbacks.is_empty() {
            let _ = self.context.with(|ctx| -> Result<()> {
                for cb in callbacks {
                    let f: rquickjs::Function<'_> = cb.restore(&ctx)?;
                    let _ = f.call::<_, ()>((now_ms,));
                }
                Ok(())
            });
        }
        self.drain_jobs();
    }

    fn fire_pointer_event(
        &mut self,
        event_type: &str,
        layout: &crate::layout::LayoutTree,
        x: f32,
        y: f32,
    ) {
        let (ids, target_id) = {
            let s = self.shared.borrow();
            if let Some(ref cap) = s.pointer_capture {
                (vec![cap.clone()], cap.clone())
            } else if let Some(hit) = crate::renderer::hit_test(layout, x, y) {
                let target = hit.id_chain.first().cloned().unwrap_or_default();
                (hit.id_chain.clone(), target)
            } else {
                (vec![], String::new())
            }
        };
        let ids_str = ids.join(",");
        let etype = event_type.to_string();
        let _ = self.context.with(|ctx| -> Result<()> {
            let f: Function<'_> = ctx.globals().get("__dispatchPointerEvent")?;
            f.call::<_, ()>((etype, ids_str, target_id, x as f64, y as f64))?;
            Ok(())
        });
        self.drain_jobs();
    }

    pub fn dispatch_pointer_down(&mut self, layout: &crate::layout::LayoutTree, x: f32, y: f32) {
        let target =
            crate::renderer::hit_test(layout, x, y).and_then(|h| h.id_chain.first().cloned());
        self.shared.borrow_mut().pointer_down_target = target;
        self.fire_pointer_event("pointerdown", layout, x, y);
    }

    pub fn dispatch_pointer_move(&mut self, layout: &crate::layout::LayoutTree, x: f32, y: f32) {
        self.fire_pointer_event("pointermove", layout, x, y);
    }

    pub fn dispatch_pointer_up(&mut self, layout: &crate::layout::LayoutTree, x: f32, y: f32) {
        self.fire_pointer_event("pointerup", layout, x, y);

        let had_capture = self.shared.borrow_mut().pointer_capture.take().is_some();

        let down_target = self.shared.borrow().pointer_down_target.clone();
        let up_target = if had_capture {
            None
        } else {
            crate::renderer::hit_test(layout, x, y).and_then(|h| h.id_chain.first().cloned())
        };

        if let (Some(ref dt), Some(ref ut)) = (&down_target, &up_target) {
            if dt == ut {
                self.fire_pointer_event("click", layout, x, y);
            }
        }
        self.shared.borrow_mut().pointer_down_target = None;
    }

    pub fn is_dirty(&self) -> bool {
        self.shared.borrow().dom_dirty
    }

    pub fn clear_dirty(&mut self) {
        self.shared.borrow_mut().dom_dirty = false;
    }

    pub fn canvas_ops(&self) -> HashMap<String, Vec<CanvasDrawOp>> {
        self.shared.borrow().canvas_ops.clone()
    }

    pub fn pointer_capture(&self) -> Option<String> {
        self.shared.borrow().pointer_capture.clone()
    }

    pub fn update_element_rects(&self, rects: HashMap<String, crate::types::LayoutRect>) {
        self.shared.borrow_mut().element_rects = rects;
    }

    pub fn text_overrides(&self) -> std::cell::Ref<'_, HashMap<String, String>> {
        std::cell::Ref::map(self.shared.borrow(), |s| &s.text_overrides)
    }

    pub fn style_overrides(&self) -> HashMap<String, Vec<(String, String)>> {
        self.shared
            .borrow()
            .style_overrides
            .iter()
            .map(|(id, map)| {
                (
                    id.clone(),
                    map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                )
            })
            .collect()
    }

    fn drain_jobs(&mut self) {
        while self.runtime.is_job_pending() {
            if let Err(e) = self.runtime.execute_pending_job() {
                self.context.with(|ctx| {
                    if let Some(ex) = ctx.catch().into_exception() {
                        log::error!("JS error (exception): {:?}", ex.message());
                    } else {
                        log::error!("JS error (job execution): {:?}", e);
                    }
                });
            }
        }
    }

    pub fn init_webgpu(&mut self, device: std::sync::Arc<wgpu::Device>, queue: std::sync::Arc<wgpu::Queue>) {
        self.shared.borrow_mut().webgpu = Some(WebGpuBridge::new(device, queue));

        let shared = self.shared.clone();
        let _ = self.context.with(|ctx| -> Result<()> {
            let globals = ctx.globals();

            // __hostGpuCreateShaderModule(code) -> handle
            let s = shared.clone();
            globals.set("__hostGpuCreateShaderModule", Function::new(ctx.clone(),
                move |_ctx: rquickjs::Ctx<'_>, code: String| -> u64 {
                    let mut st = s.borrow_mut();
                    if let Some(ref mut gpu) = st.webgpu {
                        gpu.create_shader_module(&code)
                    } else { 0 }
                }
            )?)?;

            // __hostGpuCreatePipeline(vs, fs, vs_entry, fs_entry, stride, attr_json) -> handle
            let s = shared.clone();
            globals.set("__hostGpuCreatePipeline", Function::new(ctx.clone(),
                move |_ctx: rquickjs::Ctx<'_>, vs: u64, fs: u64, vs_entry: String, fs_entry: String,
                      stride: u64, attr_json: String| -> u64 {
                    let mut st = s.borrow_mut();
                    if let Some(ref mut gpu) = st.webgpu {
                        let attrs: Vec<(u64, u32, u32)> = serde_json_mini_parse_attrs(&attr_json);
                        let wgpu_attrs: Vec<wgpu::VertexAttribute> = attrs.iter().enumerate().map(|(i, (offset, fmt_code, _))| {
                            wgpu::VertexAttribute {
                                format: vertex_format_from_code(*fmt_code),
                                offset: *offset,
                                shader_location: i as u32,
                            }
                        }).collect();
                        let layouts = vec![crate::webgpu_bridge::VertexBufferLayoutDesc {
                            array_stride: stride,
                            attributes: wgpu_attrs,
                        }];
                        let format = gpu.preferred_format();
                        gpu.create_render_pipeline(vs, fs, &vs_entry, &fs_entry, &layouts, format, wgpu::PrimitiveTopology::TriangleList)
                    } else { 0 }
                }
            )?)?;

            // __hostGpuCreateBuffer(size, usage) -> handle
            let s = shared.clone();
            globals.set("__hostGpuCreateBuffer", Function::new(ctx.clone(),
                move |_ctx: rquickjs::Ctx<'_>, size: u64, usage: u32| -> u64 {
                    let mut st = s.borrow_mut();
                    if let Some(ref mut gpu) = st.webgpu {
                        gpu.create_buffer(size, usage)
                    } else { 0 }
                }
            )?)?;

            // __hostGpuWriteBuffer(handle, data: [f32 as comma-separated])
            let s = shared.clone();
            globals.set("__hostGpuWriteBuffer", Function::new(ctx.clone(),
                move |ctx: rquickjs::Ctx<'_>, handle: u64, data: rquickjs::Value<'_>| {
                    let st = s.borrow();
                    if let Some(ref gpu) = st.webgpu {
                        if let Some(arr) = data.as_array() {
                            let mut floats = Vec::with_capacity(arr.len());
                            for i in 0..arr.len() {
                                if let Ok(v) = arr.get::<f32>(i) {
                                    floats.push(v);
                                }
                            }
                            let bytes: &[u8] = bytemuck::cast_slice(&floats);
                            gpu.write_buffer(handle, bytes);
                        }
                    }
                    let _ = ctx;
                }
            )?)?;

            // __hostGpuConfigureCanvas(canvas_id, width, height)
            let s = shared.clone();
            globals.set("__hostGpuConfigureCanvas", Function::new(ctx.clone(),
                move |_ctx: rquickjs::Ctx<'_>, canvas_id: String, width: u32, height: u32| {
                    let mut st = s.borrow_mut();
                    if let Some(ref mut gpu) = st.webgpu {
                        let fmt = gpu.preferred_format();
                        gpu.configure_canvas(&canvas_id, width, height, fmt);
                    }
                    st.dom_dirty = true;
                }
            )?)?;

            // __hostGpuDraw(canvas_id, pipeline, vbuf, vertex_count, clear_packed)
            // clear_packed encodes RGBA as a comma-separated string
            let s = shared.clone();
            globals.set("__hostGpuDraw", Function::new(ctx.clone(),
                move |_ctx: rquickjs::Ctx<'_>, canvas_id: String, pipeline: u64, vbuf: u64,
                      vertex_count: u32, clear_packed: String| {
                    let parts: Vec<f64> = clear_packed.split(',').filter_map(|s| s.trim().parse().ok()).collect();
                    let cr = parts.get(0).copied().unwrap_or(0.0);
                    let cg = parts.get(1).copied().unwrap_or(0.0);
                    let cb = parts.get(2).copied().unwrap_or(0.0);
                    let ca = parts.get(3).copied().unwrap_or(1.0);
                    let mut st = s.borrow_mut();
                    if let Some(ref mut gpu) = st.webgpu {
                        gpu.begin_render_pass(&canvas_id, cr, cg, cb, ca);
                        gpu.render_pass_draw(pipeline, vbuf, vertex_count, [cr, cg, cb, ca]);
                    }
                }
            )?)?;

            // __hostGpuPreferredFormat() -> string
            globals.set("__hostGpuPreferredFormat", Function::new(ctx.clone(),
                move |_ctx: rquickjs::Ctx<'_>| -> String {
                    "rgba8unorm-srgb".to_string()
                }
            )?)?;

            Ok(())
        });
    }

    pub fn webgpu_bridge(&self) -> std::cell::Ref<'_, Option<WebGpuBridge>> {
        std::cell::Ref::map(self.shared.borrow(), |s| &s.webgpu)
    }
}

impl Drop for JsBridge {
    fn drop(&mut self) {
        self.shared.borrow_mut().animation_callbacks.clear();
    }
}
