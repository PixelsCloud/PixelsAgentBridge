package com.pixels.agent

import com.pixels.agent.core.RemoteCommandHandler
import com.pixels.agent.core.RemotePlatformResult
import com.pixels.agent.core.StorageFailure
import com.pixels.agent.platform.storage.AndroidFileStorage
import java.io.FileNotFoundException
import java.nio.file.FileAlreadyExistsException
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject

/** Routes file IO independently of accessibility; normal connection/pause gates still apply. */
internal class RemoteFileAdapter(
    private val storage: AndroidFileStorage,
    private val mobile: RemoteCommandHandler,
    private val allowed: () -> Boolean,
    private val paused: () -> Boolean,
    private val isGenerationActive: (Long) -> Boolean,
) : RemoteCommandHandler {
    override suspend fun execute(command: String): RemotePlatformResult {
        val input = JSONObject(command)
        if (input.optString("domain") !in setOf("storage", "storage_control"))
            return mobile.execute(command)
        if (!allowed()) return error("control_disconnected")
        if (paused()) return error("control_paused")
        if (input.optString("domain") == "storage_control") {
            return try {
                storage.checkMountAccess(input.getString("path"))
                RemotePlatformResult("{}")
            } catch (failure: StorageFailure) {
                error(failure.code)
            }
        }
        return withContext(Dispatchers.IO) {
            try {
                val checkpoint = {
                    if (!allowed() || !isGenerationActive(input.optLong("storage_epoch")))
                        throw StorageFailure("control_disconnected")
                    if (paused()) throw StorageFailure("control_paused")
                }
                checkpoint()
                RemotePlatformResult(storage.execute(input, checkpoint).toString())
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: Exception) {
                error(
                    when (failure) {
                        is StorageFailure -> failure.code
                        is SecurityException -> "access_denied"
                        is FileNotFoundException -> "not_found"
                        is FileAlreadyExistsException -> "already_exists"
                        else ->
                            if (input.optString("action") in listOf("publish", "mkdir", "delete", "managed"))
                                "publication_unconfirmed"
                            else "storage_io_error"
                    }
                )
            }
        }
    }

    private fun error(code: String) =
        RemotePlatformResult(JSONObject().put("error", code).toString())
}
