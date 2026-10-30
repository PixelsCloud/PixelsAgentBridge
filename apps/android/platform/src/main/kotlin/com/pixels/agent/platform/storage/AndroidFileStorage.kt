package com.pixels.agent.platform.storage

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.DocumentsContract
import com.pixels.agent.core.AtomicFilePublisher
import com.pixels.agent.core.StorageEntry
import com.pixels.agent.core.StorageFailure
import com.pixels.agent.core.StorageMetadata
import com.pixels.agent.core.StorageMounts
import java.io.File
import java.security.MessageDigest
import java.util.UUID
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import org.json.JSONArray
import org.json.JSONObject

data class StorageGrant(val mount: String, val name: String, val writable: Boolean)

/** Owns an isolated workspace and Android's persisted, user-selected directory grants. */
class AndroidFileStorage(context: Context, private val publisher: AtomicFilePublisher) {
    private val application = context.applicationContext
    private val resolver = context.applicationContext.contentResolver
    private val workspace = File(context.noBackupFilesDir, "workspace").apply { mkdirs() }
    val stagingDirectory = File(context.noBackupFilesDir, "file-staging").apply { mkdirs() }
    private val currentGrants = MutableStateFlow<List<StorageGrant>>(emptyList())
    val grants = currentGrants.asStateFlow()
    private val mounts = StorageMounts(::roots)
    private val managed = ManagedStorage(mounts)

    /** Check persisted authorization without calling a possibly blocked document provider. */
    fun checkMountAccess(path: String) {
        if (path == "/") return // Control authorization is checked by RemoteFileAdapter.
        val parts = path.removePrefix("/").split('/')
        if (
            !path.startsWith('/') ||
                path.length > 4096 ||
                parts.any { !StorageMounts.validName(it) }
        )
            throw StorageFailure("invalid_path")
        if (parts.first() !in mounts.rootNames()) throw StorageFailure("mount_not_found")
    }

    /** A package reader owns and deletes this immutable authorized snapshot after use. */
    fun packageSnapshot(path: String): File {
        val uri = exportForViewing(path).first
        return File(
            File(File(application.cacheDir, "remote-open"), uri.pathSegments[0]),
            uri.pathSegments[1],
        )
    }

    /** Export a bounded read-only snapshot; the viewer never receives a private filesystem path. */
    fun exportForViewing(path: String): Pair<Uri, String> {
        val entry = mounts.resolve(path)
        val metadata = entry.metadata()
        if (metadata.directory) throw StorageFailure("not_a_file")
        if ((metadata.size ?: 0) > 32L * 1024 * 1024) throw StorageFailure("file_view_size_limit")
        val root = File(application.cacheDir, "remote-open").apply { mkdirs() }
        // Only our generated UUID directories and regular snapshots live in this root.
        var used = 0L
        var liveSnapshots = 0
        for (directory in root.listFiles().orEmpty()) {
            if (
                !directory.name.matches(Regex("[0-9a-f-]{36}")) ||
                    java.nio.file.Files.isSymbolicLink(directory.toPath())
            )
                continue
            for (file in directory.listFiles().orEmpty()) {
                if (!file.isFile || java.nio.file.Files.isSymbolicLink(file.toPath())) continue
                if (System.currentTimeMillis() - file.lastModified() > 86400000) file.delete()
                if (file.exists()) {
                    used += file.length()
                    liveSnapshots++
                }
            }
            directory.delete() // Nonrecursive; never deletes a live snapshot.
        }
        if (liveSnapshots >= 16) throw StorageFailure("file_view_cache_limit")
        val directory = File(root, UUID.randomUUID().toString()).apply { mkdirs() }
        val name =
            metadata.name.takeIf {
                it.isNotEmpty() &&
                    it != "." &&
                    it != ".." &&
                    !it.contains('/') &&
                    !it.contains('\\')
            } ?: "document"
        val target = File(directory, name)
        try {
            var count = 0L
            entry.openRead().use { input ->
                java.io.FileOutputStream(target).use { output ->
                    val buffer = ByteArray(65536)
                    while (true) {
                        val size = input.read(buffer)
                        if (size < 0) break
                        count += size
                        if (count > 32L * 1024 * 1024 || used + count > 64L * 1024 * 1024)
                            throw StorageFailure("file_view_cache_limit")
                        output.write(buffer, 0, size)
                    }
                    output.fd.sync()
                }
            }
            val after = entry.metadata()
            if (metadata != after) throw StorageFailure("file_changed_during_snapshot")
            val mime =
                android.webkit.MimeTypeMap.getSingleton()
                    .getMimeTypeFromExtension(
                        name.substringAfterLast('.', "").lowercase(java.util.Locale.ROOT)
                    ) ?: "application/octet-stream"
            return Uri.Builder()
                .scheme("content")
                .authority(application.packageName + ".remote-files")
                .appendPath(directory.name)
                .appendPath(name)
                .build() to mime
        } catch (failure: Exception) {
            target.delete()
            directory.delete()
            throw failure
        }
    }

