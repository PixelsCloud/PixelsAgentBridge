package com.pixels.agent.fixture

import android.graphics.BitmapFactory
import android.os.Build
import com.pixels.agent.PixelsApplication
import com.pixels.agent.core.AutomationCommand
import com.pixels.agent.core.AutomationError
import com.pixels.agent.core.AutomationResult
import com.pixels.agent.core.CaptureResult
import com.pixels.agent.core.TouchGesture
import kotlinx.coroutines.delay
import org.json.JSONObject

/** Uses the real adapter in its normal process, independently of instrumentation restarts. */
internal object AutomationSmokeProbe {
    suspend fun run(activity: AutomationFixtureActivity): JSONObject {
        val app = activity.application as PixelsApplication
        repeat(50) { if (!app.automationBackend.available.value) delay(100) }
        check(app.automationBackend.available.value) { "service_not_bound" }
        delay(500)
        val screen =
            app.dispatcher.execute(AutomationCommand.Observe) as? AutomationResult.Observed
                ?: error("observation_failed")
        val field = screen.screen.elements.single { it.name == "fixture_input" }
        check(
            app.dispatcher.execute(AutomationCommand.SetText(field.reference, "Pixels 测试"))
                is AutomationResult.Dispatched
        ) {
            "text_not_dispatched"
        }
        awaitCondition("text_verification_failed") { activity.field.text.toString() == "Pixels 测试" }
        delay(300)
        var observed =
            app.dispatcher.execute(AutomationCommand.Observe) as AutomationResult.Observed
        var button = observed.screen.elements.single { it.name == "fixture_save" }
        check(
            app.dispatcher.execute(AutomationCommand.Click(button.reference))
                is AutomationResult.Dispatched
        ) {
            "click_not_dispatched"
        }
        awaitCondition("click_verification_failed") {
            activity.status.text.toString() == "fixture_saved:Pixels 测试"
        }
        check(
            app.dispatcher.execute(AutomationCommand.Click(button.reference)) ==
                AutomationResult.Failed(AutomationError.TARGET_STALE)
        ) {
            "stale_reference_accepted"
        }
        delay(300)
        observed = app.dispatcher.execute(AutomationCommand.Observe) as AutomationResult.Observed
        button = observed.screen.elements.single { it.name == "fixture_save" }
        val x = (button.bounds.left + button.bounds.right) / 2
        val y = (button.bounds.top + button.bounds.bottom) / 2
        check(
            app.dispatcher.execute(
                AutomationCommand.Touch(TouchGesture(observed.screen.revision, x, y, x, y, 80))
            ) is AutomationResult.Dispatched
        ) {
            "gesture_not_dispatched"
        }
        awaitCondition("gesture_verification_failed") { activity.saveCount == 2 }
        app.dispatcher.setPaused(true)
        try {
            check(
                app.dispatcher.execute(
                    AutomationCommand.Touch(TouchGesture(observed.screen.revision, x, y, x, y, 80))
                ) == AutomationResult.Failed(AutomationError.CONTROL_PAUSED)
            ) {
                "pause_not_respected"
            }
        } finally {
            app.dispatcher.setPaused(false)
        }
        val result =
            JSONObject()
                .put("status", "passed")
                .put("observation_nodes", screen.screen.elements.size)
                .put("text_input", true)
                .put("native_click", true)
                .put("touch_gesture", true)
                .put("stale_reference_rejected", true)
                .put("pause_respected", true)
        if (Build.VERSION.SDK_INT >= 30) {
            delay(500)
            val capture =
                app.screenCapture.capture() as? CaptureResult.Frame
                    ?: error("accessibility_screenshot_failed")
            val bitmap =
                BitmapFactory.decodeByteArray(capture.jpeg, 0, capture.jpeg.size)
                    ?: error("screenshot_jpeg_invalid")
            try {
                check(bitmap.width == capture.width && bitmap.height == capture.height) {
                    "screenshot_dimensions_invalid"
                }
            } finally {
                bitmap.recycle()
            }
            result
                .put("accessibility_screenshot", true)
                .put("screenshot_width", capture.width)
                .put("screenshot_height", capture.height)
                .put("screenshot_bytes", capture.jpeg.size)
        }
        return result
    }

    private suspend fun awaitCondition(reason: String, condition: () -> Boolean) {
        repeat(50) {
            if (condition()) return
            delay(20)
        }
        error(reason)
    }
}
