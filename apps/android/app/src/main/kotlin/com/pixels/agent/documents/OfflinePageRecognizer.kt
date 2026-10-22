package com.pixels.agent.documents

import android.content.res.AssetManager
import android.os.ParcelFileDescriptor
import com.pixels.agent.transport.NativeDocumentBridge
import com.pixels.agent.transport.PageRecognizer
import org.json.JSONObject

/** APK model plus one bounded page. The isolated UID never opens application-private paths. */
internal class OfflinePageRecognizer(
    private val assets: AssetManager,
    private val input: ParcelFileDescriptor,
    private val extension: String,
    private val native: NativeDocumentBridge,
) : PageRecognizer {
    private val model by lazy {
        for (library in listOf("jpeg", "pngx", "leptonica", "tesseract")) {
            System.loadLibrary(library)
        }
        assets.open("ocr/chi_sim.traineddata").use { stream ->
            val bytes = ByteArray(2469156)
            var offset = 0
            while (offset < bytes.size) {
                val count = stream.read(bytes, offset, bytes.size - offset)
                check(count > 0)
                offset += count
            }
            check(stream.read() == -1)
            bytes
        }
    }

    override fun recognize(page: Int): String {
        return try {
            val bitmap = DocumentRasterizer.bitmap(input, extension, page, 2048)
            try {
                val pixels = IntArray(bitmap.width * bitmap.height)
                bitmap.getPixels(pixels, 0, bitmap.width, 0, 0, bitmap.width, bitmap.height)
                val gray =
                    ByteArray(pixels.size) { index ->
                        val pixel = pixels[index]
                        (((pixel shr 16 and 255) * 77 +
                                (pixel shr 8 and 255) * 150 +
                                (pixel and 255) * 29) shr 8)
                            .toByte()
                    }
                val envelope =
                    JSONObject(native.recognize(gray, bitmap.width, bitmap.height, model))
                if (!envelope.optBoolean("ok"))
                    JSONObject().put("error", "ocr_recognition_failed").toString()
                else envelope.getJSONObject("value").toString()
            } finally {
                bitmap.recycle()
            }
        } catch (_: Exception) {
            JSONObject().put("error", "ocr_recognition_failed").toString()
        } catch (_: UnsatisfiedLinkError) {
            JSONObject().put("error", "ocr_runtime_unavailable").toString()
        }
    }
}
