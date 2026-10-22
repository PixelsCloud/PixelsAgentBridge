package com.pixels.agent.fixture

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.widget.Button
import android.widget.CheckBox
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.TextView
import java.util.UUID

/** Debug-only ordinary UI and real NotificationManager producer; no execution shortcuts. */
@SuppressLint("SetTextI18n")
class UnifiedToolsFixtureActivity : Activity() {
    private val handler = Handler(Looper.getMainLooper())
    private val notices by lazy { getSystemService(NotificationManager::class.java) }
    private var run = ""
    private var count = 0
    private lateinit var status: TextView
    private lateinit var input: EditText

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        run =
            intent.getStringExtra("qa_run")?.let { UUID.fromString(it).toString() }
                ?: savedInstanceState?.getString("qa_run")
                ?: UUID.randomUUID().toString()
        notices.createNotificationChannel(
            NotificationChannel(
                CHANNEL,
                "Pixels notification acceptance",
                NotificationManager.IMPORTANCE_DEFAULT,
            )
        )
        input =
            EditText(this).apply {
                id = android.R.id.edit
                contentDescription = "unified_input"
                setSingleLine(true)
            }
        status =
            TextView(this).apply {
                text = "unified_idle"
            }
        val layout =
            LinearLayout(this).apply {
                orientation = LinearLayout.VERTICAL
                addView(
                    TextView(this@UnifiedToolsFixtureActivity).apply {
                        text = "unified_fixture:$run"
                    }
                )
                addView(input)
                addView(
                    Button(this@UnifiedToolsFixtureActivity).apply {
                        text = "unified_save"
                        contentDescription = "unified_save"
                        setOnClickListener {
                            count++
                            status.text = "unified_saved:$count:${input.text}"
                        }
                    }
                )
                addView(
                    CheckBox(this@UnifiedToolsFixtureActivity).apply {
                        text = "unified_check"
                        contentDescription = "unified_check"
                    }
                )
                for ((name, action) in
                    listOf<Pair<String, () -> Unit>>(
                        "unified_select_range" to
                            {
                                input.requestFocus()
                                if (input.length() >= 4) input.setSelection(2, 4)
                                status.text = "selection_ready"
                            },
                        "notification_post" to { post(1, "v1", false) },
                        "notification_update" to { post(1, "v2", false) },
                        "notification_delayed" to
                            {
                                handler.postDelayed({ post(2, "delayed", false) }, 2500)
                            },
                        "notification_ongoing" to { post(3, "ongoing", true) },
                        "notification_cleanup" to
                            {
                                for (id in 1..3) notices.cancel(run, id)
                                status.text = "notifications_cleaned"
                            },
                    )) addView(
                    Button(this@UnifiedToolsFixtureActivity).apply {
                        text = name
                        contentDescription = name
                        setOnClickListener { action() }
                    }
                )
                addView(status)
            }
        setContentView(layout)
        if (intent.getBooleanExtra("from_notification", false))
            status.text = "notification_opened:$run"
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        if (
            intent.getStringExtra("qa_run") == run &&
                intent.getBooleanExtra("from_notification", false)
        )
            status.text = "notification_opened:$run"
    }

    override fun onSaveInstanceState(outState: Bundle) {
        outState.putString("qa_run", run)
        super.onSaveInstanceState(outState)
    }

    override fun onDestroy() {
        handler.removeCallbacksAndMessages(null)
        super.onDestroy()
    }

    private fun post(id: Int, text: String, ongoing: Boolean) {
        if (
            Build.VERSION.SDK_INT >= 33 &&
                checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) !=
                    PackageManager.PERMISSION_GRANTED
        ) {
            status.text = "notification_permission_required"
            return
        }
        val target =
            Intent(this, UnifiedToolsFixtureActivity::class.java)
                .putExtra("qa_run", run)
                .putExtra("from_notification", true)
                .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val pending =
            PendingIntent.getActivity(
                this,
                run.hashCode() + id,
                target,
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
        notices.notify(
            run,
            id,
            Notification.Builder(this, CHANNEL)
                .setSmallIcon(android.R.drawable.ic_dialog_info)
                .setContentTitle("Pixels QA $run")
                .setContentText(text)
                .setContentIntent(pending)
                .setOngoing(ongoing)
                .build(),
        )
        status.text = "notification_posted:$id:$text"
    }

    companion object {
        private const val CHANNEL = "pixels-notification-acceptance"
    }
}