    /** Recheck the source grant before exposing cached document contents. */
    fun checkDocumentAccess(path: String) {
        if (mounts.resolve(path).metadata().directory) throw StorageFailure("not_a_file")
    }

    private fun mount(uri: Uri): String =
        "shared-" +
            MessageDigest.getInstance("SHA-256")
                .digest(uri.toString().toByteArray())
                .take(8)
                .joinToString("") { "%02x".format(it.toInt() and 0xff) }

    private fun roots(): Map<String, StorageEntry> = buildMap {
        put("workspace", WorkspaceEntry(workspace, publisher = publisher))
        for (permission in resolver.persistedUriPermissions) {
            if (!permission.isReadPermission || !DocumentsContract.isTreeUri(permission.uri))
                continue
            val tree = permission.uri
            val root =
                DocumentsContract.buildDocumentUriUsingTree(
                    tree,
                    DocumentsContract.getTreeDocumentId(tree),
                )
            put(mount(tree), DocumentEntry(resolver, tree, root))
        }
    }

    fun refresh() {
        currentGrants.value =
            resolver.persistedUriPermissions.mapNotNull { permission ->
                if (!permission.isReadPermission || !DocumentsContract.isTreeUri(permission.uri))
                    return@mapNotNull null
                val name =
                    try {
                        roots()[mount(permission.uri)]?.metadata()?.name ?: "已授权目录"
                    } catch (_: Exception) {
                        "目录暂不可用"
                    }
                StorageGrant(mount(permission.uri), name, permission.isWritePermission)
            }
    }

