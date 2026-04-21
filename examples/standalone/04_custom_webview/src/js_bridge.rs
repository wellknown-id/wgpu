use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use anyhow::Result;
use rquickjs::{Context, Function, Runtime};

use crate::types::{CanvasDrawOp, DragOverlay};

fn unpack_color(packed: u32) -> [f32; 4] {
    [
        ((packed >> 24) & 0xFF) as f32 / 255.0,
        ((packed >> 16) & 0xFF) as f32 / 255.0,
        ((packed >> 8) & 0xFF) as f32 / 255.0,
        (packed & 0xFF) as f32 / 255.0,
    ]
}

pub struct JsBridge {
    runtime: Runtime,
    context: Context,
    shared: Rc<RefCell<SharedState>>,
}

pub struct SharedState {
    pub dom_dirty: bool,
    pub text_overrides: HashMap<String, String>,
    pub style_overrides: HashMap<String, HashMap<String, String>>,
    pub canvas_ops: HashMap<String, Vec<CanvasDrawOp>>,
    pub pointer_capture: Option<String>,
    pub pointer_down_target: Option<String>,
    pub drag_overlay: Option<DragOverlay>,
}

impl JsBridge {
    pub fn new(script: Option<&str>) -> Result<Self> {
        let runtime = Runtime::new()?;
        let context = Context::full(&runtime)?;

        let shared = Rc::new(RefCell::new(SharedState {
            dom_dirty: false,
            text_overrides: HashMap::new(),
            style_overrides: HashMap::new(),
            canvas_ops: HashMap::new(),
            pointer_capture: None,
            pointer_down_target: None,
            drag_overlay: None,
        }));

        let mut bridge = Self {
            runtime,
            context,
            shared,
        };

        if let Some(script) = script {
            bridge.init_and_run(script)?;
        }

        Ok(bridge)
    }

    fn init_and_run(&mut self, script: &str) -> Result<()> {
        let shared = self.shared.clone();
        let script = script.to_string();

        self.context.with(|ctx| -> Result<()> {
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
                "__hostSetDragOverlay",
                Function::new(
                    ctx.clone(),
                    move |_ctx: rquickjs::Ctx<'_>,
                          text: String,
                          tag: String,
                          tag_color: u32| {
                        shared_clone.borrow_mut().drag_overlay = Some(DragOverlay {
                            text,
                            tag,
                            tag_color: crate::js_bridge::unpack_color(tag_color),
                        });
                    },
                )?,
            )?;

            let shared_clone = shared.clone();
            globals.set(
                "__hostClearDragOverlay",
                Function::new(
                    ctx.clone(),
                    move |_ctx: rquickjs::Ctx<'_>| {
                        shared_clone.borrow_mut().drag_overlay = None;
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
            "#,
            )?;

            ctx.eval::<(), _>(script.as_str())?;
            Ok(())
        })?;

        self.drain_jobs();
        Ok(())
    }

    pub fn tick(&mut self, _now_ms: f64) {
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

    pub fn drag_overlay(&self) -> Option<DragOverlay> {
        self.shared.borrow().drag_overlay.clone()
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
            if let Err(_e) = self.runtime.execute_pending_job() {
                self.context.with(|ctx| {
                    if let Some(ex) = ctx.catch().into_exception() {
                        eprintln!("JS error: {:?}", ex.message());
                    }
                });
            }
        }
    }
}
