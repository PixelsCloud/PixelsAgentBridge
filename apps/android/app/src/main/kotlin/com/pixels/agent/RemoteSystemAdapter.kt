package com.pixels.agent

import com.pixels.agent.core.RemoteCommandHandler
import com.pixels.agent.core.RemotePlatformResult
import com.pixels.agent.core.StorageFailure
import com.pixels.agent.platform.system.AndroidSystemQueries
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject

internal class RemoteSystemAdapter(
    private val queries: AndroidSystemQueries,
    private val next: RemoteCommandHandler,
    private val allowed: () -> Boolean,
    private val paused: () -> Boolean,
) : RemoteCommandHandler {
    override suspend fun execute(command: String): RemotePlatformResult {
        val input = JSONObject(command)
        if (input.optString("domain") != "system") return next.execute(command)
        return withContext(Dispatchers.IO) {
            val result =
                try {
                    if (!allowed()) throw StorageFailure("control_disconnected")
                    if (paused()) throw StorageFailure("control_paused")
                    queries.execute(input)
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (failure: Exception) {
                    JSONObject()
                        .put(
                            "action_started",
                            failure is StorageFailure &&
                                failure.code in
                                    listOf("unconfirmed", "android_ui_dispatch_unconfirmed"),
                        )
                        .put(
                            "error",
                            when (failure) {
                                is StorageFailure -> failure.code
                                is SecurityException -> "access_denied"
                                else -> "android_system_query_failed"
                            },
                        )
                }
            RemotePlatformResult(result.toString())
        }
    }
}
