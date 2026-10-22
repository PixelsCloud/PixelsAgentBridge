package com.pixels.agent.platform.accessibility

import android.accessibilityservice.AccessibilityService
import android.content.Intent
import android.view.accessibility.AccessibilityEvent
import com.pixels.agent.platform.PlatformOwner

class PixelsAccessibilityService : AccessibilityService() {
    private val backend
        get() = (application as PlatformOwner).automationBackend

    override fun onCreate() {
        super.onCreate()
        if (applicationInfo.flags and android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE != 0) {
            android.util.Log.i("PixelsProbe", "Service created pid=${android.os.Process.myPid()}")
        }
    }

    override fun onServiceConnected() {
        super.onServiceConnected()
        backend.attach(this)
        if (applicationInfo.flags and android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE != 0) {
            android.util.Log.i(
                "PixelsProbe",
                "Service connected backend=${System.identityHashCode(backend)}",
            )
        }
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        backend.invalidate(this)
    }

    override fun onInterrupt() {
        backend.invalidate(this)
    }

    override fun onUnbind(intent: Intent?): Boolean {
        backend.detach(this)
        return super.onUnbind(intent)
    }

    override fun onDestroy() {
        backend.detach(this)
        super.onDestroy()
    }
}
