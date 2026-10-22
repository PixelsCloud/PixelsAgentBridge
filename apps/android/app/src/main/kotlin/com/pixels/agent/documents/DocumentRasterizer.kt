package com.pixels.agent.documents

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Matrix
import android.graphics.pdf.PdfRenderer
import android.media.ExifInterface
import android.os.ParcelFileDescriptor
import android.system.Os
import android.system.OsConstants

/** Bounded decoding shared by OCR and previews; runs only in the isolated worker. */
internal object DocumentRasterizer {
    fun bitmap(input: ParcelFileDescriptor, extension: String, page: Int, dimension: Int): Bitmap {
        require(dimension in 256..2048)
        Os.lseek(input.fileDescriptor, 0, OsConstants.SEEK_SET)
        return if (extension.equals("pdf", ignoreCase = true)) {
            PdfRenderer(ParcelFileDescriptor.dup(input.fileDescriptor)).use { renderer ->
                require(page in 1..renderer.pageCount && renderer.pageCount <= 500)
                renderer.openPage(page - 1).use { source ->
                    require(source.width > 0 && source.height > 0)
                    val scale = dimension.toDouble() / maxOf(source.width, source.height)
                    val result =
                        Bitmap.createBitmap(
                            maxOf(1, (source.width * scale).toInt()),
                            maxOf(1, (source.height * scale).toInt()),
                            Bitmap.Config.ARGB_8888,
                        )
                    try {
                        result.eraseColor(Color.WHITE)
                        source.render(result, null, null, PdfRenderer.Page.RENDER_MODE_FOR_DISPLAY)
                        result
                    } catch (failure: Throwable) {
                        result.recycle()
                        throw failure
                    }
                }
            }
        } else {
            require(extension.lowercase() in setOf("png", "jpg", "jpeg") && page == 1)
            image(input, dimension)
        }
    }

    private fun image(input: ParcelFileDescriptor, dimension: Int): Bitmap {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeFileDescriptor(input.fileDescriptor, null, bounds)
        require(bounds.outWidth in 1..32768 && bounds.outHeight in 1..32768)
        require(bounds.outWidth.toLong() * bounds.outHeight <= 64L * 1024 * 1024)
        val orientation =
            try {
                Os.lseek(input.fileDescriptor, 0, OsConstants.SEEK_SET)
                ExifInterface(input.fileDescriptor)
                    .getAttributeInt(ExifInterface.TAG_ORIENTATION, 1)
            } catch (_: java.io.IOException) {
                1
            }
        val options =
            BitmapFactory.Options().apply {
                inPreferredConfig = Bitmap.Config.ARGB_8888
                inSampleSize = 1
                while (maxOf(bounds.outWidth, bounds.outHeight) / inSampleSize > dimension) {
                    inSampleSize *= 2
                }
            }
        Os.lseek(input.fileDescriptor, 0, OsConstants.SEEK_SET)
        val decoded =
            checkNotNull(BitmapFactory.decodeFileDescriptor(input.fileDescriptor, null, options))
        var oriented: Bitmap? = null
        try {
            val transform =
                Matrix().apply {
                    when (orientation) {
                        2 -> setScale(-1f, 1f)
                        3 -> setRotate(180f)
                        4 -> setScale(1f, -1f)
                        5 -> {
                            setRotate(90f)
                            postScale(-1f, 1f)
                        }
                        6 -> setRotate(90f)
                        7 -> {
                            setRotate(270f)
                            postScale(-1f, 1f)
                        }
                        8 -> setRotate(270f)
                    }
                }
            oriented =
                Bitmap.createBitmap(decoded, 0, 0, decoded.width, decoded.height, transform, true)
            val result =
                Bitmap.createBitmap(oriented.width, oriented.height, Bitmap.Config.ARGB_8888)
            result.eraseColor(Color.WHITE)
            Canvas(result).drawBitmap(oriented, 0f, 0f, null)
            return result
        } finally {
            if (oriented !== decoded) oriented?.recycle()
            decoded.recycle()
        }
    }
}
