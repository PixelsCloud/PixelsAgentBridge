package com.pixels.agent.platform.notifications

import android.app.KeyguardManager
import android.app.Notification
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.ComponentName
import android.content.Context
import android.os.PowerManager
import android.os.SystemClock
import android.service.notification.StatusBarNotification
import com.pixels.agent.core.StorageFailure
import java.util.UUID
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject

/** Bounded transient references are scoped to the authenticated caller and connection. */
class AndroidNotifications(private val context: Context, private val allowed: () -> Boolean) {
    private data class Reference(
        val caller: String,
        val scope: String,
        val key: String,
        val packageName: String,
        val revision: Long,
        val created: Long,
        val fingerprint: String,
        val intent: PendingIntent?,
    )

    private val references = linkedMapOf<String, Reference>()

    @Suppress("DEPRECATION") // Sender opt-in uses the API 34/35 value only on those versions.
    suspend fun execute(input: JSONObject): JSONObject {
        val query = input.getJSONObject("query").getJSONObject("query")
        val operation = query.getString("operation")
        ensureAllowed()
        val granted = hasAccess(context)
        var listener = PixelsNotificationListener.current
        if (operation == "status")
            return result(granted, listener != null, 0, JSONArray(), "sampled")
        if (!granted) throw StorageFailure("notification_access_required")
        if (listener == null) throw StorageFailure("notification_listener_unavailable")
        val packageName = query.optString("package_id")
        var outcome = "sampled"
        if (operation == "wait") {
            val until = SystemClock.elapsedRealtime() + query.getLong("timeout_ms")
            while (listener!!.revision(packageName) == query.getLong("after_revision")) {
                ensureAllowed()
                if (!hasAccess(context)) throw StorageFailure("notification_access_required")
                if (SystemClock.elapsedRealtime() >= until) {
                    outcome = "timed_out"
                    break
                }
                delay(150)
                listener =
                    PixelsNotificationListener.current
                        ?: throw StorageFailure("notification_listener_unavailable")
            }
            if (outcome != "timed_out") outcome = "changed"
        }
        var dispatched = false
        return try {
            withContext(Dispatchers.Main.immediate) {
                ensureAllowed()
                val active =
                    PixelsNotificationListener.current
                        ?: throw StorageFailure("notification_listener_unavailable")
                if (operation in listOf("list", "wait")) {
                    val revision = active.revision(packageName)
                    val rows = JSONArray()
                    val notices =
                        active.activeNotifications
                            .filter { it.packageName == packageName }
                            .sortedByDescending { it.postTime }
                    var truncated = false
                    for (notice in notices) {
                        if (
                            rows.length() >= query.getInt("limit") ||
                                rows.toString().toByteArray().size > 16000
                        ) {
                            truncated = true
                            break
                        }
                        val id = UUID.randomUUID().toString()
                        if (references.size >= 512) references.remove(references.keys.first())
                        references[id] =
                            Reference(
                                input.getString("caller"),
                                input.getString("connection_scope"),
                                notice.key,
                                packageName,
                                revision,
                                SystemClock.elapsedRealtime(),
                                fingerprint(notice),
                                notice.notification.contentIntent,
                            )
                        rows.put(entry(id, notice))
                    }
                    result(true, true, revision, rows, outcome, truncated = truncated)
                } else {
                    val reference =
                        references[query.getString("notification_ref")]
                            ?: throw StorageFailure("notification_reference_expired")
                    if (
                        reference.caller != input.getString("caller") ||
                            reference.scope != input.getString("connection_scope") ||
                            SystemClock.elapsedRealtime() - reference.created > 600000 ||
                            active.revision(reference.packageName) != reference.revision
                    )
                        throw StorageFailure("notification_reference_expired")
                    val notice =
                        active.activeNotifications.singleOrNull { it.key == reference.key }
                            ?: throw StorageFailure("notification_reference_expired")
                    if (
                        fingerprint(notice) != reference.fingerprint ||
                            notice.notification.contentIntent != reference.intent
                    )
                        throw StorageFailure("notification_reference_expired")
                    ensureAllowed()
                    if (operation == "open") {
                        val intent =
                            notice.notification.contentIntent
                                ?: throw StorageFailure("notification_open_unavailable")
                        try {
                            val options =
                                if (android.os.Build.VERSION.SDK_INT >= 34)
                                    android.app.ActivityOptions.makeBasic()
                                        .apply {
                                            setPendingIntentBackgroundActivityStartMode(
                                                if (android.os.Build.VERSION.SDK_INT >= 36)
                                                    android.app.ActivityOptions
                                                        .MODE_BACKGROUND_ACTIVITY_START_ALLOW_ALWAYS
                                                else
                                                    android.app.ActivityOptions
                                                        .MODE_BACKGROUND_ACTIVITY_START_ALLOWED
                                            )
                                        }
                                        .toBundle()
                                else null
                            dispatched = true
                            intent.send(context, 0, null, null, null, null, options)
                        } catch (_: PendingIntent.CanceledException) {
                            dispatched = false
                            throw StorageFailure("notification_reference_expired")
                        }
                        result(true, true, reference.revision, JSONArray(), "dispatched", true)
                    } else if (operation == "dismiss") {
                        if (
                            notice.isOngoing ||
                                !notice.isClearable ||
                                notice.notification.flags and
                                    Notification.FLAG_FOREGROUND_SERVICE != 0
                        )
                            throw StorageFailure("notification_not_clearable")
                        dispatched = true
                        active.cancelNotification(notice.key)
                        val deadline = SystemClock.elapsedRealtime() + 2000
                        while (
                            active.activeNotifications.any { it.key == notice.key } &&
                                SystemClock.elapsedRealtime() < deadline
                        ) delay(100)
                        val removed = active.activeNotifications.none { it.key == notice.key }
                        result(
                            true,
                            true,
                            active.revision(reference.packageName),
                            JSONArray(),
                            if (removed) "removed" else "unconfirmed",
                            true,
                        )
                    } else throw StorageFailure("notification_operation_unavailable")
                }
            }
        } catch (cancelled: kotlinx.coroutines.CancellationException) {
            throw cancelled
        } catch (failure: Exception) {
            if (dispatched)
                JSONObject()
                    .put("error", "notification_action_outcome_unconfirmed")
                    .put("action_started", true)
            else throw failure
        }
    }

