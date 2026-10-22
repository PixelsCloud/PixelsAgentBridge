package com.pixels.agent.platform.capture

import android.accessibilityservice.AccessibilityService
import android.graphics.Bitmap
import android.os.Build
import android.view.Display
import androidx.annotation.RequiresApi
import com.pixels.agent.core.CaptureFailure
import com.pixels.agent.core.CaptureResult
import com.pixels.agent.core.ScreenCaptureProvider
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull

@RequiresApi(30)
class AccessibilityCaptureProvider(private val service: () -> AccessibilityService?) :
    ScreenCaptureProvider {
    override suspend fun capture(): CaptureResult =
        withContext(Dispatchers.Main.immediate) {
            val active =
                service()
                    ?: return@withContext CaptureResult.Failed(CaptureFailure.SERVICE_UNAVAILABLE)
            if (Build.VERSION.SDK_INT >= 34) {
                val rejection = SecureWindowCaptureGuard.check(active)
                if (rejection != null) return@withContext CaptureResult.Failed(rejection)
            }
            var owned: AccessibilityService.ScreenshotResult? = null
            try {
                val outcome =
                    try {
                        withTimeoutOrNull(3000) {
                            suspendCancellableCoroutine<Outcome> { continuation ->
                                    active.takeScreenshot(
                                        Display.DEFAULT_DISPLAY,
                                        active.mainExecutor,
                                        object : AccessibilityService.TakeScreenshotCallback {
                                            override fun onSuccess(
                                                screenshot: AccessibilityService.ScreenshotResult
                                            ) {
                                                continuation.resume(Outcome.Captured(screenshot)) {
                                                    _,
                                                    value,
                                                    _ ->
                                                    value.screenshot.hardwareBuffer.close()
                                                }
                                            }

                                            override fun onFailure(errorCode: Int) {
                                                val reason =
                                                    when (errorCode) {
                                                        AccessibilityService
                                                            .ERROR_TAKE_SCREENSHOT_NO_ACCESSIBILITY_ACCESS ->
                                                            CaptureFailure.SERVICE_UNAVAILABLE
                                                        AccessibilityService
                                                            .ERROR_TAKE_SCREENSHOT_INTERVAL_TIME_SHORT,
                                                        AccessibilityService
                                                            .ERROR_TAKE_SCREENSHOT_INTERNAL_ERROR,
                                                        AccessibilityService
                                                            .ERROR_TAKE_SCREENSHOT_INVALID_DISPLAY ->
                                                            CaptureFailure.FRAME_UNAVAILABLE
                                                        else -> CaptureFailure.PROTECTED_OR_REJECTED
                                                    }
                                                continuation.resume(Outcome.Rejected(reason)) {
                                                    _,
                                                    _,
                                                    _ ->
                                                }
                                            }
                                        },
                                    )
                                }
                                .also { value ->
                                    if (value is Outcome.Captured) owned = value.screenshot
                                }
                        }
                    } catch (_: SecurityException) {
                        return@withContext CaptureResult.Failed(CaptureFailure.SERVICE_UNAVAILABLE)
                    }
                when (outcome) {
                    null -> CaptureResult.Failed(CaptureFailure.FRAME_UNAVAILABLE)
                    is Outcome.Rejected -> CaptureResult.Failed(outcome.reason)
                    is Outcome.Captured ->
                        withContext(Dispatchers.Default) { encode(outcome.screenshot) }
                }
            } finally {
                // Ownership is transferred before leaving withTimeoutOrNull as well as
                // before leaving the main dispatcher. Both boundaries can discard a result.
                owned?.hardwareBuffer?.close()
            }
        }

    private fun encode(screenshot: AccessibilityService.ScreenshotResult): CaptureResult {
        val hardware =
            Bitmap.wrapHardwareBuffer(screenshot.hardwareBuffer, screenshot.colorSpace)
                ?: return CaptureResult.Failed(CaptureFailure.FRAME_UNAVAILABLE)
        val bitmap =
            try {
                hardware.copy(Bitmap.Config.ARGB_8888, false)
            } finally {
                hardware.recycle()
            }
        return try {
            FrameEncoder.encode(bitmap)
        } finally {
            bitmap.recycle()
        }
    }

    private sealed interface Outcome {
        data class Captured(val screenshot: AccessibilityService.ScreenshotResult) : Outcome

        data class Rejected(val reason: CaptureFailure) : Outcome
    }
}
