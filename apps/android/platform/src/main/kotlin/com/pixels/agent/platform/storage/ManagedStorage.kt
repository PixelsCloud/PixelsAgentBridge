package com.pixels.agent.platform.storage

import com.pixels.agent.core.StorageEntry
import com.pixels.agent.core.StorageFailure
import com.pixels.agent.core.StorageMetadata
import com.pixels.agent.core.StorageMounts
import java.security.MessageDigest
import java.util.UUID
import org.json.JSONArray
import org.json.JSONObject

/** File-window adapter. Only virtual workspace and persisted SAF grants are visible. */
internal class ManagedStorage(private val mounts: StorageMounts) {
    private fun native(path: String) = JSONObject().put("encoding", "utf8").put("value", path)
    private fun path(value: JSONObject): String {
        if (value.getString("encoding") != "utf8") throw StorageFailure("invalid_path")
        return value.getString("value").also { mounts.resolve(it) }
    }
    private fun component(value: JSONObject): String {
        if (value.getString("encoding") != "utf8") throw StorageFailure("invalid_path")
        return value.getString("value").also { if (!StorageMounts.validName(it)) throw StorageFailure("invalid_path") }
    }
    private fun hash(value: String) = MessageDigest.getInstance("SHA-256").digest(value.toByteArray()).joinToString("") { "%02x".format(it.toInt() and 255) }
    fun identity(path: String, metadata: StorageMetadata): String {
        val id = metadata.identity ?: throw StorageFailure("provider_identity_unavailable")
        return "android:" + hash(path.removePrefix("/").substringBefore('/') + "\u0000" + id)
    }
    private fun attributes(metadata: StorageMetadata) = JSONObject()
        .put("modified_at_unix_ms", metadata.modifiedAtMillis ?: JSONObject.NULL).put("unix_mode", JSONObject.NULL)
    private fun entry(path: String, metadata: StorageMetadata) = JSONObject()
        .put("id", UUID.randomUUID().toString()).put("name", metadata.name).put("path", native(path))
        .put("kind", if (metadata.directory) "directory" else "file")
        .put("size", metadata.size?.toString() ?: JSONObject.NULL)
        .put("modified_at_unix_ms", metadata.modifiedAtMillis ?: JSONObject.NULL)
        .put("hidden", metadata.name.startsWith('.')).put("readonly", metadata.readOnly)
        .put("unix_mode", JSONObject.NULL).put("error", JSONObject.NULL)
    fun listing(path: String): JSONObject {
        val items = mounts.resolve(path).children()
        if (items.size > 20000) throw StorageFailure("directory_limit")
        return JSONObject().put("entries", JSONArray(items.map { entry("$path/${it.name}", it.metadata()) }))
    }
    fun guardParent(path: String, expected: String): StorageEntry {
        val parentPath = path.substringBeforeLast('/', "")
        val parent = mounts.resolve(parentPath)
        val metadata = parent.metadata()
        if (!metadata.directory || identity(parentPath, metadata) != expected)
            throw StorageFailure("destination_parent_changed")
        return parent
    }
    fun boundTarget(path: String, expected: String): StorageEntry {
        val parent = guardParent(path, expected)
        val metadata = parent.metadata()
        if (identity(path.substringBeforeLast('/'), metadata) != expected) throw StorageFailure("destination_parent_changed")
        return parent.child(path.substringAfterLast('/')).bindParent(metadata.identity ?: throw StorageFailure("provider_identity_unavailable"))
    }
    fun source(path: String, constraint: JSONObject, metadata: StorageMetadata) {
        if (metadata.directory || metadata.size?.toString() != constraint.getString("size") ||
            !constraint.isNull("modified_at_unix_ms") && metadata.modifiedAtMillis != constraint.getLong("modified_at_unix_ms"))
            throw StorageFailure("source_changed")
    }
    fun metadataResult(requested: JSONObject, actual: StorageMetadata): JSONObject {
        val warnings = JSONArray()
        for (key in listOf("modified_at_unix_ms", "unix_mode")) {
            if (!requested.isNull(key) && (key == "unix_mode" || requested.getLong(key) != actual.modifiedAtMillis))
                warnings.put(JSONObject().put("attribute", if (key == "unix_mode") "unix_mode" else "modified_time")
                    .put("code", "provider_attribute_unavailable").put("message", "Android storage provider did not apply this attribute"))
        }
        return JSONObject().put("requested", requested).put("actual", attributes(actual)).put("warnings", warnings)
    }
    fun execute(operation: JSONObject, scope: String, checkpoint: () -> Unit): JSONObject {
        val action = operation.getString("action")
        if (action == "locations") return JSONObject().put("result", "locations").put("locations", JSONObject()
            .put("filesystem_scope", scope).put("path_style", "posix").put("home", native("/workspace"))
            .put("roots", JSONArray(mounts.rootNames().map { JSONObject().put("name", if (it == "workspace") "Workspace" else mounts.resolve("/$it").metadata().name).put("path", native("/$it")) })))
        val path = path(operation.getJSONObject(if (action == "create_directory") "parent" else "path"))
        val target = mounts.resolve(path)
        return when (action) {
            "inspect" -> JSONObject().put("result", "entry").put("entry", entry(path, target.metadata()))
            "inspect_path_identity" -> {
                val components = path.removePrefix("/").split('/')
                val ancestors = JSONArray()
                for (n in 1 until components.size) {
                    val parentPath = "/" + components.take(n).joinToString("/")
                    try { val metadata = mounts.resolve(parentPath).metadata(); if (!metadata.directory) throw StorageFailure("not_a_directory"); ancestors.put(identity(parentPath, metadata)) }
                    catch (e: StorageFailure) { if (e.code == "not_found") break else throw e }
                }
                val metadata = try { target.metadata() } catch (e: StorageFailure) { if (e.code == "not_found") null else throw e }
                JSONObject().put("result", "path_identity").put("path", native(path)).put("identity", JSONObject()
                    .put("filesystem_scope", scope).put("object", metadata?.let { identity(path, it) } ?: JSONObject.NULL)
                    .put("kind", metadata?.let { if (it.directory) "directory" else "file" } ?: JSONObject.NULL).put("ancestors", ancestors))
            }
            "inspect_directory_object", "set_directory_metadata" -> {
                val metadata = target.metadata(); if (!metadata.directory) throw StorageFailure("not_a_directory")
                val id = identity(path, metadata)
                if (action == "set_directory_metadata" && id != operation.getString("expected_object")) throw StorageFailure("directory_changed")
                JSONObject().put("result", if (action == "set_directory_metadata") "directory_metadata" else "directory_object")
                    .put("path", native(path)).put("identity", id).put("metadata", if (action == "set_directory_metadata") metadataResult(operation.getJSONObject("metadata"), metadata) else attributes(metadata))
            }
            "prepare_directory" -> {
                val parent = guardParent(path, operation.getString("expected_parent"))
                val child = parent.child(path.substringAfterLast('/'))
                val existing = operation.optString("existing_object").takeUnless { it.isEmpty() || it == "null" }
                if (existing == null) child.mkdir(false, false, checkpoint)
                val metadata = child.metadata()
                if (!metadata.directory || existing != null && identity(path, metadata) != existing) throw StorageFailure("directory_changed")
                JSONObject().put("result", "directory_prepared").put("path", native(path)).put("identity", identity(path, metadata)).put("created", existing == null)
            }
            "create_directory" -> {
                val name = component(operation.getJSONObject("name")); target.child(name).mkdir(false, false, checkpoint)
                JSONObject().put("result", "applied").put("path", native("$path/$name"))
            }
            "rename" -> {
                val name = component(operation.getJSONObject("name")); target.rename(name, checkpoint)
                JSONObject().put("result", "applied").put("path", native(path.substringBeforeLast('/') + "/" + name))
            }
            "delete" -> {
                if (path.removePrefix("/").count { it == '/' } == 0) throw StorageFailure("mount_root_protected")
                var count = 0
                fun remove(value: StorageEntry, depth: Int) {
                    if (++count > 20000 || depth > 128) throw StorageFailure("directory_limit")
                    checkpoint()
                    if (value.metadata().directory && operation.optBoolean("recursive")) value.children().forEach { remove(it, depth + 1) }
                    value.delete(checkpoint)
                }
                remove(target, 0); JSONObject().put("result", "applied").put("path", native(path))
            }
            else -> throw StorageFailure("unsupported_storage_action")
        }
    }
}
