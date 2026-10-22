package com.pixels.agent

import android.app.UiAutomation
import android.content.Intent
import android.os.Build
import android.os.SystemClock
import android.view.inputmethod.InputMethodManager
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.Configurator
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.UiObject
import androidx.test.uiautomator.UiSelector
import com.pixels.agent.core.ConnectionSettings
import com.pixels.agent.fixture.AutomationFixtureActivity
import java.io.File
import java.util.UUID
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

/** Drives the ordinary home screen. Only ConnectionService owns the actual connection. */
@RunWith(AndroidJUnit4::class)
class ProductionConnectionDeviceTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val app
        get() = instrumentation.targetContext.applicationContext as PixelsApplication

    private val device
        get() = UiDevice.getInstance(instrumentation)

    @Test
    fun homeScreenConnectsThroughForegroundService() = runBlocking {
        val args = InstrumentationRegistry.getArguments()
        val request = UUID.fromString(args.getString("production_request_id")).toString()
        val control = requireNotNull(args.getString("control_url"))
        val relay = requireNotNull(args.getString("relay_url"))
        check(java.net.URI(control).host == "localhost" && java.net.URI(relay).host == "localhost")
        Configurator.getInstance()
            .setUiAutomationFlags(UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES)
        instrumentation.getUiAutomation(UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES)
        val settingsFile = File(app.noBackupFilesDir, "settings.toml")
        val original = settingsFile.takeIf { it.exists() }?.readBytes()
        val record = File(app.filesDir, "production-session.json")
        val commands = File(app.filesDir, "production-command.json")
        var step = "ready"
        try {
            // Only the temporary local trust root is staged by the harness. The user-facing
            // address/name/save/start steps below all use the real Compose controls.
            val certificate =
                String(
                    android.util.Base64.decode(
                        args.getString("fixture_ca"),
                        android.util.Base64.DEFAULT,
                    ),
                    Charsets.UTF_8,
                )
            app.connection.saveSettings(
                ConnectionSettings(
                    "wss://localhost:1/control",
                    "Before UI",
                    certificate,
                    listOf(relay),
                )
            )
            val activity =
                instrumentation.startActivitySync(
                    Intent(app, MainActivity::class.java)
                        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK)
                )
            val bindingDeadline = SystemClock.elapsedRealtime() + 3000
            while (
                !app.automationBackend.available.value &&
                    SystemClock.elapsedRealtime() < bindingDeadline
            ) SystemClock.sleep(100)
            if (!app.automationBackend.available.value) {
                check(args.getString("accessibility_preconfigured") != "true") {
                    "Enable Pixels device control on the phone before running this test"
                }
                setAccessibility(true)
                showHome()
            }
            find(UiSelector().text(app.getString(R.string.server_settings))).click()
            setField("control_url", control, activity)
            setField("device_name", "Android full Server", activity)
            setField("relay_url", relay, activity)
            find(UiSelector().text(app.getString(R.string.connect))).click()
            if (Build.VERSION.SDK_INT >= 33) {
                val allow =
                    device.findObject(
                        UiSelector().resourceIdMatches(".*:id/permission_allow_button")
                    )
                if (allow.waitForExists(1500)) allow.click()
                else {
                    val miuiAllow = device.findObject(UiSelector().text("允许"))
                    if (miuiAllow.exists()) miuiAllow.click()
                }
            }
            val connectDeadline = SystemClock.elapsedRealtime() + 50000
            while (
                !app.connection.status.value.remoteReady &&
                    SystemClock.elapsedRealtime() < connectDeadline
            ) {
                check(
                    app.connection.status.value.error == null || app.connection.status.value.active
                ) {
                    "Connection failed: ${app.connection.status.value.error}"
                }
                SystemClock.sleep(100)
            }
            assertTrue(
                "Production connection did not become ready",
                app.connection.status.value.remoteReady,
            )
            assertStatus(R.string.status_ready)
            find(UiSelector().text(app.getString(R.string.show_password))).click()
            find(UiSelector().text(app.getString(R.string.hide_password))).click()
            showTarget()
            val deadline =
                SystemClock.elapsedRealtime() +
                    300000 +
                    (args.getString("background_seconds")?.toLongOrNull() ?: 0) * 1000
            var lastCommand = ""
            while (SystemClock.elapsedRealtime() < deadline) {
                val status = app.connection.status.value
                val value =
                    JSONObject()
                        .put("request_id", request)
                        .put("step", step)
                        .put("phase", status.phase.name)
                        .put("remote_ready", status.remoteReady)
                        .put("generation", status.generation)
                        .put("device_code", status.deviceCode)
                        .put("accessibility_ready", app.automationBackend.available.value)
                        .put("paused", app.dispatcher.paused.value)
                if (status.remoteReady) value.put("password", app.connection.accessPassword.value)
                val temporary = File(app.filesDir, "production-session.tmp")
                temporary.writeText(value.toString())
                check(temporary.renameTo(record))
                val command =
                    try {
                        JSONObject(commands.readText())
                    } catch (_: Exception) {
                        null
                    }
                if (
                    command?.optString("request_id") == request &&
                        command.optString("nonce") != lastCommand
                ) {
                    lastCommand = command.getString("nonce")
                    when (command.getString("action")) {
                        "pause",
                        "resume" -> {
                            showHome()
                            val paused = command.getString("action") == "pause"
                            find(
                                    UiSelector()
                                        .text(
                                            app.getString(
                                                if (paused) R.string.pause_control
                                                else R.string.resume_control
                                            )
                                        )
                                )
                                .click()
                            assertTrue(
                                "Control pause did not change",
                                app.dispatcher.paused.value == paused,
                            )
                            assertStatus(
                                if (paused) R.string.status_paused else R.string.status_ready
                            )
                            showTarget()
                        }
                        "permission-off",
                        "permission-on" -> {
                            val enabled = command.getString("action") == "permission-on"
                            setAccessibility(enabled)
                            showHome()
                            assertStatus(
                                if (enabled) R.string.status_ready
                                else R.string.status_permission_required
                            )
                            showTarget()
                        }
                        "screen-off" -> {
                            device.sleep()
                            SystemClock.sleep(500)
                        }
                        "unlock" -> {
                            device.wakeUp()
                            device.pressMenu()
                            showHome()
                            assertStatus(R.string.status_ready)
                            showTarget()
                        }
                        "capture" -> {
                            if (Build.VERSION.SDK_INT >= 30) {
                                showTarget()
                                step = lastCommand
                                continue
                            }
                            showHome()
                            find(UiSelector().text(app.getString(R.string.authorize_capture)))
                                .click()
                            val allow =
                                device.findObject(UiSelector().resourceId("android:id/button1"))
                            assertTrue("System consent dialog missing", allow.waitForExists(5000))
                            allow.click()
                            SystemClock.sleep(500)
                            assertTrue(
                                "Capture state did not become active",
                                app.projectionCapture.active.value,
                            )
                            showTarget()
                        }
                        "stop-capture" -> {
                            showHome()
                            find(UiSelector().text(app.getString(R.string.stop_capture))).click()
                            val stopDeadline = SystemClock.elapsedRealtime() + 5000
                            while (
                                app.projectionCapture.active.value &&
                                    SystemClock.elapsedRealtime() < stopDeadline
                            ) SystemClock.sleep(100)
                            assertTrue("Capture did not stop", !app.projectionCapture.active.value)
                            showTarget()
                        }
                        "background" -> {
                            instrumentation.runOnMainSync { activity.finish() }
                            device.pressHome()
                        }
                        "target" -> showTarget()
                        "disconnect" -> {
                            showHome()
                            find(UiSelector().text(app.getString(R.string.disconnect))).click()
                            val stopDeadline = SystemClock.elapsedRealtime() + 15000
                            while (
                                app.connection.status.value.active &&
                                    SystemClock.elapsedRealtime() < stopDeadline
                            ) SystemClock.sleep(100)
                            assertTrue(
                                "Connection did not stop",
                                !app.connection.status.value.active,
                            )
                            assertStatus(R.string.status_disconnected)
                        }
                        "finish" -> break
                        else -> error("Unsupported local test command")
                    }
                    step = lastCommand
                }
                SystemClock.sleep(100)
            }
            assertTrue(
                "Host did not finish the acceptance",
                commands.exists() &&
                    JSONObject(commands.readText()).getString("action") == "finish",
            )
        } finally {
            ConnectionService.stop(app)
            app.stopService(
                Intent(app, com.pixels.agent.platform.capture.ProjectionCaptureService::class.java)
            )
            val stopDeadline = SystemClock.elapsedRealtime() + 15000
            while (
                app.connection.status.value.active && SystemClock.elapsedRealtime() < stopDeadline
            ) SystemClock.sleep(100)
            if (original == null) settingsFile.delete() else settingsFile.writeBytes(original)
            app.dispatcher.setPaused(false)
            record.delete()
            commands.delete()
            device.wakeUp()
            device.pressMenu()
            if (
                !app.automationBackend.available.value &&
                    args.getString("accessibility_preconfigured") != "true"
            )
                setAccessibility(true)
        }
    }

    private fun assertStatus(message: Int) {
        val status = find(UiSelector().resourceId("control_status"))
        val deadline = SystemClock.elapsedRealtime() + 5000
        while (status.text != app.getString(message) && SystemClock.elapsedRealtime() < deadline) {
            SystemClock.sleep(100)
        }
        assertTrue(
            "Home availability message did not match system state",
            status.text == app.getString(message),
        )
    }

    private fun setAccessibility(enabled: Boolean) {
        app.startActivity(
            Intent(android.provider.Settings.ACTION_ACCESSIBILITY_SETTINGS)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK)
        )
        SystemClock.sleep(500)
        find(UiSelector().text("Pixels device control")).click()
        val toggle = device.findObject(UiSelector().className("android.widget.Switch"))
        assertTrue("Accessibility switch missing", toggle.waitForExists(3000))
        fun changeSwitch() {
            toggle.click()
            val allow = device.findObject(UiSelector().resourceId("android:id/button1"))
            assertTrue("Accessibility confirmation missing", allow.waitForExists(3000))
            allow.click()
            SystemClock.sleep(300)
        }
        // Some emulators retain an enabled switch after the old service process is gone.
        // Recover through the same system controls shown to the user, never secure settings.
        if (enabled && toggle.isChecked && !app.automationBackend.available.value) changeSwitch()
        if (toggle.isChecked != enabled) changeSwitch()
        val deadline = SystemClock.elapsedRealtime() + 5000
        while (
            app.automationBackend.available.value != enabled &&
                SystemClock.elapsedRealtime() < deadline
        ) {
            SystemClock.sleep(100)
        }
        assertTrue(
            "Accessibility binding did not change",
            app.automationBackend.available.value == enabled,
        )
    }

    private fun setField(tag: String, value: String, activity: android.app.Activity) {
        assertTrue(
            "Could not enter connection setting",
            find(UiSelector().resourceId(tag)).setText(value),
        )
        instrumentation.runOnMainSync {
            activity
                .getSystemService(InputMethodManager::class.java)
                .hideSoftInputFromWindow(activity.window.decorView.windowToken, 0)
        }
    }

    private fun showHome() {
        app.startActivity(
            Intent(app, MainActivity::class.java)
                .addFlags(
                    Intent.FLAG_ACTIVITY_NEW_TASK or
                        Intent.FLAG_ACTIVITY_CLEAR_TOP or
                        Intent.FLAG_ACTIVITY_SINGLE_TOP
                )
        )
        instrumentation.waitForIdleSync()
        SystemClock.sleep(500)
    }

    private fun showTarget() {
        app.startActivity(
            Intent(app, AutomationFixtureActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_REORDER_TO_FRONT)
        )
        SystemClock.sleep(300)
    }

    private fun find(selector: UiSelector): UiObject {
        val target = device.findObject(selector)
        for (direction in listOf(1, -1)) {
            repeat(12) {
                if (target.exists()) return target
                val x = device.displayWidth * 9 / 10
                val start = if (direction == 1) 4 else 1
                val end = if (direction == 1) 1 else 4
                device.swipe(
                    x,
                    device.displayHeight * start / 5,
                    x,
                    device.displayHeight * end / 5,
                    20,
                )
                SystemClock.sleep(100)
            }
        }
        error("Home screen control not found")
    }
}
