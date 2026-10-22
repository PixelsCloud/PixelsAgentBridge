package com.pixels.agent.documents

import android.app.Service
import android.content.Intent
import android.os.Bundle
import android.os.Debug
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.Message
import android.os.Messenger
import android.os.ParcelFileDescriptor
import android.os.Process
import com.pixels.agent.transport.NativeDocumentBridge
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean
import org.json.JSONObject

/** One job per isolated process; no access to application storage, credentials or network. */
class DocumentWorkerService : Service() {
    private val used = AtomicBoolean()
    private val main = Handler(Looper.getMainLooper())
    private val executor = Executors.newSingleThreadExecutor()
    private val messenger =
        Messenger(
            Handler(Looper.getMainLooper()) { message ->
                if (message.what == 2) {
                    Process.killProcess(Process.myPid())
                    return@Handler true
                }
                if (message.what != 1 || !used.compareAndSet(false, true)) return@Handler true
                val recipient = message.replyTo ?: return@Handler true
                val data = message.data
                @Suppress("DEPRECATION")
                val input = data.getParcelable<ParcelFileDescriptor>("input") ?: return@Handler true
                @Suppress("DEPRECATION")
                val output =
                    data.getParcelable<ParcelFileDescriptor>("output") ?: return@Handler true
                val options = JSONObject(data.getString("options") ?: "{}")
                android.util.Log.i(
                    "PixelsDocumentWorker",
                    "job_started pid=${Process.myPid()} uid=${Process.myUid()} action=${options.optString("action")}",
                )
                main.postDelayed({ Process.killProcess(Process.myPid()) }, 30000)
                val memoryWatchdog =
                    object : Runnable {
                        override fun run() {
                            if (Debug.getPss() > 384 * 1024) Process.killProcess(Process.myPid())
                            main.postDelayed(this, 250)
                        }
                    }
                main.post(memoryWatchdog)
                executor.execute {
                    val result =
                        try {
                            input.use { source ->
                                output.use { destination ->
                                    val native = NativeDocumentBridge()
                                    val limits = JSONObject(native.limitResources())
                                    check(limits.optBoolean("ok")) { "document_limits_unavailable" }
                                    if (options.getString("action") == "extract") {
                                        val envelope =
                                            JSONObject(
                                                native.extract(
                                                    source.fd,
                                                    options.getString("extension"),
                                                    options.getString("ocr_mode"),
                                                    destination.fd,
                                                    OfflinePageRecognizer(
                                                        assets,
                                                        source,
                                                        options.getString("extension"),
                                                        native,
                                                    ),
                                                )
                                            )
                                        if (envelope.optBoolean("ok"))
                                            JSONObject().put("ready", true)
                                        else
                                            JSONObject()
                                                .put(
                                                    "error",
                                                    envelope.optString(
                                                        "error",
                                                        "document_parse_failed",
                                                    ),
                                                )
                                    } else {
                                        render(source, destination, options)
                                        JSONObject().put("ready", true)
                                    }
                                }
                            }
                        } catch (_: SecurityException) {
                            JSONObject().put("error", "password_required_or_access_denied")
                        } catch (_: Exception) {
                            JSONObject().put("error", "document_worker_failed")
                        }
                    try {
                        recipient.send(
                            Message.obtain().apply {
                                this.data =
                                    Bundle().apply { putString("result", result.toString()) }
                            }
                        )
                    } catch (_: Exception) {
                        Process.killProcess(Process.myPid())
                    }
                }
                true
            }
        )

    override fun onBind(intent: Intent): IBinder = messenger.binder

    private fun render(
        input: ParcelFileDescriptor,
        output: ParcelFileDescriptor,
        options: JSONObject,
    ) {
        val dimension = options.getInt("max_dimension")
        require(dimension in 256..2048)
        val expected = options.getString("sha256")
        val hash = java.security.MessageDigest.getInstance("SHA-256")
        ParcelFileDescriptor.AutoCloseInputStream(ParcelFileDescriptor.dup(input.fileDescriptor))
            .use { stream ->
                val bytes = ByteArray(65536)
                while (true) {
                    val count = stream.read(bytes)
                    if (count < 0) break
                    hash.update(bytes, 0, count)
                }
                stream.channel.position(0)
            }
        check(hash.digest().joinToString("") { "%02x".format(it.toInt() and 255) } == expected)
        val bitmap =
            DocumentRasterizer.bitmap(
                input,
                options.getString("extension"),
                options.getInt("page_number"),
                dimension,
            )
        try {
            ParcelFileDescriptor.AutoCloseOutputStream(
                    ParcelFileDescriptor.dup(output.fileDescriptor)
                )
                .use { stream ->
                    check(bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, stream))
                    stream.fd.sync()
                }
        } finally {
            bitmap.recycle()
        }
    }
}
