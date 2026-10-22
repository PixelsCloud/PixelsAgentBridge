package com.pixels.agent.fixture

import android.app.Activity
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.util.AtomicFile
import com.pixels.agent.ConnectionService
import com.pixels.agent.PixelsApplication
import com.pixels.agent.core.ConnectionSettings
import java.io.File
import java.util.UUID
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.json.JSONObject

/** Debug setup only. The ordinary ConnectionService is the sole connection owner. */
class ProductionProbeActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val request = intent.getStringExtra("production_request_id").orEmpty()
        CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate).launch {
            try {
                if (intent.getBooleanExtra("probe_restart", false))
                    ProductionProbe.restart(application as PixelsApplication, request)
                else ProductionProbe.prepare(application as PixelsApplication, request)
                // Starting from a visible Activity follows the same foreground-service rules
                // as the real home screen. No test coroutine runs the connection itself.
                ConnectionService.start(this@ProductionProbeActivity)
                startActivity(
                    Intent(this@ProductionProbeActivity, AutomationFixtureActivity::class.java)
                )
            } catch (_: Exception) {
                ProductionProbe.failure(application as PixelsApplication, request)
            } finally {
                finish()
            }
        }
    }
}

/** Fixed, debug-only actions; configuration must already exist in the app-private directory. */
class ProductionProbeReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val pending = goAsync()
        val app = context.applicationContext as PixelsApplication
        CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate).launch {
            try {
                val request = intent.getStringExtra("production_request_id").orEmpty()
                when (intent.getStringExtra("probe_action")) {
                    "slow-storage-fixture" -> {
                        UUID.fromString(request)
                        app.packageManager.setComponentEnabledSetting(
                            android.content.ComponentName(app, SlowDocumentsProvider::class.java),
                            if (intent.getBooleanExtra("fixture_enabled", false))
                                android.content.pm.PackageManager.COMPONENT_ENABLED_STATE_ENABLED
                            else android.content.pm.PackageManager.COMPONENT_ENABLED_STATE_DISABLED,
                            android.content.pm.PackageManager.DONT_KILL_APP,
                        )
                        return@launch
                    }
                    "file-view-fixture" -> {
                        UUID.fromString(request)
                        app.packageManager.setComponentEnabledSetting(
                            android.content.ComponentName(app, FileViewFixtureActivity::class.java),
                            if (intent.getBooleanExtra("fixture_enabled", false))
                                android.content.pm.PackageManager.COMPONENT_ENABLED_STATE_ENABLED
                            else android.content.pm.PackageManager.COMPONENT_ENABLED_STATE_DISABLED,
                            android.content.pm.PackageManager.DONT_KILL_APP,
                        )
                        return@launch
                    }
                    "normal-status" -> {
                        HomeSnapshot.recordStatus(app, request)
                        return@launch
                    }
                    "home-credentials" -> {
                        HomeSnapshot.recordRevealedCredentials(app, request)
                        return@launch
                    }
                    "notification-stop" -> {
                        UUID.fromString(request)
                        val manager =
                            app.getSystemService(android.app.NotificationManager::class.java)
                        val action =
                            manager.activeNotifications
                                .single { it.id == 2 }
                                .notification
                                .actions
                                .orEmpty()
                                .single {
                                    it.title.toString() ==
                                        app.getString(com.pixels.agent.R.string.disconnect)
                                }
                        requireNotNull(action.actionIntent).send()
                        return@launch
                    }
                    "home-snapshot" -> {
                        HomeSnapshot.record(app, request)
                        return@launch
                    }
                    "soak-start" -> {
                        DebugSoakSampler.start(app, request)
                        return@launch
                    }
                    "soak-stop" -> {
                        DebugSoakSampler.stop(request)
                        return@launch
                    }
                }
                ProductionProbe.command(
                    app,
                    intent.getStringExtra("production_request_id").orEmpty(),
                    intent.getStringExtra("probe_action").orEmpty(),
                    intent.getStringExtra("probe_nonce").orEmpty(),
                )
            } catch (_: Exception) {
                ProductionProbe.failure(
                    app,
                    intent.getStringExtra("production_request_id").orEmpty(),
                )
            } finally {
                pending.finish()
            }
        }
    }
}

internal object ProductionProbe {
    private var activeRequest: String? = null
    private var step = "ready"
    private var targetDestroyed = false

    fun targetCreated() {
        targetDestroyed = false
    }

