package com.pixels.agent.platform.capture

import android.graphics.Bitmap
import com.pixels.agent.core.CaptureFailure
import com.pixels.agent.core.CaptureResult
import java.io.ByteArrayOutputStream

internal object FrameEncoder {
    fun encode(bitmap: Bitmap): CaptureResult {
        if (bitmap.width.toLong() * bitmap.height > 16L * 1024 * 1024) {
            return CaptureResult.Failed(CaptureFailure.PROTECTED_OR_REJECTED)
        }
        val output = ByteArrayOutputStream()
        if (
            !bitmap.compress(Bitmap.CompressFormat.JPEG, 80, output) ||
                output.size() > 8 * 1024 * 1024
        ) {
            return CaptureResult.Failed(CaptureFailure.PROTECTED_OR_REJECTED)
        }
        return CaptureResult.Frame(bitmap.width, bitmap.height, output.toByteArray())
    }
}
