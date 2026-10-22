package com.pixels.agent.fixture

import android.annotation.SuppressLint
import android.app.Activity
import android.os.Bundle
import android.util.Log
import android.widget.TextView
import java.security.MessageDigest

/** Debug-only deterministic ACTION_VIEW consumer; displays hashes, never source contents. */
class FileViewFixtureActivity : Activity() {
    @SuppressLint("SetTextI18n")
    override fun onCreate(state: Bundle?) {
        super.onCreate(state)
        val uri = requireNotNull(intent.data)
        require(uri.scheme == "content" && uri.authority == "$packageName.remote-files")
        val bytes =
            contentResolver.openInputStream(uri)!!.use { input ->
                val buffer = ByteArray(131073)
                var count = 0
                while (count < buffer.size) {
                    val size = input.read(buffer, count, buffer.size - count)
                    if (size < 0) break
                    count += size
                }
                buffer.copyOf(count)
            }
        if (bytes.size > 131072) {
            setContentView(
                TextView(this).apply { text = "Pixels QA viewer: file exceeds fixture limit" }
            )
            return
        }
        val hash =
            MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") {
                "%02x".format(it.toInt() and 255)
            }
        val readonly =
            try {
                contentResolver.openOutputStream(uri)?.close()
                false
            } catch (_: Exception) {
                true
            }
        setContentView(
            TextView(this).apply {
                text =
                    "Pixels file view fixture\nsha256=$hash\nbytes=${bytes.size}\nread_only=$readonly"
                textSize = 16f
            }
        )
        Log.i("PixelsFileViewFixture", "file_snapshot_opened")
    }
}
