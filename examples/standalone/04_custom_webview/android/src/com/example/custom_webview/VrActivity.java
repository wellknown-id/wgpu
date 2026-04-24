package com.example.custom_webview;

import android.app.NativeActivity;

public class VrActivity extends NativeActivity {
    static {
        System.loadLibrary("wgpu_example_04_custom_webview");
    }

    private static native void nativeSetResumed(boolean resumed);
    private static native void nativeSetFocused(boolean focused);

    @Override
    protected void onResume() {
        super.onResume();
        nativeSetResumed(true);
    }

    @Override
    protected void onPause() {
        nativeSetResumed(false);
        super.onPause();
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        nativeSetFocused(hasFocus);
    }
}
