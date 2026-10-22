package com.pixels.agent

import android.app.UiAutomation
import android.content.Intent
import android.os.SystemClock
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.Configurator
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.UiObject
import androidx.test.uiautomator.UiSelector
import java.io.File
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class HomeSettingsDeviceTest {
    @Test
    fun unsupportedSettingsRemainIntactUntilRestoredAndReloaded() = runBlocking {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val app = instrumentation.targetContext.applicationContext as PixelsApplication
        Configurator.getInstance()
            .setUiAutomationFlags(UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES)
        instrumentation.getUiAutomation(UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES)
        val device = UiDevice.getInstance(instrumentation)
        check(!app.connection.status.value.active)
        app.connection.loadSettings()
        val settings = File(app.noBackupFilesDir, "settings.toml")
        val original = settings.takeIf { it.exists() }?.readBytes()
        val futureSettings =
            "schema_version = 999\ncontrol_url = \"wss://unused.invalid/control\"\n".toByteArray()
        fun restore() {
            if (original == null) settings.delete() else settings.writeBytes(original)
        }
        try {
            settings.writeBytes(futureSettings)
            val activity =
                instrumentation.startActivitySync(
                    Intent(app, MainActivity::class.java)
                        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK)
                )
            fun find(selector: UiSelector): UiObject {
                val target = device.findObject(selector)
                if (target.waitForExists(2000)) return target
                for (direction in listOf(1, -1)) {
                    repeat(8) {
                        if (target.exists()) return target
                        device.swipe(
                            device.displayWidth * 9 / 10,
                            device.displayHeight * (if (direction == 1) 4 else 1) / 5,
                            device.displayWidth * 9 / 10,
                            device.displayHeight * (if (direction == 1) 1 else 4) / 5,
                            20,
                        )
                    }
                }
                error("Home recovery control missing")
            }
            fun assertStatus(message: Int) {
                val status = find(UiSelector().resourceId("control_status"))
                val deadline = SystemClock.elapsedRealtime() + 5000
                while (
                    status.text != app.getString(message) &&
                        SystemClock.elapsedRealtime() < deadline
                ) SystemClock.sleep(100)
                assertTrue("Unexpected recovery state", status.text == app.getString(message))
            }
            assertStatus(R.string.status_settings_unavailable)
            find(UiSelector().text(app.getString(R.string.reload_settings))).click()
            assertStatus(R.string.status_settings_unavailable)
            val connect = find(UiSelector().resourceId("connect_device"))
            assertFalse(
                "Unreadable settings must not be overwritten from the form",
                connect.isEnabled,
            )
            assertFalse(app.connection.status.value.active)
            assertArrayEquals(futureSettings, settings.readBytes())
            restore()
            find(UiSelector().text(app.getString(R.string.reload_settings))).click()
            assertStatus(
                if (app.automationBackend.available.value) R.string.status_disconnected
                else R.string.status_permission_required
            )
            if (original == null) assertFalse(settings.exists())
            else assertArrayEquals(original, settings.readBytes())
            instrumentation.runOnMainSync { activity.finish() }
        } finally {
            restore()
            app.connection.loadSettings()
        }
    }
}
