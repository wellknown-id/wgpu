#![cfg(target_os = "android")]

use anyhow::{Context, Result};
use jni::{
    objects::{JObject, JString, JValue},
    sys::{jboolean, jobject},
    JavaVM,
};
use winit::platform::android::activity::AndroidApp;

#[derive(Default)]
pub struct LaunchOptions {
    pub immersive_activity: bool,
    pub launch_asset: Option<String>,
    pub auto_enter_vr: bool,
}

fn with_activity<R>(
    app: &AndroidApp,
    f: impl FnOnce(&mut jni::AttachGuard<'_>, &JObject<'_>) -> Result<R>,
) -> Result<R> {
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr() as *mut jni::sys::JavaVM) }
        .context("failed to wrap Java VM")?;
    let mut env = vm
        .attach_current_thread()
        .context("failed to attach current thread")?;
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as jobject) };
    let result = f(&mut env, &activity);
    std::mem::forget(activity);
    result
}

pub fn read_launch_options(app: &AndroidApp) -> Result<LaunchOptions> {
    with_activity(app, |env, activity| {
        let immersive_activity = env
            .call_method(activity, "isImmersiveActivity", "()Z", &[])
            .context("failed to query activity mode")?
            .z()
            .context("activity mode was not a boolean")?;

        let launch_asset = env
            .call_method(activity, "consumeLaunchPath", "()Ljava/lang/String;", &[])
            .context("failed to query launch path")?
            .l()
            .context("launch path was not a string")?;
        let launch_asset = if launch_asset.is_null() {
            None
        } else {
            Some(
                env.get_string(&JString::from(launch_asset))
                    .context("failed to read launch path")?
                    .into(),
            )
        };

        let auto_enter_vr = env
            .call_method(activity, "consumeAutoEnterVr", "()Z", &[])
            .context("failed to query auto-enter flag")?
            .z()
            .context("auto-enter flag was not a boolean")?;

        Ok(LaunchOptions {
            immersive_activity,
            launch_asset,
            auto_enter_vr,
        })
    })
}

pub fn launch_immersive_activity(app: &AndroidApp, asset: &str, auto_enter_vr: bool) -> Result<()> {
    with_activity(app, |env, activity| {
        let asset = env
            .new_string(asset)
            .context("failed to create Java launch path string")?;
        let asset = JObject::from(asset);
        env.call_method(
            activity,
            "launchImmersiveActivity",
            "(Ljava/lang/String;Z)V",
            &[
                JValue::Object(&asset),
                JValue::Bool(auto_enter_vr as jboolean),
            ],
        )
        .context("failed to start immersive activity")?;
        Ok(())
    })
}
