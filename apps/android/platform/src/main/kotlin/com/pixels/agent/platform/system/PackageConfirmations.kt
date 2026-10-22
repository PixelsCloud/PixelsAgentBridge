package com.pixels.agent.platform.system

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import com.pixels.agent.platform.R
import java.util.concurrent.ConcurrentHashMap
import org.json.JSONArray
import org.json.JSONObject

/** Lookup fields are durable; only Android retains the original Binder-bearing intent. */
internal object PackageConfirmations {
    private const val CHANNEL = "package-confirmations"
    private val retained = ConcurrentHashMap<String, PendingIntent>()

    fun encode(intent: Intent): String =
        obj(
                "version" to 1,
                "action" to intent.action,
                "data" to intent.dataString,
                "type" to intent.type,
                "package" to intent.`package`,
                "component" to intent.component?.flattenToString(),
                "categories" to JSONArray(intent.categories.orEmpty().toList()),
                "identifier" to if (Build.VERSION.SDK_INT >= 29) intent.identifier else null,
            )
            .toString()

    @Suppress("DEPRECATION")
    fun decode(value: String): Intent {
        if (!value.startsWith("{")) return Intent.parseUri(value, Intent.URI_INTENT_SCHEME)
        val key = JSONObject(value)
        check(key.getInt("version") == 1)
        fun field(name: String) = if (key.isNull(name)) null else key.getString(name)
        return Intent().apply {
            action = field("action")
            setDataAndType(field("data")?.let(Uri::parse), field("type"))
            setPackage(field("package"))
            component = field("component")?.let(ComponentName::unflattenFromString)
            val categories = key.getJSONArray("categories")
            for (index in 0 until categories.length()) addCategory(categories.getString(index))
            if (Build.VERSION.SDK_INT >= 29) identifier = field("identifier")
        }
    }

    fun retain(context: Context, id: String, pending: PendingIntent): Boolean {
        retained[id] = pending
        val manager = context.getSystemService(NotificationManager::class.java)
        if (
            !manager.areNotificationsEnabled() ||
                (Build.VERSION.SDK_INT >= 33 &&
                    context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) !=
                        PackageManager.PERMISSION_GRANTED)
        )
            return false
        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL,
                context.getString(R.string.package_confirmation_channel),
                NotificationManager.IMPORTANCE_LOW,
            )
        )
        if (
            manager.getNotificationChannel(CHANNEL).importance ==
                NotificationManager.IMPORTANCE_NONE
        )
            return false
        // NotificationManager holds this PendingIntent outside our process. An
        // unregistered PendingIntent alone may disappear when its last holder dies.
        manager.notify(
            id,
            1,
            Notification.Builder(context, CHANNEL)
                .setSmallIcon(R.drawable.lucide_check)
                .setContentTitle(context.getString(R.string.package_confirmation_title))
                .setContentText(context.getString(R.string.package_confirmation_text))
                .setContentIntent(pending)
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .build(),
        )
        return true
    }

    fun release(context: Context, id: String) {
        context.getSystemService(NotificationManager::class.java).cancel(id, 1)
        retained.remove(id)?.cancel()
    }
}
