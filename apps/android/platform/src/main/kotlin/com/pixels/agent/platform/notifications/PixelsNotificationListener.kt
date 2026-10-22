package com.pixels.agent.platform.notifications

import android.service.notification.NotificationListenerService
import android.service.notification.StatusBarNotification
import java.util.concurrent.atomic.AtomicLong

/** Android owns the listener binding. No notification text is written to disk. */
class PixelsNotificationListener : NotificationListenerService() {
    private val initialRevision = clock.incrementAndGet()
    private val revisions = linkedMapOf<String, Long>()

    override fun onListenerConnected() {
        current = this
    }

    override fun onListenerDisconnected() {
        if (current === this) current = null
    }

    override fun onDestroy() {
        if (current === this) current = null
        super.onDestroy()
    }

    override fun onNotificationPosted(sbn: StatusBarNotification) {
        changed(sbn.packageName)
    }

    override fun onNotificationRemoved(sbn: StatusBarNotification) {
        changed(sbn.packageName)
    }

    @Synchronized
    private fun changed(packageName: String) {
        if (revisions.size >= 256 && !revisions.containsKey(packageName))
            revisions.remove(revisions.keys.first())
        revisions[packageName] = clock.incrementAndGet()
    }

    @Synchronized
    internal fun revision(packageName: String): Long = revisions[packageName] ?: initialRevision

    companion object {
        private val clock = AtomicLong(System.currentTimeMillis() * 1024)
        @Volatile
        internal var current: PixelsNotificationListener? = null
            private set
    }
}
