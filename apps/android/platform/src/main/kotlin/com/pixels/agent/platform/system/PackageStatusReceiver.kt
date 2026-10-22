package com.pixels.agent.platform.system

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** Only Android holding our explicit mutable PendingIntent can deliver a result. */
class PackageStatusReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        try {
            AndroidPackageChanges.receive(context, intent)
        } catch (_: Exception) {
            /* Preserve the prior durable fact; never infer success. */
        }
    }
}
