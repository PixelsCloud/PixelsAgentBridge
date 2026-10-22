package com.pixels.agent.platform.capture

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.content.res.Configuration
import android.graphics.PixelFormat
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.media.ImageReader
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.os.IBinder
import android.util.DisplayMetrics
import android.view.WindowManager
import com.pixels.agent.platform.PlatformOwner
import com.pixels.agent.platform.R

/** User-authorized capture session; never persists or silently reuses a consent token. */
class ProjectionCaptureService : Service() {
    private var projection: MediaProjection? = null
    private var reader: ImageReader? = null
    private var display: VirtualDisplay? = null
    private var worker: HandlerThread? = null
    private val frames
        get() = (application as PlatformOwner).projectionCapture

    @Suppress("DEPRECATION")
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (projection != null) return START_NOT_STICKY
        val request =
            intent
                ?: run {
                    stopSelf()
                    return START_NOT_STICKY
                }
        val data =
            if (Build.VERSION.SDK_INT >= 33) {
                request.getParcelableExtra(EXTRA_DATA, Intent::class.java)
            } else request.getParcelableExtra(EXTRA_DATA)
        if (data == null) {
            stopSelf()
            return START_NOT_STICKY
        }
        val notifications = getSystemService(NotificationManager::class.java)
        notifications.createNotificationChannel(
            NotificationChannel(
                CHANNEL,
                getString(R.string.capture_channel),
                NotificationManager.IMPORTANCE_LOW,
            )
        )
        val notification =
            Notification.Builder(this, CHANNEL)
                .setSmallIcon(R.drawable.lucide_monitor)
                .setContentTitle(getString(R.string.capture_active))
                .setOngoing(true)
                .build()
        if (Build.VERSION.SDK_INT >= 29) {
            startForeground(1, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION)
        } else startForeground(1, notification)

        val thread =
            HandlerThread("pixels-screen-capture").also {
                it.start()
                worker = it
            }
        val handler = Handler(thread.looper)
        val metrics = DisplayMetrics()
        getSystemService(WindowManager::class.java).defaultDisplay.getRealMetrics(metrics)
        val imageReader =
            ImageReader.newInstance(
                metrics.widthPixels,
                metrics.heightPixels,
                PixelFormat.RGBA_8888,
                3,
            )
        reader = imageReader
        frames.start()
        imageReader.setOnImageAvailableListener({ frames.accept(it) }, handler)
        val manager = getSystemService(MediaProjectionManager::class.java)
        try {
            val session =
                manager.getMediaProjection(request.getIntExtra(EXTRA_CODE, 0), data)
                    ?: run {
                        stopSelf()
                        return START_NOT_STICKY
                    }
            projection = session
            session.registerCallback(
                object : MediaProjection.Callback() {
                    override fun onStop() {
                        stopSelf()
                    }
                },
                Handler(mainLooper),
            )
            display =
                session.createVirtualDisplay(
                    "Pixels screen",
                    metrics.widthPixels,
                    metrics.heightPixels,
                    metrics.densityDpi,
                    DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR,
                    imageReader.surface,
                    null,
                    handler,
                )
        } catch (_: SecurityException) {
            stopSelf()
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        reader?.setOnImageAvailableListener(null, null)
        frames.stop()
        display?.release()
        projection?.stop()
        reader?.close()
        worker?.quitSafely()
        super.onDestroy()
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        // A changed geometry invalidates this preview's frame mapping. Request a new session.
        frames.stop()
        stopSelf()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    companion object {
        private const val CHANNEL = "pixels_screen_capture"
        private const val EXTRA_DATA = "capture_data"
        private const val EXTRA_CODE = "capture_code"

        fun start(context: Context, resultCode: Int, data: Intent) {
            val intent =
                Intent(context, ProjectionCaptureService::class.java)
                    .putExtra(EXTRA_CODE, resultCode)
                    .putExtra(EXTRA_DATA, data)
            context.startForegroundService(intent)
        }
    }
}