    fun targetDestroyed() {
        targetDestroyed = true
    }

    fun restart(app: PixelsApplication, request: String) {
        recover(app, request)
        check(activeRequest == request && !app.connection.status.value.active)
    }

    private fun recover(app: PixelsApplication, request: String) {
        if (activeRequest != null) return
        val id = UUID.fromString(request).toString()
        val marker = File(app.noBackupFilesDir, "production-probe-backup/request")
        if (marker.exists() && marker.readText() == id) activeRequest = id
    }

    suspend fun prepare(app: PixelsApplication, request: String) {
        val id = UUID.fromString(request).toString()
        check(!app.connection.status.value.active) { "connection_already_active" }
        val backup = File(app.noBackupFilesDir, "production-probe-backup")
        check(!backup.exists()) { "previous_probe_requires_cleanup" }
        val settings = File(app.noBackupFilesDir, "settings.toml")
        check(backup.mkdirs())
        if (settings.exists()) settings.copyTo(File(backup, "settings.toml"))
        val intent = File(app.noBackupFilesDir, "control-intent.toml")
        if (intent.exists()) intent.copyTo(File(backup, "control-intent.toml"))
        File(backup, "request").writeText(id)
        activeRequest = id
        val source = File(app.filesDir, "production-fixture.toml")
        check(source.length() <= 65536)
        val decoded = JSONObject(app.nativeBridge.decodeSettings(source.readText()))
        check(decoded.getBoolean("ok"))
        val config = decoded.getJSONObject("value").getJSONObject("config")
        val control = config.getString("control_url")
        val relays = config.getJSONArray("relay_urls")
        check(java.net.URI(control).host == "localhost")
        check(relays.length() == 1 && java.net.URI(relays.getString(0)).host == "localhost")
        app.connection.saveSettings(
            ConnectionSettings(
                control,
                "Android production probe",
                config.getString("trusted_ca_pem"),
                listOf(relays.getString(0)),
            )
        )
        app.controlIntent.setPaused(false)
        record(app, id)
    }

    suspend fun command(app: PixelsApplication, request: String, action: String, nonce: String) {
        recover(app, request)
        if (activeRequest != request) return
        when (action) {
            "status",
            "checkpoint" -> Unit
            "pause" -> app.controlIntent.setPaused(true)
            "resume" -> app.controlIntent.setPaused(false)
            "disconnect",
            "finish" -> {
                ConnectionService.stop(app)
                repeat(120) {
                    if (!app.connection.status.value.active) return@repeat
                    delay(50)
                }
                check(!app.connection.status.value.active) { "connection_did_not_stop" }
                if (action == "finish") restore(app, request)
            }
            else -> return
        }
        if (action != "status") step = nonce
        record(app, request)
    }

    fun failure(app: PixelsApplication, request: String) {
        write(
            app,
            JSONObject().put("request_id", request).put("error", "production_probe_failed"),
        )
    }

    private suspend fun restore(app: PixelsApplication, request: String) {
        val backup = File(app.noBackupFilesDir, "production-probe-backup")
        check(File(backup, "request").readText() == request)
        val original = File(backup, "settings.toml")
        val settings = File(app.noBackupFilesDir, "settings.toml")
        if (original.exists()) original.copyTo(settings, overwrite = true) else settings.delete()
        val originalIntent = File(backup, "control-intent.toml")
        val intent = File(app.noBackupFilesDir, "control-intent.toml")
        if (originalIntent.exists()) originalIntent.copyTo(intent, overwrite = true)
        else intent.delete()
        app.connection.loadSettings()
        app.controlIntent.load()
        // Only files created by this probe are removed; never delete an arbitrary subtree.
        original.delete()
        originalIntent.delete()
        File(backup, "request").delete()
        check(backup.delete())
        activeRequest = null
    }

    private fun record(app: PixelsApplication, request: String) {
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
                .put("target_destroyed", targetDestroyed)
        if (status.remoteReady) value.put("password", app.connection.accessPassword.value)
        write(app, value)
    }

    private fun write(app: PixelsApplication, value: JSONObject) {
        val file = AtomicFile(File(app.filesDir, "production-session.json"))
        val stream = file.startWrite()
        try {
            stream.write(value.toString().toByteArray())
            file.finishWrite(stream)
        } catch (error: Exception) {
            file.failWrite(stream)
            throw error
        }
    }
}
