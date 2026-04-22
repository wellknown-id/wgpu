fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        println!("cargo:rustc-link-arg=-z");
        println!("cargo:rustc-link-arg=max-page-size=16384");

        let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
        let jni_dir = if arch == "aarch64" {
            "arm64-v8a"
        } else {
            "armeabi-v7a"
        };
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        println!(
            "cargo:rustc-link-search=native={MANIFEST_DIR}/jniLibs/{JNI_DIR}",
            MANIFEST_DIR = manifest_dir,
            JNI_DIR = jni_dir
        );
        println!("cargo:rustc-link-lib=dylib=openxr_loader");
    }
}
