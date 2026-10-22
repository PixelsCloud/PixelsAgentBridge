package com.pixels.agent.fixture

import android.content.Intent
import android.content.IntentFilter
import android.net.TrafficStats
import android.os.BatteryManager
import android.os.Debug
import android.os.Process
import android.os.SystemClock
import com.pixels.agent.PixelsApplication
import java.io.File
import java.security.MessageDigest
import java.util.UUID
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.json.JSONObject

/** Debug telemetry only: no wake lock, connection owner, screen content, or input execution. */
internal object DebugSoakSampler {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var request: String? = null
    private var job: Job? = null

    @Synchronized
    fun start(app: PixelsApplication, value: String) {
        val id = UUID.fromString(value).toString()
        if (request == id && job?.isActive == true) return
        check(job?.isActive != true) { "sampler_already_running" }
        check(app.connection.status.value.remoteReady) { "connection_not_ready" }
        check(!File(app.noBackupFilesDir, "production-probe-backup").exists()) {
            "normal_connection_required"
        }
        val output = File(app.filesDir, "soak-$id.jsonl")
        check(output.createNewFile()) { "sampler_evidence_already_exists" }
        request = id
        job = scope.launch {
            try {
                val digest = MessageDigest.getInstance("SHA-256")
                File(app.applicationInfo.sourceDir).inputStream().use { input ->
                    val buffer = ByteArray(65536)
                    while (true) {
                        val count = input.read(buffer)
                        if (count < 0) break
                        digest.update(buffer, 0, count)
                    }
                }
                append(
                    output,
                    JSONObject()
                        .put("type", "start")
                        .put("request_id", id)
                        .put(
                            "apk_sha256",
                            digest.digest().joinToString("") { "%02x".format(it.toInt() and 255) },
                        )
                        .put("pid", Process.myPid())
                        .put("elapsed_ms", SystemClock.elapsedRealtime())
                        .put("wall_time_ms", System.currentTimeMillis()),
                )
                repeat(1500) { index ->
                    append(output, sample(app).put("type", "sample").put("index", index))
                    delay(60000)
                }
                append(output, JSONObject().put("type", "capacity_reached"))
            } catch (error: kotlinx.coroutines.CancellationException) {
                runCatching {
                    append(
                        output,
                        JSONObject()
                            .put("type", "stopped")
                            .put("elapsed_ms", SystemClock.elapsedRealtime()),
                    )
                }
                throw error
            } catch (_: Exception) {
                runCatching { append(output, JSONObject().put("type", "sampling_failed")) }
            }
        }
    }

    @Synchronized
    fun stop(value: String) {
        check(request == UUID.fromString(value).toString()) { "sampler_request_mismatch" }
        job?.cancel()
    }

    private fun sample(app: PixelsApplication): JSONObject {
        val battery = app.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
        val status = app.connection.status.value
        val memory = Debug.MemoryInfo().also(Debug::getMemoryInfo)
        val threads =
            File("/proc/self/status").useLines { lines ->
                lines
                    .firstOrNull { it.startsWith("Threads:") }
                    ?.substringAfter(':')
                    ?.trim()
                    ?.toIntOrNull()
            }
        return JSONObject()
            .put("elapsed_ms", SystemClock.elapsedRealtime())
            .put("wall_time_ms", System.currentTimeMillis())
            .put("pid", Process.myPid())
            .put("phase", status.phase.name)
            .put("remote_ready", status.remoteReady)
            .put("generation", status.generation)
            .put("paused", app.dispatcher.paused.value)
            .put("pss_kib", memory.totalPss)
            .put("native_heap_bytes", Debug.getNativeHeapAllocatedSize())
            .put("fd_count", File("/proc/self/fd").list()?.size ?: JSONObject.NULL)
            .put("thread_count", threads ?: JSONObject.NULL)
            .put("battery_level", battery?.getIntExtra(BatteryManager.EXTRA_LEVEL, -1))
            .put("battery_scale", battery?.getIntExtra(BatteryManager.EXTRA_SCALE, -1))
            .put("plugged", battery?.getIntExtra(BatteryManager.EXTRA_PLUGGED, -1))
            .put("temperature_tenths_c", battery?.getIntExtra(BatteryManager.EXTRA_TEMPERATURE, -1))
            .put("uid_rx_bytes", TrafficStats.getUidRxBytes(Process.myUid()))
            .put("uid_tx_bytes", TrafficStats.getUidTxBytes(Process.myUid()))
    }

    private fun append(output: File, value: JSONObject) {
        output.appendText(value.toString() + "\n")
    }
}
