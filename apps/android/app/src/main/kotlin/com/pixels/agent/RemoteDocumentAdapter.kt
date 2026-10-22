package com.pixels.agent

import android.content.Context
import com.pixels.agent.core.RemoteCommandHandler
import com.pixels.agent.core.RemotePlatformResult
import com.pixels.agent.core.StorageFailure
import com.pixels.agent.documents.DocumentWorkerClient
import com.pixels.agent.platform.storage.AndroidFileStorage
import java.io.File
import java.nio.file.Files
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.withContext
import org.json.JSONObject

internal class RemoteDocumentAdapter(
    context: Context,
    private val storage: AndroidFileStorage,
    private val next: RemoteCommandHandler,
    private val allowed: () -> Boolean,
    private val paused: () -> Boolean,
) : RemoteCommandHandler {
    private val root = File(context.noBackupFilesDir, "runtime")
    private val worker = DocumentWorkerClient(context.applicationContext)

    override suspend fun execute(command: String): RemotePlatformResult {
        val input = JSONObject(command)
        if (input.optString("domain") != "document") return next.execute(command)
        val result =
            withContext(Dispatchers.IO) {
                try {
                    if (!allowed()) throw StorageFailure("control_disconnected")
                    if (paused()) throw StorageFailure("control_paused")
                    if (input.getString("action") == "authorize") {
                        if (input.has("path")) storage.checkDocumentAccess(input.getString("path"))
                        JSONObject()
                    } else {
                        val directory = File(input.getString("directory"))
                        val cache =
                            directory.parentFile
                                ?: throw StorageFailure("invalid_document_snapshot")
                        val scope =
                            cache.parentFile ?: throw StorageFailure("invalid_document_snapshot")
                        if (
                            cache.name != "mobile.document-cache" ||
                                scope.parentFile != root ||
                                !scope.name.matches(Regex("[0-9a-f]{64}")) ||
                                Files.isSymbolicLink(scope.toPath()) ||
                                Files.isSymbolicLink(cache.toPath()) ||
                                UUID.fromString(directory.name).toString() != directory.name ||
                                Files.isSymbolicLink(root.toPath()) ||
                                Files.isSymbolicLink(directory.toPath())
                        )
                            throw StorageFailure("invalid_document_snapshot")
                        val source = File(directory, "input")
                        if (
                            !source.isFile ||
                                source.length() > 32L * 1024 * 1024 ||
                                Files.isSymbolicLink(source.toPath())
                        )
                            throw StorageFailure("invalid_document_snapshot")
                        val action = input.getString("action")
                        if (action !in setOf("extract", "render"))
                            throw StorageFailure("unsupported_document_action")
                        val output =
                            File(
                                directory,
                                if (action == "extract") "result.json" else "preview.png",
                            )
                        if (Files.isSymbolicLink(output.toPath()))
                            throw StorageFailure("invalid_document_snapshot")
                        val options = JSONObject().put("action", action)
                        for (key in
                            listOf(
                                "extension",
                                "ocr_mode",
                                "page_number",
                                "max_dimension",
                                "sha256",
                            )) {
                            if (input.has(key)) options.put(key, input.get(key))
                        }
                        worker.execute(source, output, options)
                    }
                } catch (_: TimeoutCancellationException) {
                    JSONObject().put("error", "document_worker_timeout")
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (failure: Exception) {
                    JSONObject()
                        .put(
                            "error",
                            if (failure is StorageFailure) failure.code
                            else "document_adapter_failed",
                        )
                }
            }
        return RemotePlatformResult(result.toString())
    }
}
