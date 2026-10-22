package com.pixels.agent

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import com.pixels.agent.core.shouldRestoreConnection
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch

/** A user-requested connection can survive reboot; its notification always offers Stop. */
class ConnectionService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var session: Job? = null
    private var stopping = false

    override fun onCreate() {
        super.onCreate()
        getSystemService(NotificationManager::class.java)
            .createNotificationChannel(
                NotificationChannel(
                    CHANNEL,
                    getString(R.string.connection_channel),
                    NotificationManager.IMPORTANCE_LOW,
                )
            )
        val open =
            PendingIntent.getActivity(
                this,
                0,
                Intent(this, MainActivity::class.java),
                PendingIntent.FLAG_IMMUTABLE,
            )
        val stop =
            PendingIntent.getService(
                this,
                1,
                Intent(this, ConnectionService::class.java).setAction(ACTION_STOP),
                PendingIntent.FLAG_IMMUTABLE,
            )
        val notification =
            Notification.Builder(this, CHANNEL)
                .setSmallIcon(com.pixels.agent.platform.R.drawable.lucide_link)
                .setContentTitle(getString(R.string.connection_notification))
                .setContentText(getString(R.string.connection_preview))
                .setContentIntent(open)
                .addAction(
                    Notification.Action.Builder(null, getString(R.string.disconnect), stop).build()
                )
                .setOngoing(true)
                .build()
        if (Build.VERSION.SDK_INT >= 34) {
            startForeground(2, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        } else startForeground(2, notification)
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            (application as PixelsApplication).remoteControlAllowed.set(false)
            if (!stopping) {
                stopping = true
                scope.launch {
                    try {
                        (application as PixelsApplication)
                            .controlIntent
                            .setConnectionRequested(false)
                    } catch (cancelled: CancellationException) {
                        throw cancelled
                    } catch (_: Exception) {
                        (application as PixelsApplication).dispatcher.setPaused(true)
                    } finally {
                        session?.cancel() ?: stopSelf()
                    }
                }
            }
            return START_NOT_STICKY
        }
        if (session == null && !stopping) {
            session = scope.launch {
                try {
                    val app = application as PixelsApplication
                    app.controlIntent.load()
                    if (intent?.action == ACTION_RESTORE) {
                        app.productPreferences.load()
                        if (
                            app.productPreferences.failed.value ||
                                !shouldRestoreConnection(
                                    app.productPreferences.state.value.startOnBoot,
                                    app.controlIntent.state.value,
                                )
                        )
                            return@launch
                    } else if (intent == null) {
                        if (!app.controlIntent.state.value.connectionRequested) return@launch
                    } else app.controlIntent.setConnectionRequested(true)
                    app.remoteControlAllowed.set(true)
                    app.connection.runConnection()
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (_: Exception) {
                    (application as PixelsApplication).dispatcher.setPaused(true)
                } finally {
                    (application as PixelsApplication).remoteControlAllowed.set(false)
                    stopSelf()
                }
            }
        }
        return START_STICKY
    }

    override fun onDestroy() {
        (application as PixelsApplication).remoteControlAllowed.set(false)
        scope.cancel()
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    companion object {
        private const val CHANNEL = "pixels-control-connection"
        private const val ACTION_STOP = "com.pixels.agent.STOP_CONNECTION"
        private const val ACTION_RESTORE = "com.pixels.agent.RESTORE_CONNECTION"

        fun start(context: Context) {
            context.startForegroundService(Intent(context, ConnectionService::class.java))
        }

        internal fun restore(context: Context) {
            context.startForegroundService(
                Intent(context, ConnectionService::class.java).setAction(ACTION_RESTORE)
            )
        }

        fun stop(context: Context) {
            context.startService(
                Intent(context, ConnectionService::class.java).setAction(ACTION_STOP)
            )
        }
    }
}
