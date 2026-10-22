package com.pixels.agent.platform.capture

import android.os.Build
import com.pixels.agent.core.ScreenCaptureProvider
import com.pixels.agent.platform.AndroidAutomationBackend

object ScreenCaptureFactory {
    fun create(
        backend: AndroidAutomationBackend,
        projection: ProjectionCaptureProvider,
    ): ScreenCaptureProvider =
        if (Build.VERSION.SDK_INT >= 30) AccessibilityCaptureProvider(backend::activeService)
        else projection
}
