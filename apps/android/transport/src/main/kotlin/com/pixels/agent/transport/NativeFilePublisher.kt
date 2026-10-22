package com.pixels.agent.transport

import com.pixels.agent.core.AtomicFilePublisher
import com.pixels.agent.core.StorageFailure
import java.io.File
import org.json.JSONObject

/** Android app policy may forbid hard links; renameat2 supplies atomic no-clobber publication. */
class NativeFilePublisher(private val native: NativeBridge) : AtomicFilePublisher {
    override fun publish(source: File, destination: File, replace: Boolean) {
        val result =
            JSONObject(native.publishFile(source.absolutePath, destination.absolutePath, replace))
        if (!result.optBoolean("ok")) {
            throw StorageFailure(result.optString("error", "publication_unconfirmed"))
        }
    }
}
