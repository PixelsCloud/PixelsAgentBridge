package com.pixels.agent.transport

import android.util.AtomicFile
import com.pixels.agent.core.ControlIntentRepository
import com.pixels.agent.core.ControlIntentState
import java.io.File
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.json.JSONObject

/** Publish resume only after the TOML write commits; an unreadable intent always pauses input. */
class AndroidControlIntentRepository(
    directory: File,
    private val native: NativeBridge,
    private val applyPaused: (Boolean) -> Unit,
) : ControlIntentRepository {
    private val file = AtomicFile(File(directory, "control-intent.toml"))
    private val mutex = Mutex()
    private val pendingPauses = AtomicInteger()
    private val current = MutableStateFlow(ControlIntentState())
    override val state = current.asStateFlow()

    // Connected devices allow control. Migrate the retired user pause choice on load.
    override suspend fun load() = update(false, null)

    override suspend fun setPaused(paused: Boolean) {
        // Close the dispatch gate before waiting for disk or an earlier update.
        if (paused) {
            pendingPauses.incrementAndGet()
            applyPaused(true)
        }
        try {
            update(paused, null)
        } finally {
            if (paused) pendingPauses.decrementAndGet()
        }
    }

    override suspend fun setConnectionRequested(requested: Boolean) = update(null, requested)

    private suspend fun update(paused: Boolean?, requested: Boolean?) {
        withContext(Dispatchers.IO) {
            mutex.withLock {
                try {
                    val previous = read()
                    val next =
                        previous.copy(
                            paused = paused ?: previous.paused,
                            connectionRequested = requested ?: previous.connectionRequested,
                        )
                    if (next != previous) write(next)
                    current.value = next
                    applyPaused(next.paused || pendingPauses.get() > 0)
                } catch (cancelled: CancellationException) {
                    applyPaused(true)
                    throw cancelled
                } catch (error: Exception) {
                    applyPaused(true)
                    current.value = ControlIntentState(error = "control_intent_unavailable")
                    throw error
                }
            }
        }
    }

    private fun read(): ControlIntentState {
        val source =
            try {
                file.openRead().use { it.readBounded(4096) }.toString(Charsets.UTF_8)
            } catch (_: java.io.FileNotFoundException) {
                return ControlIntentState(loaded = true, paused = false)
            }
        val intent = nativeValue(native.decodeUserIntent(source)).getJSONObject("intent")
        return ControlIntentState(
            loaded = true,
            paused = intent.getBoolean("paused"),
            connectionRequested = intent.getBoolean("connection_requested"),
        )
    }

    private fun write(intent: ControlIntentState) {
        val input =
            JSONObject()
                .put("schema_version", 1)
                .put("paused", intent.paused)
                .put("connection_requested", intent.connectionRequested)
        val source = nativeValue(native.encodeUserIntent(input.toString())).getString("toml")
        file.baseFile.parentFile?.mkdirs()
        val stream = file.startWrite()
        try {
            stream.write(source.toByteArray(Charsets.UTF_8))
            file.finishWrite(stream)
        } catch (error: Exception) {
            file.failWrite(stream)
            throw error
        }
    }
}
