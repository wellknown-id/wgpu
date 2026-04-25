pub use kest::run;

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_com_example_custom_1webview_VrActivity_nativeSetResumed(
    env: *mut core::ffi::c_void,
    class: *mut core::ffi::c_void,
    resumed: u8,
) {
    kest::Java_com_example_custom_1webview_VrActivity_nativeSetResumed(env, class, resumed);
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_com_example_custom_1webview_VrActivity_nativeSetFocused(
    env: *mut core::ffi::c_void,
    class: *mut core::ffi::c_void,
    focused: u8,
) {
    kest::Java_com_example_custom_1webview_VrActivity_nativeSetFocused(env, class, focused);
}

#[cfg(target_os = "android")]
#[no_mangle]
pub fn android_main(app: winit::platform::android::activity::AndroidApp) {
    kest::android_main(app);
}
