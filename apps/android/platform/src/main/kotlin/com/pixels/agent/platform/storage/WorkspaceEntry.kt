package com.pixels.agent.platform.storage

import com.pixels.agent.core.AtomicFilePublisher
import com.pixels.agent.core.StorageEntry
import com.pixels.agent.core.StorageFailure
import com.pixels.agent.core.StorageMetadata
import com.pixels.agent.core.StorageMounts
import java.io.File
import java.io.InputStream
import java.nio.file.Files
import java.nio.file.LinkOption
import java.nio.file.attribute.BasicFileAttributes
import java.security.MessageDigest
import java.util.UUID

/** Dedicated app workspace only. Symlinks are rejected at every path component. */
internal class WorkspaceEntry(
    private val root: File,
    private val file: File = root,
    private val publisher: AtomicFilePublisher,
    private val parentIdentity: String? = null,
) : StorageEntry {
    override val name: String
        get() = file.name

    private fun checked(): File {
        var current: File? = file
        while (current != null) {
            if (Files.isSymbolicLink(current.toPath())) throw StorageFailure("symlink_not_allowed")
            if (current == root) return file
            current = current.parentFile
        }
        throw StorageFailure("access_denied")
    }

    override fun metadata(): StorageMetadata {
        val target = checked()
        if (!target.exists()) throw StorageFailure("not_found")
        return StorageMetadata(
            target.name,
            target.isDirectory,
            target.takeIf { it.isFile }?.length(),
            target.lastModified().takeIf { it > 0 },
            !target.canWrite(),
            android.system.Os.lstat(target.absolutePath).let { "${it.st_dev}:${it.st_ino}" },
        )
    }

    override fun children(): List<StorageEntry> {
        val entries = checked().listFiles() ?: throw StorageFailure("not_a_directory")
        if (entries.size > 20000) throw StorageFailure("directory_limit")
        return entries.map { WorkspaceEntry(root, it, publisher) }
    }

    override fun child(name: String): StorageEntry {
        if (!StorageMounts.validName(name)) throw StorageFailure("invalid_path")
        checked()
        return WorkspaceEntry(root, File(file, name), publisher)
    }

    override fun openRead(): InputStream {
        val target = checked()
        if (!target.isFile) throw StorageFailure("not_a_file")
        return target.inputStream()
    }

    override fun publish(
        source: () -> InputStream,
        overwrite: Boolean,
        expectedHash: String?,
        checkpoint: () -> Unit,
    ) {
        val target = checked()
        if (target == root) throw StorageFailure("mount_root_protected")
        if (target.exists() && !target.isFile) throw StorageFailure("not_a_file")
        if (target.exists() && !overwrite) throw StorageFailure("already_exists")
        if (expectedHash != null && (!target.exists() || digest(target) != expectedHash)) {
            throw StorageFailure("hash_conflict")
        }
        val parent = target.parentFile ?: throw StorageFailure("invalid_path")
        if (!parent.isDirectory) throw StorageFailure("parent_missing")
        val staging = File(root.parentFile, "file-staging")
        if (!staging.isDirectory && !staging.mkdirs()) throw StorageFailure("staging_unavailable")
        val stage = File(staging, "${UUID.randomUUID()}.part")
        try {
            if (!stage.createNewFile()) throw StorageFailure("staging_conflict")
            stage.outputStream().use { output ->
                source().use { it.copyTo(output, 65536) }
                output.fd.sync()
            }
            checked()
            if (expectedHash != null && (!target.exists() || digest(target) != expectedHash)) {
                throw StorageFailure("hash_conflict")
            }
            checkpoint()
            if (parentIdentity == null) publisher.publish(stage, target, overwrite)
            else publisher.publishBound(stage, target, overwrite, parentIdentity)
        } finally {
            stage.delete()
        }
    }

    override fun mkdir(parents: Boolean, existOk: Boolean, checkpoint: () -> Unit) {
        val target = checked()
        if (target.exists()) {
            if (existOk && target.isDirectory) return
            throw StorageFailure("already_exists")
        }
        checkpoint()
        if (!(if (parents) target.mkdirs() else target.mkdir()))
            throw StorageFailure("mkdir_failed")
    }

    override fun delete(checkpoint: () -> Unit) {
        val target = checked()
        if (target == root) throw StorageFailure("mount_root_protected")
        if (!target.exists()) throw StorageFailure("not_found")
        checkpoint()
        if (!target.delete()) throw StorageFailure("delete_failed")
    }

    override fun bindParent(identity: String): StorageEntry = WorkspaceEntry(root, file, publisher, identity)

    override fun rename(name: String, checkpoint: () -> Unit) {
        if (!StorageMounts.validName(name)) throw StorageFailure("invalid_path")
        val source = checked()
        if (source == root) throw StorageFailure("mount_root_protected")
        if (!source.exists()) throw StorageFailure("not_found")
        val target = File(source.parentFile, name)
        if (target.exists()) throw StorageFailure("already_exists")
        checkpoint()
        publisher.publish(source, target, false)
    }

    private fun digest(target: File): String =
        target.inputStream().use { input ->
            val hash = MessageDigest.getInstance("SHA-256")
            val buffer = ByteArray(65536)
            while (true) {
                val count = input.read(buffer)
                if (count < 0) break
                hash.update(buffer, 0, count)
            }
            hash.digest().joinToString("") { "%02x".format(it.toInt() and 0xff) }
        }
}
