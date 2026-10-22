package com.pixels.agent.documents

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.os.Bundle
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.Message
import android.os.Messenger
import android.os.ParcelFileDescriptor
import com.pixels.agent.core.StorageFailure
import java.io.File
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.withTimeoutOrNull
import org.json.JSONObject

/** Only bounded options and two descriptors cross into the isolated process. */
internal class DocumentWorkerClient(private val context: Context) {
    private val owner = Mutex()

    suspend fun execute(input: File, output: File, options: JSONObject): JSONObject =
        owner.withLock {
            val result = CompletableDeferred<JSONObject>()
            val died = CompletableDeferred<Unit>()
            var remote: Messenger? = null
            var binder: IBinder? = null
            var bound = false
            val dispatched = AtomicBoolean()
            val death = IBinder.DeathRecipient {
                died.complete(Unit)
                result.completeExceptionally(StorageFailure("document_worker_died_or_limit"))
            }
            val receiver =
                Messenger(
                    Handler(Looper.getMainLooper()) { message ->
                        try {
                            result.complete(JSONObject(message.data.getString("result") ?: "{}"))
                        } catch (_: Exception) {
                            result.completeExceptionally(
                                StorageFailure("document_worker_result_invalid")
                            )
                        }
                        true
                    }
                )
            val connection =
                object : ServiceConnection {
                    override fun onServiceConnected(name: ComponentName, service: IBinder) {
                        // Android can reconnect a BIND_AUTO_CREATE service after its
                        // process dies. Never resend an already dispatched parser job.
                        if (result.isCompleted || !dispatched.compareAndSet(false, true)) {
                            try {
                                Messenger(service).send(Message.obtain().apply { what = 2 })
                            } catch (_: Exception) {}
                            return
                        }
                        binder = service
                        try {
                            service.linkToDeath(death, 0)
                            val messenger = Messenger(service)
                            remote = messenger
                            ParcelFileDescriptor.open(input, ParcelFileDescriptor.MODE_READ_ONLY)
                                .use { source ->
                                    ParcelFileDescriptor.open(
                                            output,
                                            ParcelFileDescriptor.MODE_CREATE or
                                                ParcelFileDescriptor.MODE_TRUNCATE or
                                                ParcelFileDescriptor.MODE_WRITE_ONLY,
                                        )
                                        .use { destination ->
                                            messenger.send(
                                                Message.obtain().apply {
                                                    what = 1
                                                    replyTo = receiver
                                                    data =
                                                        Bundle().apply {
                                                            putParcelable("input", source)
                                                            putParcelable("output", destination)
                                                            putString("options", options.toString())
                                                        }
                                                }
                                            )
                                        }
                                }
                        } catch (_: Exception) {
                            result.completeExceptionally(
                                StorageFailure("document_worker_start_failed")
                            )
                        }
                    }

                    override fun onServiceDisconnected(name: ComponentName) {
                        death.binderDied()
                    }

                    override fun onNullBinding(name: ComponentName) {
                        result.completeExceptionally(StorageFailure("document_worker_unavailable"))
                    }

                    override fun onBindingDied(name: ComponentName) {
                        death.binderDied()
                    }
                }
            try {
                withContext(Dispatchers.Main.immediate) {
                    bound =
                        context.bindService(
                            Intent(context, DocumentWorkerService::class.java),
                            connection,
                            Context.BIND_AUTO_CREATE,
                        )
                }
                if (!bound) throw StorageFailure("document_worker_unavailable")
                withTimeout(40000) { result.await() }
            } finally {
                withContext(NonCancellable + Dispatchers.Main.immediate) {
                    try {
                        remote?.send(Message.obtain().apply { what = 2 })
                    } catch (_: Exception) {}
                    withTimeoutOrNull(2000) { if (remote != null) died.await() }
                    try {
                        binder?.unlinkToDeath(death, 0)
                    } catch (_: Exception) {}
                    if (bound) context.unbindService(connection)
                }
            }
        }
}
