package com.pixels.agent.fixture

import com.pixels.agent.PixelsApplication
import com.pixels.agent.RemoteAutomationAdapter
import com.pixels.agent.transport.AndroidConnectionRepository
import java.io.File
import java.util.UUID
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject

/** Fixed debug fixture: accepts only an app-private loopback configuration, never public input. */
internal object RemoteFixture {
    suspend fun run(activity: AutomationFixtureActivity, request: String) = coroutineScope {
        val requestId = UUID.fromString(request).toString()
        val app = activity.application as PixelsApplication
        val directory = File(app.noBackupFilesDir, "remote-tests/$requestId").apply { mkdirs() }
        val source =
            withContext(Dispatchers.IO) {
                val file = File(app.filesDir, "remote-fixture.toml")
                check(file.length() <= 65536)
                file.readText()
            }
        val decoded = JSONObject(app.nativeBridge.decodeSettings(source))
        check(decoded.getBoolean("ok"))
        val config = decoded.getJSONObject("value").getJSONObject("config")
        check(java.net.URI(config.getString("control_url")).host == "localhost")
        val relays = config.getJSONArray("relay_urls")
        check(relays.length() == 1 && java.net.URI(relays.getString(0)).host == "localhost")
        withContext(Dispatchers.IO) { File(directory, "settings.toml").writeText(source) }
        val repository =
            AndroidConnectionRepository(
                directory,
                app.nativeBridge,
                RemoteAutomationAdapter(
                    app.dispatcher,
                    app.screenCapture,
                    app.getSystemService(android.app.KeyguardManager::class.java),
                    app.getSystemService(android.os.PowerManager::class.java),
                ),
            )
        val session = launch { repository.runConnection() }
        try {
            repository.status.collectLatest { state ->
                val record =
                    JSONObject()
                        .put("request_id", requestId)
                        .put("phase", state.phase.name)
                        .put("error", state.error)
                        .put("ready", state.remoteReady)
                        .put("accessibility_ready", app.automationBackend.available.value)
                if (state.remoteReady) {
                    record.put("device_code", state.deviceCode)
                    record.put("password", repository.accessPassword.value)
                }
                withContext(Dispatchers.IO) {
                    File(app.filesDir, "remote-fixture-access.json").writeText(record.toString())
                }
            }
        } finally {
            session.cancel()
        }
    }
}
