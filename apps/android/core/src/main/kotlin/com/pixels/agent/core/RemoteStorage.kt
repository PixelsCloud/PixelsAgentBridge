package com.pixels.agent.core

import java.io.File
import java.io.InputStream

fun interface AtomicFilePublisher {
    fun publish(source: File, destination: File, replace: Boolean)

    fun publishBound(source: File, destination: File, replace: Boolean, parentIdentity: String) {
        throw StorageFailure("bound_publication_unavailable")
    }
}

/** A granted document or workspace entry. Implementations never expose credentials as a root. */
interface StorageEntry {
    val name: String

    fun metadata(): StorageMetadata

    fun children(): List<StorageEntry>

    fun child(name: String): StorageEntry

    fun openRead(): InputStream

    /** A full-file handle for bounded random access; sliced provider assets must reject it. */
    fun openLogRead(): InputStream = openRead()

    /** Publish a complete staged file; failures must not be reported as a completed write. */
    fun publish(
        source: () -> InputStream,
        overwrite: Boolean,
        expectedHash: String?,
        checkpoint: () -> Unit = {},
    )

    fun mkdir(parents: Boolean, existOk: Boolean, checkpoint: () -> Unit = {})

    fun delete(checkpoint: () -> Unit = {})

    fun bindParent(identity: String): StorageEntry {
        throw StorageFailure("bound_publication_unavailable")
    }

    fun rename(name: String, checkpoint: () -> Unit = {}) {
        throw StorageFailure("rename_unavailable")
    }
}

data class StorageMetadata(
    val name: String,
    val directory: Boolean,
    val size: Long?,
    val modifiedAtMillis: Long?,
    val readOnly: Boolean,
    val identity: String? = null,
)

class StorageFailure(val code: String) : Exception(code)

/** Remote paths refer to explicit mounts, never Android's entire filesystem. */
class StorageMounts(private val roots: () -> Map<String, StorageEntry>) {
    fun resolve(path: String): StorageEntry {
        if (!path.startsWith('/') || path.length > 4096 || path.indexOf('\u0000') >= 0) {
            throw StorageFailure("invalid_path")
        }
        val components = path.removePrefix("/").split('/')
        if (components.isEmpty() || components.any { !validName(it) }) {
            throw StorageFailure("invalid_path")
        }
        var entry = roots()[components.first()] ?: throw StorageFailure("mount_not_found")
        for (component in components.drop(1)) entry = entry.child(component)
        return entry
    }

    fun rootNames(): List<String> = roots().keys.sorted()

    companion object {
        fun validName(name: String): Boolean =
            name.isNotEmpty() &&
                name != "." &&
                name != ".." &&
                name.toByteArray(Charsets.UTF_8).size <= 255 &&
                name.none { it == '/' || it == '\\' || it == '\u0000' }
    }
}
