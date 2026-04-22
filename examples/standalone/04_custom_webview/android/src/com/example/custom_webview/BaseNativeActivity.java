package com.example.custom_webview;

import android.content.Context;
import android.app.NativeActivity;
import android.content.Intent;

public abstract class BaseNativeActivity extends NativeActivity {
    public static final String EXTRA_LAUNCH_PATH = "com.example.custom_webview.LAUNCH_PATH";
    public static final String EXTRA_AUTO_ENTER_VR = "com.example.custom_webview.AUTO_ENTER_VR";
    public static final String EXTRA_DEBUG_AUTO_LAUNCH_IMMERSIVE =
            "com.example.custom_webview.DEBUG_AUTO_LAUNCH_IMMERSIVE";

    private boolean debugAutoLaunchHandled = false;

    @Override
    protected void onResume() {
        super.onResume();

        Intent intent = getIntent();
        if (intent == null || isImmersiveActivity() || debugAutoLaunchHandled) {
            return;
        }
        if (!intent.getBooleanExtra(EXTRA_DEBUG_AUTO_LAUNCH_IMMERSIVE, false)) {
            return;
        }

        debugAutoLaunchHandled = true;
        String path = intent.getStringExtra(EXTRA_LAUNCH_PATH);
        boolean autoEnterVr = intent.getBooleanExtra(EXTRA_AUTO_ENTER_VR, false);
        intent.removeExtra(EXTRA_DEBUG_AUTO_LAUNCH_IMMERSIVE);

        getWindow().getDecorView().post(() -> launchImmersiveActivity(path, autoEnterVr));
    }

    public boolean isImmersiveActivity() {
        return this instanceof VrActivity;
    }

    public String consumeLaunchPath() {
        Intent intent = getIntent();
        if (intent == null) {
            return null;
        }
        String path = intent.getStringExtra(EXTRA_LAUNCH_PATH);
        intent.removeExtra(EXTRA_LAUNCH_PATH);
        return path;
    }

    public boolean consumeAutoEnterVr() {
        Intent intent = getIntent();
        if (intent == null) {
            return false;
        }
        boolean autoEnterVr = intent.getBooleanExtra(EXTRA_AUTO_ENTER_VR, false);
        intent.removeExtra(EXTRA_AUTO_ENTER_VR);
        return autoEnterVr;
    }

    public static Intent createImmersiveIntent(Context context, String path, boolean autoEnterVr) {
        Intent immersiveIntent = new Intent(context, VrActivity.class);
        immersiveIntent.setAction(Intent.ACTION_MAIN);
        immersiveIntent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        if (path != null) {
            immersiveIntent.putExtra(EXTRA_LAUNCH_PATH, path);
        }
        immersiveIntent.putExtra(EXTRA_AUTO_ENTER_VR, autoEnterVr);
        return immersiveIntent;
    }

    public void launchImmersiveActivity(String path, boolean autoEnterVr) {
        Intent immersiveIntent = createImmersiveIntent(this, path, autoEnterVr);
        startActivity(immersiveIntent);
        finishAndRemoveTask();
    }
}
