package com.pixels.agent.platform.storage

import android.content.ContentResolver
import android.database.Cursor
import android.net.Uri
import android.provider.DocumentsContract
import android.system.Os
import com.pixels.agent.core.StorageEntry
import com.pixels.agent.core.StorageFailure
import com.pixels.agent.core.StorageMetadata
import com.pixels.agent.core.StorageMounts
import java.io.InputStream
import java.security.MessageDigest
import java.util.UUID

/** SAF document identifiers stay opaque; they are never converted into filesystem paths. */
internal class DocumentEntry(
    private val resolver: ContentResolver,
    private val tree: Uri,
    private val fixedUri: Uri?,
    private val parent: DocumentEntry? = null,
    private val requestedName: String? = null,
    private val listingMetadata: StorageMetadata? = null,
) : StorageEntry {
    // Entries are recreated for every SDK command. Never cache URI resolution across commands.
    private var resolvedUri: Uri? = fixedUri

    private fun actual(): Uri {
        resolvedUri?.let {
            return it
        }
        val uri = parent?.find(requestedName ?: "")?.actual() ?: throw StorageFailure("not_found")
        resolvedUri = uri
        return uri
    }

    override val name: String
        get() = requestedName ?: metadata().name

    override fun metadata(): StorageMetadata {
        listingMetadata?.let {
            return it
        }
        val uri = actual()
        resolver.query(uri, COLUMNS, null, null, null)?.use { cursor ->
            if (!cursor.moveToFirst()) throw StorageFailure("not_found")
            return readMetadata(cursor, writeGranted())
        }
        throw StorageFailure("provider_unavailable")
    }

    private fun writeGranted(): Boolean =
        resolver.persistedUriPermissions.any { it.uri == tree && it.isWritePermission }

    private fun readMetadata(cursor: Cursor, writeGranted: Boolean): StorageMetadata {
        val directory = cursor.getString(2) == DocumentsContract.Document.MIME_TYPE_DIR
        val flags = cursor.getInt(5)
        val writable =
            if (directory) flags and DocumentsContract.Document.FLAG_DIR_SUPPORTS_CREATE != 0
            else flags and DocumentsContract.Document.FLAG_SUPPORTS_WRITE != 0
        return StorageMetadata(
            cursor.getString(1),
            directory,
            if (directory || cursor.isNull(3)) null else cursor.getLong(3),
            if (cursor.isNull(4)) null else cursor.getLong(4),
            !(writable && writeGranted),
            cursor.getString(0),
        )
    }

    override fun children(): List<StorageEntry> {
        val uri = actual()
        if (!metadata().directory) throw StorageFailure("not_a_directory")
        val children =
            DocumentsContract.buildChildDocumentsUriUsingTree(
                tree,
                DocumentsContract.getDocumentId(uri),
            )
        val result = mutableListOf<StorageEntry>()
        val writable = writeGranted()
        resolver
            .query(
                children,
                COLUMNS,
                null,
                null,
                null,
            )
            ?.use { cursor ->
                while (cursor.moveToNext()) {
                    if (result.size >= 20000) throw StorageFailure("directory_limit")
                    result +=
                        DocumentEntry(
                            resolver,
                            tree,
                            DocumentsContract.buildDocumentUriUsingTree(tree, cursor.getString(0)),
                            listingMetadata = readMetadata(cursor, writable),
                        )
                }
                return result
            }
        throw StorageFailure("provider_unavailable")
    }

    private fun find(name: String): DocumentEntry? {
        val matches = children().filter { it.metadata().name == name }
        if (matches.size > 1) throw StorageFailure("ambiguous_document_name")
        return matches.singleOrNull() as DocumentEntry?
    }

    override fun child(name: String): StorageEntry {
        if (!StorageMounts.validName(name)) throw StorageFailure("invalid_path")
        return DocumentEntry(resolver, tree, null, this, name)
    }

    override fun openRead(): InputStream {
        if (metadata().directory) throw StorageFailure("not_a_file")
        return resolver.openInputStream(actual()) ?: throw StorageFailure("provider_unavailable")
    }

    override fun openLogRead(): InputStream {
        if (metadata().directory) throw StorageFailure("not_a_file")
        val asset =
            resolver.openAssetFileDescriptor(actual(), "r")
                ?: throw StorageFailure("provider_unavailable")
        try {
            val size = Os.fstat(asset.fileDescriptor).st_size
            if (
                asset.startOffset != 0L ||
                    (asset.declaredLength >= 0 && asset.declaredLength != size)
            )
                throw StorageFailure("log_seek_unavailable")
            return asset.createInputStream()
        } catch (failure: Exception) {
            asset.close()
            throw failure
        }
    }

    override fun publish(
        source: () -> InputStream,
        overwrite: Boolean,
        expectedHash: String?,
        checkpoint: () -> Unit,
    ) {
        val container = parent ?: throw StorageFailure("mount_root_protected")
        val targetName = requestedName ?: throw StorageFailure("invalid_path")
        if (container.metadata().readOnly) throw StorageFailure("access_denied")
        val existing = container.find(targetName)
        if (existing != null && !overwrite) throw StorageFailure("already_exists")
        if (existing?.metadata()?.readOnly == true) throw StorageFailure("access_denied")
        if (existing?.metadata()?.directory == true) throw StorageFailure("not_a_file")
        if (expectedHash != null && existing?.digest() != expectedHash) {
            throw StorageFailure("hash_conflict")
        }
        val stageName = ".pixels-${UUID.randomUUID()}.part"
        var stage =
            try {
                DocumentsContract.createDocument(
                    resolver,
                    container.actual(),
                    "application/octet-stream",
                    stageName,
                ) ?: throw StorageFailure("publication_unconfirmed")
            } catch (_: Exception) {
                throw StorageFailure("publication_unconfirmed")
            }
        var backup: Uri? = null
        var published = false
        var targetAttempted = false
        try {
            resolver.openOutputStream(stage, "wt")?.use { output ->
                source().use { it.copyTo(output, 65536) }
            } ?: throw StorageFailure("provider_write_failed")
            val current = container.find(targetName)
            if (current?.actual() != existing?.actual()) throw StorageFailure("hash_conflict")
            if (expectedHash != null && current?.digest() != expectedHash) {
                throw StorageFailure("hash_conflict")
            }
            checkpoint()
            if (existing != null) {
                targetAttempted = true
                backup =
                    DocumentsContract.renameDocument(
                        resolver,
                        existing.actual(),
                        "$stageName.previous",
                    ) ?: throw StorageFailure("provider_replace_unavailable")
            }
            targetAttempted = true
            stage =
                DocumentsContract.renameDocument(resolver, stage, targetName)
                    ?: throw StorageFailure("provider_rename_unavailable")
            if (DocumentEntry(resolver, tree, stage).metadata().name != targetName) {
                throw StorageFailure("publication_name_conflict")
            }
            published = true
            backup?.let {
                if (!DocumentsContract.deleteDocument(resolver, it)) {
                    throw StorageFailure("publication_unconfirmed")
                }
            }
            backup = null
        } catch (failure: Exception) {
            if (targetAttempted) throw StorageFailure("publication_unconfirmed")
            throw failure
        } finally {
            // A provider may apply a rename and lose its response. Do not blindly delete
            // the possibly published document or move a backup after an uncertain RPC.
            if (!published && !targetAttempted) {
                var cleanupFailed = false
                try {
                    if (!DocumentsContract.deleteDocument(resolver, stage)) cleanupFailed = true
                } catch (_: Exception) {
                    cleanupFailed = true
                }
                if (cleanupFailed) throw StorageFailure("staging_cleanup_unconfirmed")
            }
        }
    }

    override fun mkdir(parents: Boolean, existOk: Boolean, checkpoint: () -> Unit) {
        val container = parent ?: throw StorageFailure("mount_root_protected")
        val targetName = requestedName ?: throw StorageFailure("invalid_path")
        if (parents) container.ensureDirectory(checkpoint)
        if (container.metadata().readOnly) throw StorageFailure("access_denied")
        val existing = container.find(targetName)
        if (existing != null) {
            if (existOk && existing.metadata().directory) return
            throw StorageFailure("already_exists")
        }
        checkpoint()
        try {
            val uri =
                DocumentsContract.createDocument(
                    resolver,
                    container.actual(),
                    DocumentsContract.Document.MIME_TYPE_DIR,
                    targetName,
                ) ?: throw StorageFailure("publication_unconfirmed")
            if (DocumentEntry(resolver, tree, uri).metadata().name != targetName) {
                DocumentsContract.deleteDocument(resolver, uri)
                throw StorageFailure("publication_name_conflict")
            }
        } catch (_: Exception) {
            throw StorageFailure("publication_unconfirmed")
        }
    }

    private fun ensureDirectory(checkpoint: () -> Unit) {
        try {
            if (!metadata().directory) throw StorageFailure("not_a_directory")
        } catch (error: StorageFailure) {
            if (error.code != "not_found") throw error
            mkdir(parents = true, existOk = true, checkpoint = checkpoint)
        }
    }

    override fun bindParent(identity: String): StorageEntry {
        if (parent?.metadata()?.identity != identity)
            throw StorageFailure("destination_parent_changed")
        // Keep this resolved parent document URI through staging and publication.
        parent.actual()
        return this
    }

    override fun rename(name: String, checkpoint: () -> Unit) {
        if (!StorageMounts.validName(name)) throw StorageFailure("invalid_path")
        val container = parent ?: throw StorageFailure("mount_root_protected")
        if (!writeGranted()) throw StorageFailure("access_denied")
        if (container.find(name) != null) throw StorageFailure("already_exists")
        val uri = actual()
        checkpoint()
        try {
            val result =
                DocumentsContract.renameDocument(resolver, uri, name)
                    ?: throw StorageFailure("publication_unconfirmed")
            if (DocumentEntry(resolver, tree, result).metadata().name != name)
                throw StorageFailure("publication_name_conflict")
        } catch (_: Exception) {
            throw StorageFailure("publication_unconfirmed")
        }
    }

    override fun delete(checkpoint: () -> Unit) {
        if (parent == null) throw StorageFailure("mount_root_protected")
        if (resolver.persistedUriPermissions.none { it.uri == tree && it.isWritePermission })
            throw StorageFailure("access_denied")
        if (metadata().directory && children().isNotEmpty())
            throw StorageFailure("directory_not_empty")
        val target = actual()
        checkpoint()
        try {
            if (!DocumentsContract.deleteDocument(resolver, target))
                throw StorageFailure("publication_unconfirmed")
        } catch (_: Exception) {
            throw StorageFailure("publication_unconfirmed")
        }
    }

    private fun digest(): String =
        openRead().use { input ->
            val hash = MessageDigest.getInstance("SHA-256")
            val buffer = ByteArray(65536)
            var total = 0L
            while (true) {
                val count = input.read(buffer)
                if (count < 0) break
                total += count
                if (total > AndroidFileStorage.MAXIMUM_FILE_BYTES)
                    throw StorageFailure("file_size_limit")
                hash.update(buffer, 0, count)
            }
            hash.digest().joinToString("") { "%02x".format(it.toInt() and 0xff) }
        }

    companion object {
        private val COLUMNS =
            arrayOf(
                DocumentsContract.Document.COLUMN_DOCUMENT_ID,
                DocumentsContract.Document.COLUMN_DISPLAY_NAME,
                DocumentsContract.Document.COLUMN_MIME_TYPE,
                DocumentsContract.Document.COLUMN_SIZE,
                DocumentsContract.Document.COLUMN_LAST_MODIFIED,
                DocumentsContract.Document.COLUMN_FLAGS,
            )
    }
}
