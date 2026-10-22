package com.pixels.agent.platform.capture

import android.graphics.Bitmap
import android.media.Image
import android.media.ImageReader
import com.pixels.agent.core.CaptureFailure
import com.pixels.agent.core.CaptureResult
import com.pixels.agent.core.ScreenCaptureProvider
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.withContext

/** Keeps at most one frame. Ownership changes under a lock; encoding runs off the UI thread. */
class ProjectionCaptureProvider : ScreenCaptureProvider {
    private val lock = Any()
    private var latest: Image? = null
    private val currentActive = MutableStateFlow(false)
    val active = currentActive.asStateFlow()

    fun start() = synchronized(lock) { currentActive.value = true }

    fun accept(reader: ImageReader) {
        val image = reader.acquireLatestImage() ?: return
        synchronized(lock) {
            if (currentActive.value) {
                latest?.close()
                latest = image
            } else {
                image.close()
            }
        }
    }

    fun stop() =
        synchronized(lock) {
            currentActive.value = false
            latest?.close()
            latest = null
        }

    override suspend fun capture(): CaptureResult =
        withContext(Dispatchers.Default) {
            val bitmap =
                synchronized(lock) {
                    if (!currentActive.value)
                        return@withContext CaptureResult.Failed(CaptureFailure.CONSENT_REQUIRED)
                    val image =
                        latest
                            ?: return@withContext CaptureResult.Failed(
                                CaptureFailure.FRAME_UNAVAILABLE
                            )
                    val plane = image.planes[0]
                    val paddedWidth = plane.rowStride / plane.pixelStride
                    val buffer = plane.buffer.duplicate().apply { rewind() }
                    val padded =
                        Bitmap.createBitmap(paddedWidth, image.height, Bitmap.Config.ARGB_8888)
                    try {
                        padded.copyPixelsFromBuffer(buffer)
                        Bitmap.createBitmap(padded, 0, 0, image.width, image.height).let {
                            if (it === padded) padded.copy(Bitmap.Config.ARGB_8888, false) else it
                        }
                    } finally {
                        padded.recycle()
                    }
                }
            try {
                FrameEncoder.encode(bitmap)
            } finally {
                bitmap.recycle()
            }
        }
}
