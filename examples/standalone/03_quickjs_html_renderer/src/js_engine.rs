use std::{
    cell::RefCell,
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

use anyhow::{bail, Result};
use rquickjs::{
    loader::{Loader, Resolver},
    Context, Ctx, Function, Module, Persistent, Runtime,
};

use crate::gpu_state::GpuState;
use crate::host_api::install_host_api;
use crate::{FrameUpload, GeometryUpload, JsResult};

#[derive(Default)]
pub(crate) struct JsHostState {
    pub(crate) geometry: Option<GeometryUpload>,
    pub(crate) geometry_dirty: bool,
    pub(crate) pending_frame: Option<FrameUpload>,
    pub(crate) animation_loop: Option<Persistent<Function<'static>>>,
    pub(crate) resize_listeners: Vec<Persistent<Function<'static>>>,
    pub(crate) now_ms: f64,
}

pub(crate) struct JsEngine {
    runtime: Runtime,
    context: Context,
    host: Rc<RefCell<JsHostState>>,
}

impl JsEngine {
    pub(crate) fn new(
        asset_dir: PathBuf,
        gpu: Rc<RefCell<GpuState>>,
        width: u32,
        height: u32,
        filename: &str,
        search: &str,
    ) -> Result<Self> {
        let runtime = Runtime::new()?;

        let html = fs::read_to_string(asset_dir.join(filename))?;
        let (script, import_map) =
            extract_module_script(&html).map_err(|e| anyhow::anyhow!("{e}"))?;

        runtime.set_loader(
            FsResolver {
                root: asset_dir.clone(),
                import_map,
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

    pub(crate) fn dispatch_mouse_event(&mut self, ev_type: &str, x: f64, y: f64) -> Result<()> {
        self.context.with(|ctx| -> Result<()> {
            let target = ctx
                .globals()
                .get::<_, rquickjs::Object>("__activeCanvas")
                .or_else(|_| ctx.globals().get::<_, rquickjs::Object>("document"));

            if let Ok(document) = target {
                let has_pointer = ctx.globals().contains_key("PointerEvent").unwrap_or(false);
                let fallback = if has_pointer { "PointerEvent" } else { "Event" };
                if let Ok(event) =
                    ctx.eval::<rquickjs::Object, _>(format!("new {fallback}('{ev_type}')"))
                {
                    let _ = event.set("clientX", x);
                    let _ = event.set("clientY", y);
                    let _ = event.set("pageX", x);
                    let _ = event.set("pageY", y);
                    let _ = event.set("pointerId", 1);
                    let _ = event.set("pointerType", "mouse");
                    let _ = event.set("button", if ev_type == "pointermove" { -1 } else { 0 });
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

    pub(crate) fn set_time_ms(&mut self, now_ms: f64) {
        self.host.borrow_mut().now_ms = now_ms;
    }

    pub(crate) fn tick(&mut self) -> Result<()> {
        let callback = self.host.borrow().animation_loop.clone();
        if let Some(callback) = callback {
            if let Err(_e) = self.context.with(|ctx| -> JsResult<()> {
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
            }
        }

        self.drain_jobs()?;

        Ok(())
    }

    pub(crate) fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        let listeners = self.host.borrow().resize_listeners.clone();
        if let Err(_e) = self.context.with(|ctx| -> JsResult<()> {
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
        }

        self.drain_jobs()?;

        Ok(())
    }

    fn drain_jobs(&mut self) -> Result<()> {
        while self.runtime.is_job_pending() {
            if let Err(_err) = self.runtime.execute_pending_job() {
                self.context.with(|ctx| {
                    if let Some(js_e) = ctx.catch().into_exception() {
                        eprintln!("QuickJS Job Exception: {:?}", js_e.message());
                        if let Some(stack) = js_e.stack() {
                            eprintln!("Stack: {}", stack);
                        }
                    }
                });
            }
        }

        Ok(())
    }

    pub(crate) fn take_geometry(&mut self) -> Option<GeometryUpload> {
        let mut host = self.host.borrow_mut();
        if !host.geometry_dirty {
            return None;
        }
        host.geometry_dirty = false;
        host.geometry.clone()
    }

    pub(crate) fn take_frame(&mut self) -> Option<FrameUpload> {
        self.host.borrow_mut().pending_frame.take()
    }
}

struct FsResolver {
    root: PathBuf,
    import_map: HashMap<String, String>,
}

impl Resolver for FsResolver {
    fn resolve<'js>(&mut self, _ctx: &Ctx<'js>, base: &str, name: &str) -> JsResult<String> {
        let resolved_name = self.resolve_import_map(name);
        let name = resolved_name.as_deref().unwrap_or(name);

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

impl FsResolver {
    fn resolve_import_map(&self, name: &str) -> Option<String> {
        if let Some(mapped) = self.import_map.get(name) {
            return Some(mapped.clone());
        }
        for (prefix, target) in &self.import_map {
            if prefix.ends_with('/') && name.starts_with(prefix.as_str()) {
                let suffix = &name[prefix.len()..];
                return Some(format!("{target}{suffix}"));
            }
        }
        None
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

pub(crate) fn resolve_asset_path(root: &Path, path: &str) -> Result<PathBuf> {
    let root = fs::canonicalize(root)?;
    let requested = path.trim_start_matches("./");
    let full_path = fs::canonicalize(root.join(requested))?;
    if !full_path.starts_with(&root) {
        bail!("asset path escapes asset directory: {path}");
    }
    Ok(full_path)
}

fn extract_module_script(html: &str) -> JsResult<(String, HashMap<String, String>)> {
    let import_map = extract_import_map(html);

    let start = html.find("<script type=\"module\">").ok_or_else(|| {
        rquickjs::Error::new_loading_message("index.html", "missing module script")
    })?;
    let start = start + "<script type=\"module\">".len();
    let end = html[start..].find("</script>").ok_or_else(|| {
        rquickjs::Error::new_loading_message("index.html", "unterminated module script")
    })?;
    Ok((html[start..start + end].trim().to_string(), import_map))
}

fn extract_import_map(html: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let marker = "<script type=\"importmap\">";
    let Some(start) = html.find(marker) else {
        return map;
    };
    let start = start + marker.len();
    let Some(end) = html[start..].find("</script>") else {
        return map;
    };
    let json_str = &html[start..start + end];
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_str) else {
        return map;
    };
    if let Some(imports) = parsed.get("imports").and_then(|v| v.as_object()) {
        for (key, value) in imports {
            if let Some(target) = value.as_str() {
                let resolved = if target.starts_with("https://") || target.starts_with("http://") {
                    continue;
                } else {
                    target.to_string()
                };
                map.insert(key.clone(), resolved);
            }
        }
    }
    map
}

pub(crate) fn vec16(values: Vec<f32>, name: &'static str) -> JsResult<[f32; 16]> {
    values.try_into().map_err(|_| {
        rquickjs::Error::new_from_js_message(
            "Array",
            name,
            format!("expected 16 values for {name}"),
        )
    })
}

pub(crate) fn vec4(values: Vec<f32>, name: &'static str) -> JsResult<[f32; 4]> {
    values.try_into().map_err(|_| {
        rquickjs::Error::new_from_js_message("Array", name, format!("expected 4 values for {name}"))
    })
}
