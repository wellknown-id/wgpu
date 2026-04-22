package com.example.custom_webview;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;

public class HybridLaunchReceiver extends BroadcastReceiver {
    public static final String ACTION_LAUNCH_IMMERSIVE =
            "com.example.custom_webview.action.LAUNCH_IMMERSIVE";

    @Override
    public void onReceive(Context context, Intent intent) {
        if (!ACTION_LAUNCH_IMMERSIVE.equals(intent.getAction())) {
            return;
        }

        String path = intent.getStringExtra(BaseNativeActivity.EXTRA_LAUNCH_PATH);
        boolean autoEnterVr =
                intent.getBooleanExtra(BaseNativeActivity.EXTRA_AUTO_ENTER_VR, false);
        context.startActivity(BaseNativeActivity.createImmersiveIntent(context, path, autoEnterVr));
    }
}
