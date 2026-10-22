package com.pixels.agent

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log
import com.pixels.agent.core.shouldRestoreConnection
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeout

/**
 * BOOT_COMPLETED arrives after credential storage is unlocked; never move credentials to DE
 * storage.
 */
class StartupReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (
            intent.action !in setOf(Intent.ACTION_BOOT_COMPLETED, Intent.ACTION_MY_PACKAGE_REPLACED)
        )
            return
        val pending = goAsync()
        CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate).launch {
            try {
                withTimeout(8000) {
                    val app = context.applicationContext as PixelsApplication
                    app.productPreferences.load()
                    app.controlIntent.load()
                    if (
                        !app.productPreferences.failed.value &&
                            shouldRestoreConnection(
                                app.productPreferences.state.value.startOnBoot,
                                app.controlIntent.state.value,
                            )
                    ) {
                        try {
                            ConnectionService.restore(context)
                        } catch (error: IllegalStateException) {
                            Log.w(
                                "PixelsStartup",
                                "System blocked background connection recovery",
                                error,
                            )
                            notifyRecovery(context)
                        } catch (error: SecurityException) {
                            Log.w(
                                "PixelsStartup",
                                "System denied background connection recovery",
                                error,
                            )
                            notifyRecovery(context)
                        }
                    }
                }
            } catch (error: Exception) {
                // Leave unknown configuration and explicit Stop/Pause decisions untouched.
                Log.w("PixelsStartup", "Connection recovery was not completed", error)
            } finally {
                pending.finish()
            }
        }
    }

    private fun notifyRecovery(context: Context) {
        val notifications = context.getSystemService(NotificationManager::class.java)
        if (!notifications.areNotificationsEnabled()) return
        val channel = "pixels-startup-recovery"
        notifications.createNotificationChannel(
            NotificationChannel(
                channel,
                context.getString(R.string.startup_title),
                NotificationManager.IMPORTANCE_DEFAULT,
            )
        )
        val open =
            PendingIntent.getActivity(
                context,
                3,
                Intent(context, MainActivity::class.java),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )
        notifications.notify(
            3,
            Notification.Builder(context, channel)
                .setSmallIcon(com.pixels.agent.platform.R.drawable.lucide_link)
                .setContentTitle(context.getString(R.string.startup_title))
                .setContentText(context.getString(R.string.startup_blocked))
                .setContentIntent(open)
                .setAutoCancel(true)
                .build(),
        )
    }
}
