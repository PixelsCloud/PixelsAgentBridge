package com.pixels.agent.fixture

import android.app.NotificationManager
import android.util.AtomicFile
import com.pixels.agent.PixelsApplication
import com.pixels.agent.R
import com.pixels.agent.core.AutomationResult
import java.io.File
import java.util.UUID
import org.json.JSONArray
import org.json.JSONObject

/** ADB-only home setup metadata. External uiautomator dumps suppress the real service. */
internal object HomeSnapshot {
    fun recordStatus(app: PixelsApplication, request: String) {
        val id = UUID.fromString(request).toString()
        val status = app.connection.status.value
        write(
            app,
            "normal-status.json",
            JSONObject()
                .put("request_id", id)
                .put("phase", status.phase.name)
                .put("remote_ready", status.remoteReady)
                .put("generation", status.generation)
                .put("paused", app.dispatcher.paused.value)
                .put("connection_requested", app.controlIntent.state.value.connectionRequested)
                .put("remote_control_allowed", app.remoteControlAllowed.get())
                .put("accessibility_ready", app.automationBackend.available.value)
                .put(
                    "notifications_enabled",
                    app.getSystemService(NotificationManager::class.java).areNotificationsEnabled(),
                )
                .put("pid", android.os.Process.myPid())
                .put("elapsed_ms", android.os.SystemClock.elapsedRealtime()),
        )
    }

    suspend fun record(app: PixelsApplication, request: String) {
        val id = UUID.fromString(request).toString()
        val result = app.automationBackend.observe()
        val value = JSONObject().put("request_id", id)
        if (result is AutomationResult.Observed && result.screen.packageName == app.packageName) {
            val labelIds =
                setOf(
                    R.string.app_name,
                    R.string.connect,
                    R.string.disconnect,
                    R.string.pause_control,
                    R.string.resume_control,
                    R.string.show_password,
                    R.string.hide_password,
                    R.string.open_test_target,
                    R.string.rotate_password,
                    R.string.reload_settings,
                    R.string.permission_setup_title,
                    R.string.permission_setup_continue,
                    R.string.permission_setup_later,
                    R.string.permission_setup_workspace_only,
                    R.string.permission_setup_check,
                    R.string.permission_setup_ready,
                    R.string.permission_setup_missing,
                    R.string.package_install_settings,
                    R.string.startup_app_settings,
                    R.string.startup_battery_settings,
                    R.string.control_url,
                    R.string.relay_url,
                    R.string.files_revoke,
                    R.string.files_make_read_only,
                )
            val productIds =
                R.string::class
                    .java
                    .fields
                    .filter { it.name.startsWith("product_") }
                    .map { it.getInt(null) }
            val labels = localizedLabels(app, labelIds + productIds)
            value
                .put("width", result.screen.geometry.width)
                .put("height", result.screen.geometry.height)
                .put("service_ready", app.automationBackend.available.value)
                .put("remote_ready", app.connection.status.value.remoteReady)
                .put("paused", app.dispatcher.paused.value)
                .put("phase", app.connection.status.value.phase.name)
            val notifications = app.getSystemService(NotificationManager::class.java)
            value
                .put("notifications_enabled", notifications.areNotificationsEnabled())
                .put(
                    "notification_stop_available",
                    notifications.activeNotifications.any {
                        it.id == 2 &&
                            it.notification.actions?.any { action ->
                                action.title.toString() == app.getString(R.string.disconnect)
                            } == true
                    },
                )
            val nodes = JSONArray()
            for (element in result.screen.elements) {
                // Never expose input values, passwords, device codes, or unrelated screen text.
                nodes.put(
                    JSONObject()
                        .put("text", element.name?.takeIf { it in labels } ?: "")
                        .put("role", element.role)
                        .put("enabled", element.enabled)
                        .put("clickable", element.clickable)
                        .put("selected", element.selected)
                        .put(
                            "identifier",
                            element.identifier?.takeIf {
                                it in
                                    setOf(
                                        "device_code",
                                        "access_password",
                                        "control_status",
                                        "device_name",
                                        "control_url",
                                        "relay_url",
                                        "connect_device",
                                        "activity_filters",
                                        "activity_filter_0",
                                        "activity_filter_1",
                                        "activity_filter_2",
                                        "activity_list",
                                    )
                            } ?: "",
                        )
                        .put("left", element.bounds.left)
                        .put("top", element.bounds.top)
                        .put("right", element.bounds.right)
                        .put("bottom", element.bounds.bottom)
                )
            }
            value.put("elements", nodes)
        } else value.put("error", "home_snapshot_unavailable")
        write(app, "home-snapshot.json", value)
    }

    suspend fun recordRevealedCredentials(app: PixelsApplication, request: String) {
        val id = UUID.fromString(request).toString()
        val result =
            app.automationBackend.observe() as? AutomationResult.Observed
                ?: error("home_snapshot_unavailable")
        check(
            result.screen.packageName == app.packageName && app.connection.status.value.remoteReady
        )
        check(
            result.screen.elements.any {
                it.name in localizedLabels(app, setOf(R.string.hide_password))
            }
        ) {
            "password_must_be_revealed_through_normal_ui"
        }
        // Compose exposes a field's accessibility label separately from its value.
        // Read the exact value bound to the normal UI only after its reveal action.
        val password = requireNotNull(app.connection.accessPassword.value)
        val code = requireNotNull(app.connection.status.value.deviceCode)
        write(
            app,
            "home-credentials.json",
            JSONObject().put("request_id", id).put("device_code", code).put("password", password),
        )
    }

    private fun write(app: PixelsApplication, name: String, value: JSONObject) {
        val file = AtomicFile(File(app.filesDir, name))
        val stream = file.startWrite()
        try {
            stream.write(value.toString().toByteArray())
            file.finishWrite(stream)
        } catch (error: Exception) {
            file.failWrite(stream)
            throw error
        }
    }

    private fun localizedLabels(app: PixelsApplication, ids: Collection<Int>): Set<String> =
        listOf(java.util.Locale.SIMPLIFIED_CHINESE, java.util.Locale.ENGLISH)
            .flatMap { locale ->
                val config =
                    android.content.res.Configuration(app.resources.configuration).apply {
                        setLocale(locale)
                    }
                val context = app.createConfigurationContext(config)
                ids.map(context::getString)
            }
            .toSet()
}
