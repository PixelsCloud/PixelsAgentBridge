package com.pixels.agent.fixture

import android.database.Cursor
import android.database.MatrixCursor
import android.os.CancellationSignal
import android.os.ParcelFileDescriptor
import android.provider.DocumentsContract.Document
import android.provider.DocumentsContract.Root
import android.provider.DocumentsProvider
import java.io.File
import java.io.FileNotFoundException
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit

/** Disabled by default and absent from release: normal SAF consent is still required. */
class SlowDocumentsProvider : DocumentsProvider() {
    private val body = "Pixels slow provider: 原请求正文😀\n".toByteArray()
    private lateinit var fast: File

    override fun onCreate(): Boolean {
        fast = File(requireNotNull(context).cacheDir, "slow-provider-fixture.txt")
        if (!fast.exists()) fast.writeText("Pixels fast provider\n")
        return true
    }

    override fun queryRoots(projection: Array<out String>?): Cursor =
        row(
            projection
                ?: arrayOf(
                    Root.COLUMN_ROOT_ID,
                    Root.COLUMN_DOCUMENT_ID,
                    Root.COLUMN_TITLE,
                    Root.COLUMN_FLAGS,
                    Root.COLUMN_MIME_TYPES,
                    Root.COLUMN_ICON,
                    Root.COLUMN_SUMMARY,
                    Root.COLUMN_AVAILABLE_BYTES,
                ),
            mapOf(
                Root.COLUMN_ROOT_ID to "pixels-slow",
                Root.COLUMN_DOCUMENT_ID to "root",
                Root.COLUMN_TITLE to TITLE,
                Root.COLUMN_FLAGS to (Root.FLAG_LOCAL_ONLY or Root.FLAG_SUPPORTS_IS_CHILD),
                Root.COLUMN_MIME_TYPES to "*/*",
                Root.COLUMN_ICON to android.R.drawable.ic_menu_save,
                Root.COLUMN_SUMMARY to "Debug acceptance fixture",
                Root.COLUMN_AVAILABLE_BYTES to 0L,
            ),
        )

    override fun queryDocument(documentId: String, projection: Array<out String>?): Cursor =
        row(projection ?: COLUMNS, values(documentId))

    override fun queryChildDocuments(
        parentDocumentId: String,
        projection: Array<out String>?,
        sortOrder: String?,
    ): Cursor {
        val ids =
            when (parentDocumentId) {
                "root" -> listOf("slow.txt", "fast.txt", "pipe.log", "slow-list")
                "slow-list" -> {
                    Thread.sleep(20000)
                    listOf("delayed.txt")
                }
                else -> throw FileNotFoundException("Unknown fixture directory")
            }
        return MatrixCursor(projection ?: COLUMNS).apply {
            for (id in ids) {
                val value = values(id)
                addRow(columnNames.map { value[it] })
            }
        }
    }

    override fun isChildDocument(parentDocumentId: String, documentId: String): Boolean =
        (parentDocumentId == "root" &&
            documentId in setOf("slow.txt", "fast.txt", "pipe.log", "slow-list", "delayed.txt")) ||
            (parentDocumentId == "slow-list" && documentId == "delayed.txt")

    override fun openDocument(
        documentId: String,
        mode: String,
        signal: CancellationSignal?,
    ): ParcelFileDescriptor {
        if (mode != "r") throw FileNotFoundException("Read-only acceptance fixture")
        values(documentId)
        if (documentId == "fast.txt" || documentId == "delayed.txt")
            return ParcelFileDescriptor.open(fast, ParcelFileDescriptor.MODE_READ_ONLY)
        if (documentId !in setOf("slow.txt", "pipe.log"))
            throw FileNotFoundException("Not a fixture file")
        val pipe = ParcelFileDescriptor.createPipe()
        try {
            workers.execute {
                ParcelFileDescriptor.AutoCloseOutputStream(pipe[1]).use { output ->
                    try {
                        if (documentId == "slow.txt") Thread.sleep(20000)
                        signal?.throwIfCanceled()
                        output.write(body)
                    } catch (_: Exception) {
                        /* Client cancellation closes only this fixture pipe. */
                    }
                }
            }
        } catch (_: Exception) {
            pipe.forEach { it.close() }
            throw FileNotFoundException("Fixture workers busy")
        }
        return pipe[0]
    }

    private fun values(id: String): Map<String, Any?> {
        if (id !in setOf("root", "slow-list", "slow.txt", "fast.txt", "pipe.log", "delayed.txt"))
            throw FileNotFoundException("Unknown fixture document")
        val directory = id == "root" || id == "slow-list"
        return mapOf(
            Document.COLUMN_DOCUMENT_ID to id,
            Document.COLUMN_DISPLAY_NAME to (if (id == "root") TITLE else id),
            Document.COLUMN_MIME_TYPE to if (directory) Document.MIME_TYPE_DIR else "text/plain",
            Document.COLUMN_SIZE to
                if (directory) null
                else if (id in setOf("fast.txt", "delayed.txt")) fast.length()
                else body.size.toLong(),
            Document.COLUMN_LAST_MODIFIED to fast.lastModified(),
            Document.COLUMN_FLAGS to 0,
        )
    }

    private fun row(columns: Array<out String>, values: Map<String, Any?>): Cursor =
        MatrixCursor(columns).apply { addRow(columnNames.map { values[it] }) }

    companion object {
        const val TITLE = "Pixels 慢存储验收"
        private val COLUMNS =
            arrayOf(
                Document.COLUMN_DOCUMENT_ID,
                Document.COLUMN_DISPLAY_NAME,
                Document.COLUMN_MIME_TYPE,
                Document.COLUMN_SIZE,
                Document.COLUMN_LAST_MODIFIED,
                Document.COLUMN_FLAGS,
            )
        private val workers =
            ThreadPoolExecutor(
                2,
                2,
                0,
                TimeUnit.SECONDS,
                ArrayBlockingQueue(2),
                ThreadPoolExecutor.AbortPolicy(),
            )
    }
}
