package com.pixels.agent

import android.util.AtomicFile
import com.pixels.agent.core.AppearancePreferences
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

/** Separate from connection settings and credentials; a failed read never permits an overwrite. */
internal class ProductPreferences(directory: File) {
    private val file = AtomicFile(File(directory, "appearance.toml"))
    private val mutex = Mutex()
    private val mutable = MutableStateFlow(AppearancePreferences())
    val state = mutable.asStateFlow()
    val failed = MutableStateFlow(false)

    suspend fun load() =
        withContext(Dispatchers.IO) {
            mutex.withLock {
                try {
                    mutable.value = read()
                    failed.value = false
                } catch (_: Exception) {
                    failed.value = true
                }
            }
        }

    suspend fun save(value: AppearancePreferences) =
        withContext(Dispatchers.IO) {
            mutex.withLock {
                try {
                    read()
                    val stream = file.startWrite()
                    try {
                        stream.write(value.toToml().toByteArray(Charsets.UTF_8))
                        file.finishWrite(stream)
                    } catch (error: Exception) {
                        file.failWrite(stream)
                        throw error
                    }
                    mutable.value = value
                    failed.value = false
                } catch (_: Exception) {
                    failed.value = true
                }
            }
        }

    private fun read(): AppearancePreferences =
        try {
            file.openRead().use {
                val bytes = ByteArray(4097)
                var size = 0
                while (size < bytes.size) {
                    val count = it.read(bytes, size, bytes.size - size)
                    if (count < 0) break
                    size += count
                }
                check(size <= 4096)
                AppearancePreferences.fromToml(bytes.copyOf(size).toString(Charsets.UTF_8))
            }
        } catch (_: java.io.FileNotFoundException) {
            AppearancePreferences()
        }
}
