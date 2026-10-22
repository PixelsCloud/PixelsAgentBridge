package com.pixels.agent.platform.system

import android.content.Context
import android.os.Process
import android.os.SystemClock
import android.util.Base64
import com.pixels.agent.core.StorageFailure
import java.io.File
import java.security.MessageDigest
import java.util.UUID
import java.util.concurrent.TimeUnit
import org.json.JSONObject

/** Bounded, caller-bound snapshots of the current app PID's Android main log buffer. */
internal class AndroidAppLogs(private val context: Context) {
    private data class Event(
        val ref: String,
        val time: Long,
        val level: String,
        val message: ByteArray,
    )

    private data class Snapshot(
        val owner: String,
        val created: Long,
        val events: List<Event>,
        val scanned: Int,
    )

    private val snapshots = LinkedHashMap<String, Snapshot>()
    private val sourceId = "android:current-app:main"

    private fun hash(bytes: ByteArray) =
        MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") {
            "%02x".format(it.toInt() and 255)
        }

    private fun source() =
        obj(
            "source_id" to sourceId,
            "source_kind" to "android_logcat",
            "source_locator" to "main:pid:${Process.myPid()}",
            "availability" to "queryable",
            "reason" to null,
            "execution_identity" to "uid:${Process.myUid()}:pid:${Process.myPid()}",
            "coverage_limit" to
                "Current app PID only, last 256 available main-buffer lines, bounded snapshot; not system logs or durable history.",
        )

    fun execute(query: JSONObject, owner: String, device: JSONObject): JSONObject =
        synchronized(snapshots) {
            snapshots.entries.removeAll {
                SystemClock.elapsedRealtime() - it.value.created > 600000
            }
            when (query.getString("action")) {
                "sources" -> {
                    if (!query.isNull("scan_cursor"))
                        throw StorageFailure("log_scan_cursor_unavailable")
                    val after = query.optionalString("after")
                    val entries =
                        if (after == null || after < sourceId) listOf(source()) else emptyList()
                    response(
                        obj(
                            "type" to "logs",
                            "snapshot" to
                                obj(
                                    "type" to "sources",
                                    "sources" to array(entries),
                                    "progress" to null,
                                    "next_source" to null,
                                    "total" to 1,
                                ),
                        ),
                        entries.size,
                        listOf(
                            "Source visibility is limited to this app's current PID. Read errors are reported without falling back to ADB or global log access."
                        ),
                    )
                }
                "read" -> read(query, owner, device)
                "event_read" -> {
                    val ref = query.getString("event_ref")
                    val event =
                        snapshots.values
                            .filter { it.owner == owner }
                            .asSequence()
                            .flatMap { it.events.asSequence() }
                            .firstOrNull { it.ref == ref }
                            ?: throw StorageFailure("log_event_expired_or_wrong_caller")
                    val offset = query.getLong("offset")
                    if (offset < 0 || offset > event.message.size)
                        throw StorageFailure("invalid_log_offset")
                    val end =
                        (offset + query.getLong("max_bytes"))
                            .coerceAtMost(event.message.size.toLong())
                            .toInt()
                    val bytes = event.message.copyOfRange(offset.toInt(), end)
                    response(
                        obj(
                            "type" to "logs",
                            "snapshot" to
                                obj(
                                    "type" to "event_chunk",
                                    "event_ref" to ref,
                                    "offset" to offset,
                                    "next_offset" to end,
                                    "total_bytes" to event.message.size,
                                    "data_base64" to Base64.encodeToString(bytes, Base64.NO_WRAP),
                                    "sha256" to hash(event.message),
                                ),
                        )
                    )
                }
                else -> throw StorageFailure("android_log_action_unavailable")
            }
        }

    private fun capture(): List<Event> {
        val output = File.createTempFile("log-query-", ".tmp", context.cacheDir)
        try {
            val child =
                ProcessBuilder(
                        "/system/bin/logcat",
                        "-d",
                        "-b",
                        "main",
                        "-v",
                        "epoch",
                        "-t",
                        "256",
                        "--pid=${Process.myPid()}",
                    )
                    .redirectErrorStream(true)
                    .redirectOutput(output)
                    .start()
            try {
                if (!child.waitFor(3, TimeUnit.SECONDS)) {
                    child.destroyForcibly()
                    throw StorageFailure("app_log_timeout")
                }
                if (child.exitValue() != 0 || output.length() > 2 * 1024 * 1024)
                    throw StorageFailure("app_log_unavailable_or_byte_limit")
                val pattern =
                    Regex("^\\s*(\\d+\\.\\d+)\\s+(\\d+)\\s+(\\d+)\\s+([VDIWEFAS])\\s+(.*)$")
                return output.useLines { lines ->
                    lines
                        .take(512)
                        .mapNotNull { line ->
                            val raw = line.take(8192)
                            val match = pattern.matchEntire(raw) ?: return@mapNotNull null
                            if (match.groupValues[2].toIntOrNull() != Process.myPid())
                                return@mapNotNull null
                            val bytes = raw.toByteArray(Charsets.UTF_8)
                            Event(
                                UUID.randomUUID().toString(),
                                (match.groupValues[1].toDouble() * 1000).toLong(),
                                match.groupValues[4],
                                bytes,
                            )
                        }
                        .take(256)
                        .toList()
                }
            } finally {
                if (child.isAlive) child.destroyForcibly()
            }
        } finally {
            output.delete()
        }
    }

    private fun read(query: JSONObject, owner: String, device: JSONObject): JSONObject {
        if (query.getString("mode") != "range")
            throw StorageFailure("android_log_follow_unavailable")
        val cursor = query.optionalString("cursor")
        val token: String
        val offset: Int
        val snapshot: Snapshot
        if (cursor == null) {
            val sources = query.optJSONArray("sources")
            if (
                sources != null &&
                    (0 until sources.length()).any { sources.getString(it) != sourceId }
            )
                throw StorageFailure("log_source_not_visible")
            val start = if (query.isNull("start_unix_ms")) null else query.getLong("start_unix_ms")
            val end = if (query.isNull("end_unix_ms")) null else query.getLong("end_unix_ms")
            val filter = query.optJSONObject("filter")
            val needle = filter?.optionalString("message_contains")
            val level = filter?.optionalString("native_level")
            val captured = capture()
            val events = captured.filter {
                (start == null || it.time >= start) &&
                    (end == null || it.time < end) &&
                    (level == null || it.level == level) &&
                    (needle == null || it.message.toString(Charsets.UTF_8).contains(needle))
            }
            while (snapshots.size >= 8) snapshots.remove(snapshots.keys.first())
            token = UUID.randomUUID().toString()
            offset = 0
            snapshot = Snapshot(owner, SystemClock.elapsedRealtime(), events, captured.size)
            snapshots[token] = snapshot
        } else {
            val parts = cursor.split(":")
            if (parts.size != 2) throw StorageFailure("invalid_log_cursor")
            token = parts[0]
            offset = parts[1].toIntOrNull() ?: throw StorageFailure("invalid_log_cursor")
            snapshot =
                snapshots[token]?.takeIf { it.owner == owner }
                    ?: throw StorageFailure("log_cursor_expired_or_wrong_caller")
            if (offset < 0 || offset > snapshot.events.size)
                throw StorageFailure("invalid_log_cursor")
        }
        val selected = snapshot.events.drop(offset).take(query.getInt("limit").coerceAtMost(16))
        val events = selected.map { event ->
            obj(
                "device_ref" to device,
                "event_ref" to event.ref,
                "source_id" to sourceId,
                "source_kind" to "android_logcat",
                "source_locator" to "main:pid:${Process.myPid()}",
                "execution_identity" to "uid:${Process.myUid()}:pid:${Process.myPid()}",
                "native_position" to null,
                "timestamp_unix_ms" to event.time,
                "native_level" to event.level,
                "message_preview" to event.message.toString(Charsets.UTF_8).take(256),
                "total_bytes" to event.message.size,
                "sha256" to hash(event.message),
            )
        }
        val next = offset + selected.size
        val more = next < snapshot.events.size
        return response(
            obj(
                "type" to "logs",
                "snapshot" to
                    obj(
                        "type" to "page",
                        "events" to array(events),
                        "sources" to
                            array(
                                listOf(
                                    obj(
                                        "source_id" to sourceId,
                                        "integrity" to "partial",
                                        "native_position" to null,
                                        "scanned_count" to snapshot.scanned,
                                        "matched_count" to snapshot.events.size,
                                        "reason" to "bounded_current_pid_ring_buffer_snapshot",
                                    )
                                )
                            ),
                        "source_total" to 1,
                        "source_progress_truncated" to false,
                        "integrity" to "partial",
                        "capture_state" to null,
                        "cursor" to if (more) "$token:$next" else null,
                        "has_more" to more,
                        "oldest_unread_age_ms" to null,
                    ),
            ),
            events.size,
            listOf(
                "Bounded current-PID logcat snapshot; history may have rotated and formatted events are bounded to 8192 characters. Integrity remains partial; follow is unavailable."
            ),
        )
    }
}
