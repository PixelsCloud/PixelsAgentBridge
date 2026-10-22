package com.pixels.agent.platform.storage

import android.system.Os
import android.system.OsConstants
import com.pixels.agent.core.StorageEntry
import com.pixels.agent.core.StorageFailure
import java.io.FileInputStream
import java.io.IOException
import java.nio.ByteBuffer
import java.nio.channels.FileChannel
import org.json.JSONObject

/** Bounded, seekable reads only. Provider pipes are never scanned to emulate tail. */
internal object LogWindow {
    fun read(entry: StorageEntry, input: JSONObject): JSONObject {
        val maximum = input.getInt("max_bytes")
        if (maximum !in 4..16384) throw StorageFailure("invalid_range")
        val meta = entry.metadata()
        if (meta.directory) throw StorageFailure("not_a_file")
        try {
            return entry.openLogRead().use { stream ->
                val file =
                    stream as? FileInputStream ?: throw StorageFailure("log_seek_unavailable")
                val before = Os.fstat(file.fd)
                if (!OsConstants.S_ISREG(before.st_mode))
                    throw StorageFailure("log_seek_unavailable")
                val header = slice(file.channel, 0, 3)
                val bom =
                    if (
                        header.size >= 3 &&
                            header[0] == 0xef.toByte() &&
                            header[1] == 0xbb.toByte() &&
                            header[2] == 0xbf.toByte()
                    )
                        3
                    else if (
                        header.size >= 2 &&
                            ((header[0] == 0xff.toByte() && header[1] == 0xfe.toByte()) ||
                                (header[0] == 0xfe.toByte() && header[1] == 0xff.toByte()))
                    )
                        2
                    else 0
                var start =
                    (if (!input.isNull("offset")) input.getLong("offset")
                        else if (!input.isNull("tail_bytes"))
                            (before.st_size - input.getLong("tail_bytes")).coerceAtLeast(0)
                        else 0)
                        .coerceAtLeast(bom.toLong())
                val utf16 = bom == 2 || input.optString("encoding") in setOf("utf16_le", "utf16_be")
                if (!input.isNull("tail_bytes") && utf16 && (start - bom) % 2 != 0L) start++
                if (start < 0 || start > before.st_size) throw StorageFailure("invalid_range")
                val contextStart = (start - 1024).coerceAtLeast(0)
                val window =
                    slice(file.channel, contextStart, (start - contextStart).toInt() + maximum)
                val head = slice(file.channel, 0, 1024)
                val after = Os.fstat(file.fd)
                entry.openLogRead().use { observed ->
                    val named =
                        observed as? FileInputStream ?: throw StorageFailure("log_seek_unavailable")
                    val identity = Os.fstat(named.fd)
                    if (
                        identity.st_dev != before.st_dev ||
                            identity.st_ino != before.st_ino ||
                            after.st_size < before.st_size ||
                            (after.st_size == before.st_size &&
                                after.st_mtime != before.st_mtime) ||
                            !slice(file.channel, contextStart, window.size).contentEquals(window) ||
                            !slice(file.channel, 0, head.size).contentEquals(head)
                    )
                        throw StorageFailure("log_changed")
                }
                JSONObject()
                    .put("start", start)
                    .put("context_start", contextStart)
                    .put("window", hex(window))
                    .put("head", hex(head))
                    .put("identity", "${before.st_dev}:${before.st_ino}")
                    .put(
                        "metadata",
                        JSONObject()
                            .put("directory", false)
                            .put("name", meta.name)
                            .put("size", after.st_size)
                            .put("modified_at_unix_ms", after.st_mtime * 1000)
                            .put("readonly", meta.readOnly),
                    )
            }
        } catch (_: IOException) {
            throw StorageFailure("log_seek_unavailable")
        }
    }

    private fun slice(file: FileChannel, offset: Long, count: Int): ByteArray {
        file.position(offset)
        val buffer = ByteBuffer.allocate(count)
        while (buffer.hasRemaining()) {
            if (file.read(buffer) <= 0) break
        }
        return buffer.array().copyOf(buffer.position())
    }

    private fun hex(bytes: ByteArray): String =
        buildString(bytes.size * 2) {
            val digits = "0123456789abcdef"
            for (byte in bytes) {
                val value = byte.toInt() and 255
                append(digits[value ushr 4])
                append(digits[value and 15])
            }
        }
}
