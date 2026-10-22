package com.pixels.agent

import android.content.Context
import android.os.Process
import com.pixels.agent.core.RemoteCommandHandler
import com.pixels.agent.core.RemotePlatformResult
import java.io.File
import org.json.JSONObject

/** Native subprocesses use the app UID; this gate never runs a command from the SDK mailbox. */
internal class RemoteExecutionAdapter(
    context: Context,
    private val next: RemoteCommandHandler,
    private val allowed: () -> Boolean,
    private val paused: () -> Boolean,
) : RemoteCommandHandler {
    private val workspace = File(context.noBackupFilesDir, "workspace")

    override suspend fun execute(command: String): RemotePlatformResult {
        val input = JSONObject(command)
        if (input.optString("domain") != "execution") return next.execute(command)
        val result = JSONObject()
        when {
            !allowed() -> result.put("error", "control_disconnected")
            paused() -> result.put("error", "control_paused")
            input.optString("action") != "authorize" ->
                result.put("error", "invalid_execution_action")
            else -> result.put("workspace", workspace.absolutePath).put("uid", Process.myUid())
        }
        return RemotePlatformResult(result.toString())
    }
}
