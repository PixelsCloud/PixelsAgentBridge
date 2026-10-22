package com.pixels.agent

import android.app.UiAutomation
import android.content.Intent
import android.graphics.BitmapFactory
import android.os.Build
import android.os.SystemClock
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.Configurator
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.UiSelector
import com.pixels.agent.core.CaptureFailure
import com.pixels.agent.core.CaptureResult
import com.pixels.agent.fixture.AutomationFixtureActivity
import com.pixels.agent.platform.capture.AccessibilityCaptureProvider
import com.pixels.agent.platform.capture.ProjectionCaptureService
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

/** Accessibility input has a separate in-process smoke harness; see the Android README. */
@RunWith(AndroidJUnit4::class)
class AutomationDeviceTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val app
        get() = instrumentation.targetContext.applicationContext as PixelsApplication

    @Before
    fun preserveAccessibilityServicesDuringUiAutomation() {
        Configurator.getInstance()
            .setUiAutomationFlags(UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES)
        instrumentation.getUiAutomation(UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES)
    }

    @Test
    fun sharedCoreLoadsOnAndroid() {
        val result = JSONObject(app.nativeBridge.probe())
        assertEquals("loaded", result.getString("status"))
        assertEquals("android", result.getString("os"))
        assertEquals("pixels-agent-bridge/1", result.getString("transport_alpn"))
    }

    @Test
    fun accessibilityCaptureReturnsDecodableJpeg() = runBlocking {
        assumeTrue(Build.VERSION.SDK_INT >= 30)
        val activity =
            instrumentation.startActivitySync(
                Intent(app, AutomationFixtureActivity::class.java)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            )
        try {
            val deadline = SystemClock.elapsedRealtime() + 5000
            while (
                !app.automationBackend.available.value && SystemClock.elapsedRealtime() < deadline
            ) SystemClock.sleep(100)
            assertTrue(
                "Enable Pixels device control before the screenshot test",
                app.automationBackend.available.value,
            )
            assertEquals(
                CaptureResult.Failed(CaptureFailure.SERVICE_UNAVAILABLE),
                AccessibilityCaptureProvider { null }.capture(),
            )
            SystemClock.sleep(500)
            val result = app.screenCapture.capture()
            assertTrue("Accessibility screenshot failed: $result", result is CaptureResult.Frame)
            val frame = result as CaptureResult.Frame
            val bitmap =
                requireNotNull(BitmapFactory.decodeByteArray(frame.jpeg, 0, frame.jpeg.size))
            try {
                assertEquals(frame.width, bitmap.width)
                assertEquals(frame.height, bitmap.height)
                assertTrue(bitmap.width > 0 && bitmap.height > 0)
            } finally {
                bitmap.recycle()
            }
            val next = app.screenCapture.capture()
            assertTrue(
                "Rate limiting must not be classified as protected content",
                next is CaptureResult.Frame ||
                    next == CaptureResult.Failed(CaptureFailure.FRAME_UNAVAILABLE),
            )
        } finally {
            instrumentation.runOnMainSync { activity.finish() }
        }
    }

    @Test
    fun projectionRequiresConsentAndReturnsJpeg() = runBlocking {
        assumeTrue(Build.VERSION.SDK_INT < 30)
        val context = instrumentation.targetContext
        context.stopService(Intent(context, ProjectionCaptureService::class.java))
        instrumentation.waitForIdleSync()
        SystemClock.sleep(300)
        assertEquals(
            CaptureResult.Failed(CaptureFailure.CONSENT_REQUIRED),
            app.screenCapture.capture(),
        )
        val activity =
            instrumentation.startActivitySync(
                Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            )
        val device = UiDevice.getInstance(instrumentation)
        try {
            val label = context.getString(R.string.authorize_capture)
            val consent = device.findObject(UiSelector().text(label))
            // Use bounded gestures near the edge: the old UiScrollable search stops early
            // on this emulator when Compose text fields precede the consent controls.
            var attempts = 0
            while (!consent.exists() && attempts++ < 8) {
                val x = device.displayWidth * 9 / 10
                device.swipe(x, device.displayHeight * 4 / 5, x, device.displayHeight / 5, 30)
                instrumentation.waitForIdleSync()
            }
            assertTrue("Screen capture consent control was not reached", consent.exists())
            assertTrue(consent.click())
            val allow = device.findObject(UiSelector().resourceId("android:id/button1"))
            assertTrue("System capture consent dialog did not open", allow.waitForExists(5000))
            allow.click()
            var capture: CaptureResult = CaptureResult.Failed(CaptureFailure.FRAME_UNAVAILABLE)
            val deadline = SystemClock.elapsedRealtime() + 5000
            while (capture !is CaptureResult.Frame && SystemClock.elapsedRealtime() < deadline) {
                SystemClock.sleep(100)
                capture = app.screenCapture.capture()
            }
            assertTrue(
                "No screenshot received after consent: ${capture.javaClass.simpleName}",
                capture is CaptureResult.Frame,
            )
            val frame = capture as CaptureResult.Frame
            assertTrue(frame.width > 0 && frame.height > 0)
            assertTrue(frame.jpeg.size > 100)
            assertEquals(0xff, frame.jpeg[0].toInt() and 0xff)
            assertEquals(0xd8, frame.jpeg[1].toInt() and 0xff)
        } finally {
            context.stopService(Intent(context, ProjectionCaptureService::class.java))
            instrumentation.runOnMainSync { activity.finish() }
        }
        SystemClock.sleep(300)
        assertEquals(
            CaptureResult.Failed(CaptureFailure.CONSENT_REQUIRED),
            app.screenCapture.capture(),
        )
    }
}
