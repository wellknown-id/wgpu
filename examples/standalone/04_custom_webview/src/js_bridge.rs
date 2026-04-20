use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use anyhow::Result;
use rquickjs::{Context, Function, Runtime};

pub struct JsBridge {
    runtime: Runtime,
    context: Context,
    shared: Rc<RefCell<SharedState>>,
}

pub struct SharedState {
    pub dom_dirty: bool,
    pub text_overrides: HashMap<String, String>,
}

impl JsBridge {
    pub fn new(script: Option<&str>) -> Result<Self> {
        let runtime = Runtime::new()?;
        let context = Context::full(&runtime)?;

        let shared = Rc::new(RefCell::new(SharedState {
            dom_dirty: false,
            text_overrides: HashMap::new(),
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

            ctx.eval::<(), _>(
                r#"
                var __listeners = {};

                function __hostAddEventListener(key, cb) {
                    if (!__listeners[key]) __listeners[key] = [];
                    __listeners[key].push(cb);
                }

                function __hostFireEvent(key) {
                    var cbs = __listeners[key];
                    if (cbs) {
                        for (var i = 0; i < cbs.length; i++) {
                            cbs[i]();
                        }
                    }
                }

                var document = {
                    getElementById: function(id) {
                        var elem = {
                            id: id,
                            addEventListener: function(event, cb) {
                                __hostAddEventListener(event + ':' + id, cb);
                            }
                        };
                        Object.defineProperty(elem, 'textContent', {
                            set: function(v) { __hostSetText(id, '' + v); },
                            get: function() { return ''; }
                        });
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

    pub fn dispatch_click(&mut self, layout: &crate::layout::LayoutTree, x: f32, y: f32) {
        if let Some(hit) = crate::renderer::hit_test(layout, x, y) {
            for id in &hit.id_chain {
                let key = format!("click:{id}");
                let _ = self.context.with(|ctx| -> Result<()> {
                    let fire: Function<'_> = ctx.globals().get("__hostFireEvent")?;
                    fire.call::<_, ()>((key,))?;
                    Ok(())
                });
            }
            self.drain_jobs();
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.shared.borrow().dom_dirty
    }

    pub fn clear_dirty(&mut self) {
        self.shared.borrow_mut().dom_dirty = false;
    }

    pub fn text_overrides(&self) -> std::cell::Ref<'_, HashMap<String, String>> {
        std::cell::Ref::map(self.shared.borrow(), |s| &s.text_overrides)
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