    fun grant(uri: Uri, flags: Int) {
        if (!DocumentsContract.isTreeUri(uri)) throw StorageFailure("directory_required")
        val modes =
            flags and
                (Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
        if (modes and Intent.FLAG_GRANT_READ_URI_PERMISSION == 0)
            throw StorageFailure("read_permission_required")
        resolver.takePersistableUriPermission(uri, modes)
        refresh()
    }

    fun revoke(id: String) {
        val permission =
            resolver.persistedUriPermissions.singleOrNull { mount(it.uri) == id }
                ?: throw StorageFailure("mount_not_found")
        var flags = 0
        if (permission.isReadPermission) flags = flags or Intent.FLAG_GRANT_READ_URI_PERMISSION
        if (permission.isWritePermission) flags = flags or Intent.FLAG_GRANT_WRITE_URI_PERMISSION
        resolver.releasePersistableUriPermission(permission.uri, flags)
        refresh()
    }

    fun makeReadOnly(id: String) {
        val permission =
            resolver.persistedUriPermissions.singleOrNull { mount(it.uri) == id }
                ?: throw StorageFailure("mount_not_found")
        if (permission.isWritePermission) {
            resolver.releasePersistableUriPermission(
                permission.uri,
                Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
            )
        }
        refresh()
    }

    /** Called on the IO dispatcher through the authenticated native mailbox only. */
    fun execute(input: JSONObject, checkpoint: () -> Unit = {}): JSONObject {
        checkpoint()
        val path = input.getString("path")
        if (input.getString("action") == "managed_context")
            return JSONObject()
                .put(
                    "identity",
                    JSONObject()
                        .put("mode", "service")
                        .put("account_id", "uid:${android.os.Process.myUid()}")
                        .put("account_name", application.packageName)
                        .put("home", "/workspace")
                        .put("primary_group", android.system.Os.getgid())
                        .put("session_id", JSONObject.NULL)
                        .put("logon_id", JSONObject.NULL)
                        .put("environment_source", "service_process"),
                )
        if (input.getString("action") == "managed")
            return managed.execute(
                input.getJSONObject("operation"),
                input.getString("filesystem_scope"),
                checkpoint,
            )
        if (input.getString("action") == "managed_list") return managed.listing(path)
        if (input.getString("action") == "check_staging") {
            checkStaging(input.getLong("additional_bytes"))
            return JSONObject()
        }
        if (path == "/" && input.getString("action") == "stat") {
            return JSONObject()
                .put("metadata", metadataJson(StorageMetadata("/", true, null, null, true)))
        }
        if (path == "/" && input.getString("action") == "list") {
            val after = if (input.isNull("after")) "" else input.getString("after")
            val limit = input.optInt("limit", 64).coerceIn(1, 64)
            val names = mounts.rootNames().filter { compareNames(it, after) > 0 }
            return JSONObject()
                .put(
                    "entries",
                    JSONArray(
                        names.take(limit).map {
                            metadataJson(StorageMetadata(it, true, null, null, false))
                        }
                    ),
                )
                .put("has_more", names.size > limit)
        }
        val entry =
            input.optJSONObject("destination_parent")?.let {
                managed.boundTarget(path, it.getString("identity"))
            } ?: mounts.resolve(path)
        return when (input.getString("action")) {
            "check_upload" -> {
                val existing =
                    try {
                        entry.metadata()
                    } catch (failure: StorageFailure) {
                        if (failure.code != "not_found") throw failure
                        null
                    }
                if (existing?.directory == true) throw StorageFailure("not_a_file")
                if (existing != null && !input.optBoolean("overwrite")) {
                    throw StorageFailure("already_exists")
                }
                val parent = mounts.resolve(path.substringBeforeLast('/')).metadata()
                if (!parent.directory) throw StorageFailure("parent_missing")
                if (parent.readOnly || existing?.readOnly == true)
                    throw StorageFailure("access_denied")
                JSONObject()
            }
            "stat" -> JSONObject().put("metadata", metadataJson(entry.metadata()))
            "log_probe" -> LogWindow.read(entry, input)
            "list" -> {
                val after = if (input.isNull("after")) "" else input.getString("after")
                val limit = input.optInt("limit", 64).coerceIn(1, 64)
                // UTF-8 byte order agrees with the existing Rust directory cursor contract.
                val entries =
                    entry
                        .children()
                        .map { it.metadata() }
                        .filter { compareNames(it.name, after) > 0 }
                        .sortedWith { left, right -> compareNames(left.name, right.name) }
                JSONObject()
                    .put("entries", JSONArray(entries.take(limit).map(::metadataJson)))
                    .put("has_more", entries.size > limit)
            }
            "snapshot" -> {
                val target = staged(input.getString("staging_id"))
                val maximum = input.getLong("maximum_bytes")
                if (maximum !in 0..MAXIMUM_FILE_BYTES) throw StorageFailure("file_size_limit")
                val before = entry.metadata()
                input.optJSONObject("source_constraint")?.let { managed.source(path, it, before) }
                if (before.directory) throw StorageFailure("not_a_file")
                if (before.size?.let { it > maximum } == true)
                    throw StorageFailure("file_size_limit")
                checkStaging(before.size ?: maximum)
                if (!target.createNewFile()) throw StorageFailure("staging_conflict")
                var complete = false
                try {
                    val hash = MessageDigest.getInstance("SHA-256")
                    var total = 0L
                    entry.openRead().use { source ->
                        target.outputStream().use { output ->
                            val buffer = ByteArray(65536)
                            while (true) {
                                checkpoint()
                                val count = source.read(buffer)
                                checkpoint()
                                if (count < 0) break
                                total += count
                                if (total > maximum) throw StorageFailure("file_size_limit")
                                hash.update(buffer, 0, count)
                                output.write(buffer, 0, count)
                            }
                            output.fd.sync()
                        }
                    }
                    val after = entry.metadata()
                    if (before != after) throw StorageFailure("version_conflict")
                    if (before.size != null && before.size != total)
                        throw StorageFailure("version_conflict")
                    val result =
                        JSONObject()
                            .put("metadata", metadataJson(after.copy(size = total)))
                            .put("size", total)
                            .put("sha256", hex(hash.digest()))
                    complete = true
                    result
                } finally {
                    if (!complete) target.delete()
                }
            }
            "publish" -> {
                val stage = staged(input.getString("staging_id"))
                if (!stage.isFile || stage.length() > MAXIMUM_FILE_BYTES)
                    throw StorageFailure("invalid_staging_file")
                if (path.startsWith("/workspace/")) checkStaging(stage.length())
                val expected =
                    input.optString("expected_hash").takeIf { it.isNotEmpty() && it != "null" }
                entry.publish(
                    { stage.inputStream() },
                    input.optBoolean("overwrite"),
                    expected,
                    checkpoint,
                )
                try {
                    val actual = entry.metadata()
                    JSONObject().put("metadata", metadataJson(actual)).also { result ->
                        input.optJSONObject("transfer_metadata")?.let {
                            result.put("metadata_result", managed.metadataResult(it, actual))
                        }
                    }
                } catch (_: Exception) {
                    throw StorageFailure("publication_unconfirmed")
                }
            }
            "mkdir" -> {
                entry.mkdir(input.optBoolean("parents"), input.optBoolean("exist_ok"), checkpoint)
                try {
                    JSONObject().put("metadata", metadataJson(entry.metadata()))
                } catch (_: Exception) {
                    throw StorageFailure("publication_unconfirmed")
                }
            }
            "delete" -> {
                input.optJSONObject("expected_metadata")?.let { expected ->
                    val current = metadataJson(entry.metadata())
                    val keys =
                        if (current.optBoolean("directory")) listOf("directory", "identity")
                        else listOf("directory", "identity", "size", "modified_at_unix_ms")
                    if (keys.any { expected.opt(it)?.toString() != current.opt(it)?.toString() })
                        throw StorageFailure("version_conflict")
                }
                input
                    .optString("expected_hash")
                    .takeIf { it.isNotEmpty() && it != "null" }
                    ?.let { expected ->
                        val hash = MessageDigest.getInstance("SHA-256")
                        entry.openRead().use { source ->
                            val buffer = ByteArray(65536)
                            var total = 0L
                            while (true) {
                                val count = source.read(buffer)
                                if (count < 0) break
                                total += count
                                if (total > MAXIMUM_FILE_BYTES)
                                    throw StorageFailure("file_size_limit")
                                hash.update(buffer, 0, count)
                            }
                        }
                        if (hex(hash.digest()) != expected) throw StorageFailure("hash_conflict")
                    }
                entry.delete(checkpoint)
                JSONObject()
            }
            else -> throw StorageFailure("unsupported_storage_action")
        }
    }

    private fun checkStaging(additional: Long) {
        if (additional !in 0..MAXIMUM_FILE_BYTES) throw StorageFailure("file_size_limit")
        val files = stagingDirectory.listFiles() ?: throw StorageFailure("staging_unavailable")
        val used = files.sumOf { it.length() }
        if (files.size >= 10000 || used > 2 * MAXIMUM_FILE_BYTES - additional)
            throw StorageFailure("staging_quota_exceeded")
        if (stagingDirectory.usableSpace < additional + 64 * 1024 * 1024)
            throw StorageFailure("insufficient_storage")
    }

    private fun staged(id: String): File {
        if (UUID.fromString(id).toString() != id) throw StorageFailure("invalid_staging_id")
        return File(stagingDirectory, "$id.part")
    }

    private fun metadataJson(value: StorageMetadata): JSONObject =
        JSONObject()
            .put("name", value.name)
            .put("directory", value.directory)
            .put("size", value.size ?: JSONObject.NULL)
            .put("modified_at_unix_ms", value.modifiedAtMillis ?: JSONObject.NULL)
            .put("readonly", value.readOnly)
            .put("identity", value.identity ?: JSONObject.NULL)

    private fun compareNames(left: String, right: String): Int {
        val a = left.toByteArray(Charsets.UTF_8)
        val b = right.toByteArray(Charsets.UTF_8)
        for (index in 0 until minOf(a.size, b.size)) {
            val compare = (a[index].toInt() and 255).compareTo(b[index].toInt() and 255)
            if (compare != 0) return compare
        }
        return a.size.compareTo(b.size)
    }

    private fun hex(bytes: ByteArray): String =
        bytes.joinToString("") { "%02x".format(it.toInt() and 255) }

    companion object {
        const val MAXIMUM_FILE_BYTES = 1024L * 1024 * 1024
    }
}
