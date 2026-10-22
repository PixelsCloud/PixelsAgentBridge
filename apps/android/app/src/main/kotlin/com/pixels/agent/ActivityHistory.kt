package com.pixels.agent

import com.pixels.agent.core.ConnectionSettings
import com.pixels.agent.core.activityOutcome
import com.pixels.agent.transport.NativeBridge
import java.io.File
import java.security.MessageDigest
import org.json.JSONObject

internal enum class ActivityKind {
    INTERFACE,
    NOTIFICATION,
    SYSTEM,
    FILE,
    DOCUMENT,
    EXECUTION,
}

internal typealias ActivityOutcome = com.pixels.agent.core.ActivityOutcome

internal data class ActivityRecord(
    val id: String,
    val kind: ActivityKind,
    val outcome: ActivityOutcome,
    val timestamp: Long?,
)

internal data class ActivityHistory(
    val records: List<ActivityRecord> = emptyList(),
    val unavailable: Boolean = false,
)

/**
 * Only the existing Rust SQLite runtime may read its live journals. Never use Android SQLite here.
 */
internal class ActivityHistoryReader(
    private val directory: File,
    private val native: NativeBridge,
) {
    private var cachedPath: File? = null
    private var cachedStamp: List<Long> = emptyList()
    private var cached = ActivityHistory()

    @Synchronized
    fun read(settings: ConnectionSettings): ActivityHistory {
        if (settings.controlUrl.isBlank()) return ActivityHistory()
        val encoded =
            JSONObject(
                native.encodeSettings(
                    JSONObject()
                        .put("schema_version", 1)
                        .put("control_url", settings.controlUrl)
                        .put("device_name", settings.deviceName)
                        .toString()
                )
            )
        check(encoded.getBoolean("ok"))
        val scope = encoded.getJSONObject("value").getString("scope")
        val hash =
            MessageDigest.getInstance("SHA-256").digest(scope.toByteArray()).joinToString("") {
                "%02x".format(it)
            }
        val runtime = File(directory, "runtime/$hash")
        val names =
            listOf(
                "mobile.sqlite",
                "mobile.system.sqlite",
                "mobile.files.sqlite",
                "mobile.documents.sqlite",
                "mobile.execution.sqlite",
            )
        val stamp = names.flatMap { name ->
            listOf(File(runtime, name), File(runtime, "$name-wal")).flatMap { file ->
                listOf(file.lastModified(), file.length())
            }
        }
        if (cachedPath == runtime && cachedStamp == stamp && !cached.unavailable) return cached
        val reply = JSONObject(native.recentActivity(runtime.absolutePath))
        check(reply.getBoolean("ok"))
        val value = reply.getJSONObject("value")
        val rows = value.getJSONArray("records")
        val records =
            (0 until rows.length()).map { index ->
                val record = rows.getJSONObject(index)
                val kind = record.optString("kind")
                val category =
                    when {
                        kind.startsWith("notification_") -> ActivityKind.NOTIFICATION
                        kind.startsWith("ui_") ||
                            kind.startsWith("desktop") ||
                            kind.startsWith("window_") ||
                            kind in setOf("type_text", "monitor_input", "monitors") ->
                            ActivityKind.INTERFACE
                        else ->
                            when (record.getString("source")) {
                                "file" -> ActivityKind.FILE
                                "document" -> ActivityKind.DOCUMENT
                                "execution" -> ActivityKind.EXECUTION
                                "interface" -> ActivityKind.INTERFACE
                                else -> ActivityKind.SYSTEM
                            }
                    }
                ActivityRecord(
                    record.getString("id"),
                    category,
                    activityOutcome(
                        record.optString("state"),
                        record.optString("error"),
                        record.optString("outcome"),
                    ),
                    record.optLong("timestamp").takeIf { it > 0L },
                )
            }
        cachedPath = runtime
        cachedStamp = stamp
        cached = ActivityHistory(records, value.optBoolean("unavailable"))
        return cached
    }
}
