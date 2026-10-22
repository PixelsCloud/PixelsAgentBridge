package com.pixels.agent

import android.content.Intent
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class StartupRecoveryDeviceTest {
    @Test
    fun startupRespectsDisabledAndStoppedStatesAndMigratesLegacyPause() = runBlocking {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val app = instrumentation.targetContext.applicationContext as PixelsApplication
        app.controlIntent.load()
        app.productPreferences.load()
        check(!app.productPreferences.failed.value)
        val originalIntent = app.controlIntent.state.value
        val originalPreferences = app.productPreferences.state.value
        val preferenceFile = File(app.noBackupFilesDir, "appearance.toml")
        val originalBytes = preferenceFile.takeIf { it.exists() }?.readBytes()
        val activity =
            instrumentation.startActivitySync(
                Intent(app, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            )
        suspend fun stopped() {
            app.stopService(Intent(app, ConnectionService::class.java))
            withTimeout(10000) { while (app.connection.status.value.active) delay(100) }
        }
        try {
            // Simulate a saved pause from the retired control toggle.
            app.controlIntent.setPaused(true)
            app.controlIntent.setConnectionRequested(true)
            app.productPreferences.save(originalPreferences.copy(startOnBoot = false))
            stopped()
            ConnectionService.restore(app)
            delay(1500)
            assertFalse(app.connection.status.value.active)
            assertTrue(app.controlIntent.state.value.connectionRequested)

            app.controlIntent.setConnectionRequested(false)
            app.productPreferences.save(originalPreferences.copy(startOnBoot = true))
            ConnectionService.restore(app)
            delay(1500)
            assertFalse(app.connection.status.value.active)
            assertFalse(app.controlIntent.state.value.connectionRequested)

            app.controlIntent.setConnectionRequested(true)
            ConnectionService.restore(app)
            withTimeout(30000) { while (!app.connection.status.value.remoteReady) delay(100) }
            assertFalse(app.dispatcher.paused.value)
            assertFalse(app.controlIntent.state.value.paused)
            assertEquals(true, app.productPreferences.state.value.startOnBoot)
        } finally {
            stopped()
            app.controlIntent.setConnectionRequested(originalIntent.connectionRequested)
            app.controlIntent.setPaused(originalIntent.paused)
            if (originalBytes == null) preferenceFile.delete()
            else preferenceFile.writeBytes(originalBytes)
            app.productPreferences.load()
            if (originalIntent.connectionRequested) ConnectionService.start(app)
            instrumentation.runOnMainSync { activity.finish() }
        }
    }

    @Test
    fun loadingLegacyPausePreservesConnectionChoice() = runBlocking {
        val app =
            InstrumentationRegistry.getInstrumentation().targetContext.applicationContext
                as PixelsApplication
        app.controlIntent.load()
        val file = File(app.noBackupFilesDir, "control-intent.toml")
        val original = file.takeIf { it.exists() }?.readBytes()
        try {
            for (requested in listOf(false, true)) {
                app.controlIntent.setConnectionRequested(requested)
                app.controlIntent.setPaused(true)
                assertTrue(app.dispatcher.paused.value)

                app.controlIntent.load()

                assertFalse(app.dispatcher.paused.value)
                assertFalse(app.controlIntent.state.value.paused)
                assertEquals(requested, app.controlIntent.state.value.connectionRequested)
                val intent =
                    org.json
                        .JSONObject(app.nativeBridge.decodeUserIntent(file.readText()))
                        .getJSONObject("value")
                        .getJSONObject("intent")
                assertFalse(intent.getBoolean("paused"))
                assertEquals(requested, intent.getBoolean("connection_requested"))
            }
        } finally {
            if (original == null) file.delete() else file.writeBytes(original)
            app.controlIntent.load()
        }
    }
}
