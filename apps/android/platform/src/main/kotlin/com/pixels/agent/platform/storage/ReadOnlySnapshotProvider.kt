package com.pixels.agent.platform.storage

import android.content.ContentProvider
import android.content.ContentValues
import android.database.MatrixCursor
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import android.webkit.MimeTypeMap
import java.io.File
import java.io.FileNotFoundException
import java.nio.file.Files
import java.util.Locale

/** Android enforces URI grants; only completed viewer snapshots can be read, never written. */
class ReadOnlySnapshotProvider : ContentProvider() {
    override fun onCreate() = true

    private fun file(uri: Uri): File {
        val parts = uri.pathSegments
        if (
            uri.scheme != "content" ||
                uri.authority != requireNotNull(context).packageName + ".remote-files" ||
                parts.size != 2 ||
                !parts[0].matches(Regex("[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}")) ||
                parts[1] in listOf("", ".", "..") ||
                parts[1].any { it == '/' || it == '\\' || it == '\u0000' }
        )
            throw FileNotFoundException()
        val root = File(requireNotNull(context).cacheDir, "remote-open")
        val directory = File(root, parts[0])
        val target = File(directory, parts[1])
        if (
            Files.isSymbolicLink(root.toPath()) ||
                Files.isSymbolicLink(directory.toPath()) ||
                Files.isSymbolicLink(target.toPath()) ||
                !target.isFile
        )
            throw FileNotFoundException()
        return target
    }

    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor {
        if (mode != "r") throw FileNotFoundException("read_only_snapshot")
        return ParcelFileDescriptor.open(file(uri), ParcelFileDescriptor.MODE_READ_ONLY)
    }

    override fun getType(uri: Uri): String =
        MimeTypeMap.getSingleton()
            .getMimeTypeFromExtension(file(uri).extension.lowercase(Locale.ROOT))
            ?: "application/octet-stream"

    override fun query(
        uri: Uri,
        projection: Array<out String>?,
        selection: String?,
        selectionArgs: Array<out String>?,
        sortOrder: String?,
    ): MatrixCursor {
        val target = file(uri)
        val columns =
            (projection?.toList() ?: listOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE))
                .filter { it == OpenableColumns.DISPLAY_NAME || it == OpenableColumns.SIZE }
        return MatrixCursor(columns.toTypedArray()).apply {
            addRow(columns.map { if (it == OpenableColumns.SIZE) target.length() else target.name })
        }
    }

    override fun insert(uri: Uri, values: ContentValues?): Uri? =
        throw UnsupportedOperationException("read_only_snapshot")

    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?) =
        throw UnsupportedOperationException("read_only_snapshot")

    override fun update(
        uri: Uri,
        values: ContentValues?,
        selection: String?,
        selectionArgs: Array<out String>?,
    ) = throw UnsupportedOperationException("read_only_snapshot")
}
