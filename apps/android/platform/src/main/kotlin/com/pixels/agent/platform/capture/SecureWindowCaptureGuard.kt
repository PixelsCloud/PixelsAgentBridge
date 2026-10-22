package com.pixels.agent.platform.capture

import android.accessibilityservice.AccessibilityService
import androidx.annotation.RequiresApi
import com.pixels.agent.core.CaptureFailure
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withTimeoutOrNull

/** Android 14 can explicitly reject a secure window before a full-display capture. */
@RequiresApi(34)
internal object SecureWindowCaptureGuard {
    suspend fun check(service: AccessibilityService): CaptureFailure? {
        val window = service.rootInActiveWindow?.windowId ?: return CaptureFailure.FRAME_UNAVAILABLE
        val outcome =
            try {
                withTimeoutOrNull(3000) {
                    suspendCancellableCoroutine<Outcome> { continuation ->
                        service.takeScreenshotOfWindow(
                            window,
                            service.mainExecutor,
                            object : AccessibilityService.TakeScreenshotCallback {
                                override fun onSuccess(
                                    screenshot: AccessibilityService.ScreenshotResult
                                ) {
                                    // The guard never encodes or retains this temporary window
                                    // image.
                                    screenshot.hardwareBuffer.close()
                                    continuation.resume(Outcome.Allowed) { _, _, _ -> }
                                }

                                override fun onFailure(errorCode: Int) {
                                    val failure =
                                        when (errorCode) {
                                            AccessibilityService
                                                .ERROR_TAKE_SCREENSHOT_SECURE_WINDOW ->
                                                CaptureFailure.PROTECTED_OR_REJECTED
                                            AccessibilityService
                                                .ERROR_TAKE_SCREENSHOT_NO_ACCESSIBILITY_ACCESS ->
                                                CaptureFailure.SERVICE_UNAVAILABLE
                                            else -> CaptureFailure.FRAME_UNAVAILABLE
                                        }
                                    continuation.resume(Outcome.Rejected(failure)) { _, _, _ -> }
                                }
                            },
                        )
                    }
                }
            } catch (_: SecurityException) {
                return CaptureFailure.SERVICE_UNAVAILABLE
            }
        return when (outcome) {
            Outcome.Allowed -> null
            is Outcome.Rejected -> outcome.failure
            null -> CaptureFailure.FRAME_UNAVAILABLE
        }
    }

    private sealed interface Outcome {
        data object Allowed : Outcome

        data class Rejected(val failure: CaptureFailure) : Outcome
    }
}
