package com.pixels.agent

import android.util.AtomicFile
import java.io.File

/** A deliberately small TOML preference, separate from connection and credential configuration. */
internal class PermissionSetupPreferences(directory: File) {
    private val file = AtomicFile(File(directory, "permission-setup.toml"))

    fun workspaceOnly(): Boolean =
        try {
            file.openRead().use { input ->
                val buffer = ByteArray(1025)
                var size = 0
                while (size < buffer.size) {
                    val count = input.read(buffer, size, buffer.size - size)
                    if (count < 0) break
                    size += count
                }
                if (size > 1024) false
                else {
                    val fields =
                        buffer
                            .copyOf(size)
                            .toString(Charsets.UTF_8)
                            .lineSequence()
                            .map { it.substringBefore('#').trim() }
                            .filter { it.isNotEmpty() }
                            .toList()
                    fields == listOf("version = 1", "workspace_only = true")
                }
            }
        } catch (_: java.io.FileNotFoundException) {
            false
        }

    fun useWorkspaceOnly() {
        val stream = file.startWrite()
        try {
            stream.write("version = 1\nworkspace_only = true\n".toByteArray(Charsets.UTF_8))
            file.finishWrite(stream)
        } catch (failure: Exception) {
            file.failWrite(stream)
            throw failure
        }
    }
}
