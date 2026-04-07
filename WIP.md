# WGPU Binary Size Comparison (Release + Stripped)

This document tracks the binary size of the `wgpu-example-02-hello-window` standalone example across all major platforms.

## Comparison Table

| Platform | Architecture | Size (Stripped) | Backends Included | Windowing System |
| :--- | :--- | :--- | :--- | :--- |
| **Linux** | x86_64 | **9.0 MB** | Vulkan, GLES | X11, Wayland |
| **Windows** | x86_64 | **7.6 MB** | DX12, Vulkan, GLES | Win32 |
| **Android** | arm64 | **5.8 MB** | Vulkan, GLES | NativeActivity |
| **macOS** | arm64 | **5.1 MB** | Metal | AppKit |
| **iOS** | arm64 | **4.9 MB** | Metal | UIKit |
| **Web (WASM)** | wasm32 | **1.1 MB** | WebGPU, WebGL | Browser-native |

---

## Build Commands

These commands assume the environment is set up with the necessary cross-compilation toolchains (MinGW, osxcross, Android NDK).

### 1. Linux (Native)
```bash
cargo build --release -p wgpu-example-02-hello-window
strip target/release/wgpu-example-02-hello-window
```

### 2. Windows (Cross from Linux)
```bash
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
cargo build --release -p wgpu-example-02-hello-window --target x86_64-pc-windows-gnu
x86_64-w64-mingw32-strip target/x86_64-pc-windows-gnu/release/wgpu-example-02-hello-window.exe
```

### 3. macOS (Cross from Linux via osxcross)
```bash
PATH=$PATH:~/osxcross/target/bin \
CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER=aarch64-apple-darwin25.1-clang \
cargo build --release -p wgpu-example-02-hello-window --target aarch64-apple-darwin

PATH=$PATH:~/osxcross/target/bin \
aarch64-apple-darwin25.1-strip target/aarch64-apple-darwin/release/wgpu-example-02-hello-window
```

### 4. Android (Cross from Linux)
```bash
NDK_HOME=/home/ben/Android/Sdk/ndk/29.0.14206865
TOOLCHAIN=$NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin
RUST_MIN_STACK=16777216 \
CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$TOOLCHAIN/aarch64-linux-android29-clang \
cargo build --release -p wgpu-example-02-hello-window --target aarch64-linux-android

$TOOLCHAIN/llvm-strip target/aarch64-linux-android/release/wgpu-example-02-hello-window
```

### 5. iOS (Native on Mac via SSH)
```bash
ssh mac 'cd /tmp/wgpu_ios && cargo build --release -p wgpu-example-02-hello-window --target aarch64-apple-ios'
ssh mac 'strip /tmp/wgpu_ios/target/aarch64-apple-ios/release/wgpu-example-02-hello-window'
```

### 6. Web (WASM)
```bash
cargo build --release -p wgpu-example-02-hello-window --target wasm32-unknown-unknown
wasm-bindgen target/wasm32-unknown-unknown/release/wgpu-example-02-hello-window.wasm --out-dir target/wasm-out --target web
wasm-opt -Oz -o target/wasm-out/wgpu-example-02-hello-window_opt.wasm target/wasm-out/wgpu-example-02-hello-window_bg.wasm
```

## Observations
- **Debug Symbols:** Default release builds include ~110MB+ of debug symbols. Stripping is mandatory for deployment.
- **Naga:** Native builds include the Naga shader compiler (~4-5MB), which is omitted in WASM because the browser handles shader translation.
- **Windowing:** Multi-backend support on Linux (X11 + Wayland) adds ~1.5MB compared to single-backend platforms like macOS/Android.
