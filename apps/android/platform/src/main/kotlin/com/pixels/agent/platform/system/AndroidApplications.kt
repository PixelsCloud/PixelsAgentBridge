package com.pixels.agent.platform.system

import android.app.KeyguardManager
import android.content.Context
import android.content.Intent
import android.content.pm.ApplicationInfo
import android.os.PowerManager
import android.os.Process
import android.os.UserManager
import com.pixels.agent.core.StorageFailure
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject

/** Android's interactive app UID is represented by the existing desktop_user wire selection. */
internal class AndroidApplications(
    private val context: Context,
    private val canDispatch: () -> Boolean,
    private val fileExporter: (String) -> Pair<android.net.Uri, String>,
) {
    private val packages
        get() = context.packageManager

    internal fun available() =
        context.getSystemService(UserManager::class.java).isUserUnlocked &&
            !context.getSystemService(KeyguardManager::class.java).isKeyguardLocked &&
            context.getSystemService(PowerManager::class.java).isInteractive

    internal fun identity(interactive: Boolean) =
        obj(
            "mode" to if (interactive) "desktop_user" else "service",
            "account_id" to "uid:${Process.myUid()}",
            "account_name" to context.packageName,
            "home" to "/workspace",
            "primary_group" to android.system.Os.getgid(),
            "session_id" to
                if (interactive) "android-user:${Process.myUid() / 100000}:display:0" else null,
            "logon_id" to null,
            "environment_source" to if (interactive) "interactive_session" else "service_process",
        )

    fun contexts(query: JSONObject): JSONObject {
        val entries = mutableListOf<JSONObject>()
        for (interactive in listOf(false, true)) {
            val selection =
                if (interactive && !available()) null
                else if (interactive)
                    obj(
                        "mode" to "desktop_user",
                        "context_ref" to "00000000-0000-0000-0000-000000000000",
                    )
                else obj("mode" to "service")
            val id = identity(interactive)
            entries.add(
                obj(
                    "mode" to id.getString("mode"),
                    "account_name" to context.packageName,
                    "account_id" to id.getString("account_id"),
                    "session_id" to id.opt("session_id"),
                    "identity" to id,
                    "selection" to selection,
                    "unavailable_reason" to
                        if (selection == null) "android_user_locked_or_display_not_interactive"
                        else null,
                )
            )
        }
        val user = query.optionalString("user")
        // The service row is always included, matching the existing inventory contract.
        val selected = entries.filter {
            it.getString("mode") == "service" || user == null || user == context.packageName
        }
        val limit = query.getInt("limit")
        return response(
            obj(
                "type" to "execution_contexts",
                "backend" to "android_app_uid",
                "entries" to array(selected.take(limit)),
            ),
            selected.take(limit).size,
            listOf(
                "desktop_user selects this app UID's unlocked Android activity session; it does not grant another UID, ADB shell or a desktop login."
            ),
            selected.size > limit,
        )
    }

    suspend fun execute(query: JSONObject): JSONObject {
        if (!available()) throw StorageFailure("android_user_locked_or_display_not_interactive")
        return when (query.getString("action")) {
            "list" -> list(query.getJSONObject("request"))
            "execute" -> action(query.getJSONObject("request"))
            else -> throw StorageFailure("android_application_action_unavailable")
        }
    }

    @Suppress("DEPRECATION")
    private fun list(request: JSONObject): JSONObject {
        val running = request.getString("scope") == "running"
        val search = request.optString("search")
        val visible =
            packages
                .getInstalledPackages(0)
                .asSequence()
                .filter { !running || it.packageName == context.packageName }
                .mapNotNull { pkg ->
                    val app = pkg.applicationInfo ?: return@mapNotNull null
                    val label = packages.getApplicationLabel(app).toString().take(256)
                    if (!pkg.packageName.contains(search, true) && !label.contains(search, true))
                        return@mapNotNull null
                    obj(
                        "name" to label,
                        "app_id" to pkg.packageName,
                        "path" to null,
                        "source" to
                            if (running) "android_current_app_process"
                            else "android_package_manager_visible",
                        "instance" to
                            if (running)
                                obj(
                                    "process_id" to Process.myPid(),
                                    "process_identity" to
                                        "${Process.myPid()}:${Process.getStartElapsedRealtime()}",
                                    "account_id" to "uid:${Process.myUid()}",
                                    "session_id" to identity(true).getString("session_id"),
                                )
                            else null,
                        "details" to
                            obj(
                                "version_name" to pkg.versionName?.take(128),
                                "version_code" to pkg.longVersionCode,
                                "enabled" to app.enabled,
                                "system" to (app.flags and ApplicationInfo.FLAG_SYSTEM != 0),
                                "launchable" to
                                    (packages.getLaunchIntentForPackage(pkg.packageName) != null),
                            ),
                    )
                }
                .sortedBy { it.getString("app_id") }
                .toList()
        val selected = mutableListOf<JSONObject>()
        var bytes = 0
        for (entry in visible) {
            val size = entry.toString().toByteArray(Charsets.UTF_8).size
            if (selected.size >= request.getInt("limit") || bytes + size > 22000) break
            selected.add(entry)
            bytes += size
        }
        val truncated = selected.size < visible.size
        val warnings =
            listOf(
                if (running)
                    "Running scope contains this app's current process only; not all running applications."
                else
                    "Installed scope is PackageManager-visible packages, including launcher queries; Android may hide other packages. Narrow search when truncated."
            )
        return response(
            obj(
                "type" to "applications",
                "snapshot" to
                    obj(
                        "type" to "list",
                        "snapshot" to
                            obj(
                                "apps" to array(selected),
                                "truncated" to truncated,
                                "sources" to array(listOf("android_package_manager_visible")),
                                "warnings" to array(warnings),
                                "execution_identity" to identity(true),
                            ),
                    ),
            ),
            selected.size,
            warnings,
            truncated,
        )
    }

    private fun packageId(target: JSONObject): String {
        if (target.getString("kind") != "id")
            throw StorageFailure("android_application_requires_package_id")
        return target.getString("id").also {
            if (
                it.length > 255 ||
                    !it.matches(Regex("[A-Za-z][A-Za-z0-9_]*(\\.[A-Za-z][A-Za-z0-9_]*)+"))
            )
                throw StorageFailure("invalid_package_id")
        }
    }

    private suspend fun action(request: JSONObject): JSONObject {
        val intent =
            when (request.getString("operation")) {
                "launch" -> {
                    if (request.optBoolean("new_instance"))
                        throw StorageFailure("android_new_instance_unavailable")
                    packages.getLaunchIntentForPackage(
                        packageId(request.getJSONObject("application"))
                    ) ?: throw StorageFailure("application_not_visible_or_launchable")
                }
                "open_file" -> {
                    // Installed by the composition root; this callback only exports authorized
                    // mounts.
                    val path = request.getString("path")
                    val exported =
                        fileExporter?.invoke(path)
                            ?: throw StorageFailure("file_export_unavailable")
                    Intent(Intent.ACTION_VIEW)
                        .setDataAndType(exported.first, exported.second)
                        .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                        .apply {
                            clipData = android.content.ClipData.newRawUri("file", exported.first)
                            if (!request.isNull("application"))
                                setPackage(packageId(request.getJSONObject("application")))
                        }
                }
                else -> throw StorageFailure("android_application_action_unavailable")
            }
        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        if (intent.resolveActivity(packages) == null)
            throw StorageFailure("application_handler_not_found")
        return withContext(Dispatchers.Main.immediate) {
            if (!canDispatch()) throw StorageFailure("control_paused_or_disconnected")
            if (!available()) throw StorageFailure("android_user_locked_or_display_not_interactive")
            try {
                context.startActivity(intent)
            } catch (_: android.content.ActivityNotFoundException) {
                throw StorageFailure("application_handler_not_found")
            } catch (_: SecurityException) {
                throw StorageFailure("application_launch_denied")
            } catch (_: Exception) {
                return@withContext obj(
                    "error" to "activity_dispatch_outcome_unconfirmed",
                    "action_started" to true,
                )
            }
            response(
                obj(
                    "type" to "applications",
                    "snapshot" to
                        obj(
                            "type" to "action",
                            "result" to
                                obj(
                                    "request_accepted" to true,
                                    "instance" to null,
                                    "reused_instance" to null,
                                    "execution_identity" to identity(true),
                                    "window_ready" to null,
                                    "notes" to
                                        array(
                                            listOf(
                                                "Android accepted startActivity; background launch policy may defer display. Observe the foreground app with mobile tools.",
                                                "Open-file grants are read-only snapshots; editing a viewer copy does not modify the source.",
                                            )
                                        ),
                                ),
                        ),
                )
            )
        }
    }
}