    private fun entry(id: String, notice: StatusBarNotification): JSONObject {
        val extras = notice.notification.extras
        return JSONObject()
            .put("notification_ref", id)
            .put("package_id", notice.packageName)
            .put(
                "title",
                extras.getCharSequence(Notification.EXTRA_TITLE)?.toString()?.take(240)
                    ?: JSONObject.NULL,
            )
            .put(
                "text",
                (extras.getCharSequence(Notification.EXTRA_BIG_TEXT)
                        ?: extras.getCharSequence(Notification.EXTRA_TEXT))
                    ?.toString()
                    ?.take(1000) ?: JSONObject.NULL,
            )
            .put("posted_at_unix_ms", notice.postTime)
            .put("ongoing", notice.isOngoing)
            .put(
                "clearable",
                notice.isClearable &&
                    !notice.isOngoing &&
                    notice.notification.flags and Notification.FLAG_FOREGROUND_SERVICE == 0,
            )
            .put("can_open", notice.notification.contentIntent != null)
    }

    private fun fingerprint(notice: StatusBarNotification): String {
        val notification = notice.notification
        val digest = java.security.MessageDigest.getInstance("SHA-256")
        for (value in
            listOf(
                notice.key,
                notice.postTime.toString(),
                notification.`when`.toString(),
                notification.flags.toString(),
                notification.extras.getCharSequence(Notification.EXTRA_TITLE)?.toString(),
                notification.extras.getCharSequence(Notification.EXTRA_TEXT)?.toString(),
                notification.extras.getCharSequence(Notification.EXTRA_BIG_TEXT)?.toString(),
            )) {
            val bytes = value?.toByteArray() ?: byteArrayOf()
            digest.update(java.nio.ByteBuffer.allocate(4).putInt(bytes.size).array())
            digest.update(bytes)
        }
        return digest.digest().joinToString("") { "%02x".format(it) }
    }

    private fun result(
        granted: Boolean,
        connected: Boolean,
        revision: Long,
        entries: JSONArray,
        outcome: String,
        dispatched: Boolean = false,
        truncated: Boolean = false,
    ): JSONObject =
        JSONObject()
            .put(
                "data",
                JSONObject()
                    .put("type", "notifications")
                    .put(
                        "snapshot",
                        JSONObject()
                            .put("access_granted", granted)
                            .put("listener_connected", connected)
                            .put("revision", revision)
                            .put("entries", entries)
                            .put("outcome", outcome)
                            .put("action_dispatched", dispatched)
                            .put("truncated", truncated),
                    ),
            )
            .put("count", entries.length())
            .put("truncated", truncated)
            .put("warnings", JSONArray())

    private fun ensureAllowed() {
        if (!allowed()) throw StorageFailure("control_paused_or_disconnected")
        if (
            context.getSystemService(KeyguardManager::class.java).isKeyguardLocked ||
                !context.getSystemService(PowerManager::class.java).isInteractive
        )
            throw StorageFailure("android_user_locked_or_display_not_interactive")
    }

    companion object {
        fun hasAccess(context: Context): Boolean =
            context
                .getSystemService(NotificationManager::class.java)
                .isNotificationListenerAccessGranted(
                    ComponentName(context, PixelsNotificationListener::class.java)
                )
    }
}
