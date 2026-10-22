package com.pixels.agent.platform.system

import android.app.ActivityOptions
import android.app.KeyguardManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.ApplicationInfo
import android.content.pm.PackageInfo
import android.content.pm.PackageInstaller
import android.content.pm.PackageManager
import android.content.pm.VersionedPackage
import android.net.Uri
import android.os.Build
import android.os.PowerManager
import android.os.UserManager
import com.pixels.agent.core.StorageFailure
import java.io.File
import java.security.MessageDigest
import java.util.UUID
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.json.JSONObject

/** A single coordinator stages immutable APKs and delegates all changes to Android. */
internal class AndroidPackageChanges(
    private val context: Context,
    canDispatch: () -> Boolean,
    private val snapshot: (String) -> File,
) {
    init {
        allowed = canDispatch
    }

    private val packages
        get() = context.packageManager

    private val installer
        get() = packages.packageInstaller

    suspend fun execute(request: JSONObject, owner: String, id: String): JSONObject =
        gate.withLock {
            PackageChangeStore(context).use { store ->
                when (request.getString("operation")) {
                    "inspect" ->
                        withSnapshot(request.getString("path")) { file ->
                            val metadata = inspect(file)
                            result(null, "inspected", metadata.getString("package_id"), metadata)
                        }
                    "status",
                    "resume" -> {
                        val change = request.getString("change_ref")
                        val row = store.read(change, owner)
                        val fact = row.getJSONObject("result")
                        if (
                            fact.getString("state") == "waiting_user_action" &&
                                fact.isNull("package") &&
                                fact.optString("installer_mode") != "activity" &&
                                requiresUninstallerActivity(fact.getString("package_id"))
                        ) {
                            // Older MIUI requests may never receive their final observer result.
                            // Retire only our confirmation handle; this does not cancel the OS
                            // operation or assert that the original uninstall succeeded.
                            PackageConfirmations.release(context, change)
                            store.transition(
                                change,
                                "unconfirmed",
                                "legacy_miui_uninstall_result_unavailable",
                                note =
                                    "The original system operation may still finish. Check the installed package before requesting another change.",
                            )
                            if (request.getString("operation") == "resume")
                                throw StorageFailure("app_change_confirmation_unavailable")
                            return@withLock store.read(change, owner).getJSONObject("result")
                        }
                        if (request.getString("operation") == "resume") {
                            if (fact.optString("installer_mode") == "activity") {
                                if (fact.getString("state") != "waiting_user_action")
                                    throw StorageFailure("app_change_confirmation_unavailable")
                                ensureDispatch(context)
                                val task =
                                    context
                                        .getSystemService(android.app.ActivityManager::class.java)
                                        .appTasks
                                        .firstOrNull {
                                            (if (Build.VERSION.SDK_INT >= 29) it.taskInfo.taskId
                                            else it.taskInfo.id) == fact.optInt("activity_task", -1)
                                        }
                                if (task == null) {
                                    store.transition(
                                        change,
                                        "unconfirmed",
                                        "system_confirmation_expired",
                                    )
                                    throw StorageFailure("app_change_confirmation_expired")
                                }
                                task.moveToFront()
                                return@withLock fact.put(
                                    "can_request_installs",
                                    packages.canRequestPackageInstalls(),
                                )
                            }
                            if (
                                fact.getString("state") != "waiting_user_action" ||
                                    row.isNull("confirmation")
                            )
                                throw StorageFailure("app_change_confirmation_unavailable")
                            withContext(Dispatchers.Main.immediate) {
                                val pending =
                                    confirmationPending(
                                        context,
                                        change,
                                        PackageConfirmations.decode(row.getString("confirmation")),
                                        false,
                                    )
                                if (pending == null) {
                                    store.transition(
                                        change,
                                        "unconfirmed",
                                        "system_confirmation_expired",
                                    )
                                    throw StorageFailure("app_change_confirmation_expired")
                                }
                                dispatchConfirmation(context, pending)
                            }
                        } else if (
                            fact.getString("state") == "submitted" &&
                                fact.optString("generation") != generation
                        ) {
                            // A crash between journaling and OS dispatch cannot justify repeating
                            // it.
                            if (!row.isNull("session")) {
                                val session = row.getInt("session")
                                val info = installer.getSessionInfo(session)
                                if (info != null && !info.isSealed)
                                    installer.abandonSession(session)
                            }
                            store.transition(
                                change,
                                "unconfirmed",
                                "process_restarted_query_os_result",
                            )
                        }
                        store.read(change, owner).getJSONObject("result")
                    }
                    "install",
                    "uninstall" -> {
                        // Native acceptance already enforces owner/request digest. This second
                        // guard
                        // prevents an accidental SDK redispatch from repeating a durable change.
                        val old =
                            try {
                                store.read(id, owner)
                            } catch (_: StorageFailure) {
                                null
                            }
                        if (old != null) old.getJSONObject("result")
                        else if (request.getString("operation") == "install")
                            install(store, request, owner, id)
                        else uninstall(store, request, owner, id)
                    }
                    else -> throw StorageFailure("android_package_action_unavailable")
                }.put("can_request_installs", packages.canRequestPackageInstalls())
            }
        }

    private inline fun <T> withSnapshot(path: String, block: (File) -> T): T {
        if (!path.endsWith(".apk", true)) throw StorageFailure("apk_extension_required")
        val file = snapshot(path)
        try {
            return block(file)
        } finally {
            file.delete()
            file.parentFile?.delete()
        }
    }

    @Suppress("DEPRECATION")
    private fun installed(id: String): PackageInfo? =
        try {
            packages.getPackageInfo(id, PackageManager.GET_SIGNING_CERTIFICATES)
        } catch (_: PackageManager.NameNotFoundException) {
            null
        }

    private fun digest(bytes: ByteArray) =
        MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }

    @Suppress("DEPRECATION")
    private fun archive(file: File): PackageInfo =
        packages.getPackageArchiveInfo(file.path, PackageManager.GET_SIGNING_CERTIFICATES)
            ?: throw StorageFailure("invalid_apk")

    private fun inspect(file: File): JSONObject {
        if (file.length() !in 1..(32L * 1024 * 1024)) throw StorageFailure("apk_size_limit")
        val pkg = archive(file)
        if (!pkg.splitNames.isNullOrEmpty()) throw StorageFailure("split_apk_unsupported")
        val signers =
            pkg.signingInfo?.apkContentsSigners?.map { digest(it.toByteArray()) }.orEmpty()
        if (signers.isEmpty() || signers.size > 16) throw StorageFailure("apk_signers_unavailable")
        val hash = MessageDigest.getInstance("SHA-256")
        file.inputStream().use { input ->
            val buffer = ByteArray(65536)
            while (true) {
                val count = input.read(buffer)
                if (count < 0) break
                hash.update(buffer, 0, count)
            }
        }
        return obj(
            "package_id" to pkg.packageName,
            "version_name" to pkg.versionName?.take(128),
            "version_code" to pkg.longVersionCode,
            "sha256" to hash.digest().joinToString("") { "%02x".format(it) },
            "signer_sha256" to array(signers),
            "installed_version_code" to installed(pkg.packageName)?.longVersionCode,
        )
    }

    private fun protect(id: String) {
        if (id == context.packageName || id == "com.pixels.agent" || id == "com.pixels.agent.debug")
            throw StorageFailure("pixels_package_protected")
        val flags = installed(id)?.applicationInfo?.flags ?: 0
        if (flags and (ApplicationInfo.FLAG_SYSTEM or ApplicationInfo.FLAG_UPDATED_SYSTEM_APP) != 0)
            throw StorageFailure("system_package_protected")
    }

    private fun result(id: String?, state: String, pkg: String, metadata: JSONObject? = null) =
        obj(
            "change_ref" to id,
            "state" to state,
            "package_id" to pkg,
            "package" to metadata,
            "error" to null,
            "can_request_installs" to packages.canRequestPackageInstalls(),
            "generation" to generation,
            "notes" to
                array(
                    listOf(
                        "Dispatch completion is not package-change success. Poll pab_get_app_change; do not replay the original request.",
                        "System confirmation and OEM security/account requirements apply. APK metadata is not a trust endorsement.",
                    )
                ),
        )

    private suspend fun install(
        store: PackageChangeStore,
        request: JSONObject,
        owner: String,
        id: String,
    ): JSONObject {
        val pkg = request.getString("package_id")
        protect(pkg)
        if (!packages.canRequestPackageInstalls())
            throw StorageFailure("install_source_permission_required")
        return withSnapshot(request.getString("path")) { file ->
            val metadata = inspect(file)
            if (metadata.getString("package_id") != pkg)
                throw StorageFailure("apk_package_mismatch")
            if (metadata.getString("sha256") != request.getString("sha256"))
                throw StorageFailure("apk_hash_mismatch")
            val previous = installed(pkg)
            if (previous != null) {
                if (metadata.getLong("version_code") <= previous.longVersionCode)
                    throw StorageFailure("apk_version_not_newer")
                val candidate =
                    archive(file).signingInfo ?: throw StorageFailure("apk_signers_unavailable")
                val accepted =
                    (if (candidate.hasMultipleSigners()) candidate.apkContentsSigners
                        else candidate.signingCertificateHistory)
                        .map { digest(it.toByteArray()) }
                        .toSet()
                val current =
                    previous.signingInfo
                        ?.apkContentsSigners
                        ?.map { digest(it.toByteArray()) }
                        .orEmpty()
                if (current.isEmpty() || !accepted.containsAll(current))
                    throw StorageFailure("apk_update_signer_mismatch")
            }
            ensureDispatch(context)
            if (requiresInstallerActivity())
                return@withSnapshot installWithActivity(store, owner, id, metadata, file)
            store.create(id, owner, result(id, "submitted", pkg, metadata))
            var sessionId: Int? = null
            var committing = false
            try {
                val params =
                    PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL)
                        .apply {
                            setAppPackageName(pkg)
                            setSize(file.length())
                            if (Build.VERSION.SDK_INT >= 31)
                                setRequireUserAction(
                                    PackageInstaller.SessionParams.USER_ACTION_REQUIRED
                                )
                        }
                sessionId = installer.createSession(params)
                store.session(id, sessionId)
                installer.openSession(sessionId).use { session ->
                    session.openWrite("base.apk", 0, file.length()).use { output ->
                        file.inputStream().use { it.copyTo(output, 65536) }
                        session.fsync(output)
                    }
                    ensureDispatch(context)
                    // Persist before the irreversible boundary. Failure after this point is
                    // unknown.
                    committing = true
                    session.commit(callback(store, id).intentSender)
                }
            } catch (_: Exception) {
                if (!committing && sessionId != null)
                    try {
                        installer.abandonSession(sessionId)
                    } catch (_: Exception) {}
                store.transition(
                    id,
                    if (committing) "unconfirmed" else "failed",
                    if (committing) "install_dispatch_unconfirmed" else "install_staging_failed",
                )
            }
            store.read(id, owner).getJSONObject("result")
        }
    }

    @Suppress("DEPRECATION")
    private fun requiresInstallerActivity(): Boolean {
        // The installed MIUI handler rejects CONFIRM_INSTALL sessions on API <= 34.
        // Its ordinary APK activity still performs the system permission/signature checks.
        if (Build.VERSION.SDK_INT > 34) return false
        val intent =
            Intent(Intent.ACTION_INSTALL_PACKAGE)
                .setDataAndType(
                    Uri.parse("content://${context.packageName}.remote-files/package.apk"),
                    "application/vnd.android.package-archive",
                )
        return intent.resolveActivity(packages)?.packageName == "com.miui.packageinstaller"
    }

    private fun installWithActivity(
        store: PackageChangeStore,
        owner: String,
        id: String,
        metadata: JSONObject,
        file: File,
    ): JSONObject {
        val uri =
            Uri.Builder()
                .scheme("content")
                .authority(context.packageName + ".remote-files")
                .appendPath(id)
                .appendPath("base.apk")
                .build()
        val value =
            result(id, "submitted", metadata.getString("package_id"), metadata)
                .put("installer_mode", "activity")
                .put("apk_uri", uri.toString())
        value
            .getJSONArray("notes")
            .put(
                "Compatibility APK installer activity: success requires an OS result and matching installed version/signers; no automatic redispatch."
            )
        store.create(id, owner, value)
        val directory = File(File(context.cacheDir, "remote-open"), id)
        var dispatching = false
        var ownsDirectory = false
        try {
            if (directory.exists() || !directory.mkdir())
                throw StorageFailure("apk_snapshot_unavailable")
            ownsDirectory = true
            if (!file.renameTo(File(directory, "base.apk")))
                throw StorageFailure("apk_snapshot_unavailable")
            ensureDispatch(context)
            dispatching = true
            context.startActivity(
                Intent(context, PackageInstallActivity::class.java)
                    .putExtra("change_ref", id)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            )
        } catch (_: Exception) {
            store.transition(
                id,
                if (dispatching) "unconfirmed" else "failed",
                "installer_activity_dispatch_unconfirmed",
            )
            if (!dispatching && ownsDirectory) {
                File(directory, "base.apk").delete()
                directory.delete()
            }
        }
        return store.read(id, owner).getJSONObject("result")
    }

    private fun uninstall(
        store: PackageChangeStore,
        request: JSONObject,
        owner: String,
        id: String,
    ): JSONObject {
        val pkg = request.getString("package_id")
        protect(pkg)
        val previous =
            installed(pkg) ?: throw StorageFailure("application_not_visible_or_installed")
        if (previous.longVersionCode != request.getLong("version_code"))
            throw StorageFailure("installed_version_mismatch")
        ensureDispatch(context)
        if (requiresUninstallerActivity(pkg)) {
            val signers =
                previous.signingInfo?.apkContentsSigners?.map { digest(it.toByteArray()) }.orEmpty()
            if (signers.isEmpty()) throw StorageFailure("apk_signers_unavailable")
            store.create(
                id,
                owner,
                result(id, "submitted", pkg)
                    .put("installer_mode", "activity")
                    .put("uninstall", true)
                    .put("expected_version_code", previous.longVersionCode)
                    .put("expected_signer_sha256", array(signers)),
            )
            try {
                context.startActivity(
                    Intent(context, PackageInstallActivity::class.java)
                        .putExtra("change_ref", id)
                        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                )
            } catch (_: Exception) {
                store.transition(id, "unconfirmed", "uninstaller_activity_dispatch_unconfirmed")
            }
            return store.read(id, owner).getJSONObject("result")
        }
        store.create(id, owner, result(id, "submitted", pkg))
        try {
            installer.uninstall(
                VersionedPackage(pkg, previous.longVersionCode),
                callback(store, id).intentSender,
            )
        } catch (_: SecurityException) {
            store.transition(id, "failed", "uninstall_denied")
        } catch (_: Exception) {
            store.transition(id, "unconfirmed", "uninstall_dispatch_unconfirmed")
        }
        return store.read(id, owner).getJSONObject("result")
    }

    @Suppress("DEPRECATION")
    private fun requiresUninstallerActivity(packageId: String): Boolean {
        // This MIUI handler can omit the final PackageInstaller observer callback.
        // Its public uninstall activity supports EXTRA_RETURN_RESULT instead.
        if (Build.VERSION.SDK_INT > 34) return false
        return Intent(Intent.ACTION_UNINSTALL_PACKAGE)
            .setData(Uri.fromParts("package", packageId, null))
            .resolveActivity(packages)
            ?.packageName == "com.miui.packageinstaller"
    }

    private fun callback(store: PackageChangeStore, id: String): PendingIntent {
        val nonce = store.read(id).getString("nonce")
        val intent =
            Intent(context, PackageStatusReceiver::class.java)
                .setAction(context.packageName + ".PACKAGE_RESULT")
                .setData(Uri.parse("pixels-package://result/$id/$nonce"))
        val flags =
            PendingIntent.FLAG_UPDATE_CURRENT or
                if (Build.VERSION.SDK_INT >= 31) PendingIntent.FLAG_MUTABLE else 0
        return PendingIntent.getBroadcast(context, 0, intent, flags)
    }

    companion object {
        private val gate = Mutex()
        private val generation = UUID.randomUUID().toString()
        private var allowed: () -> Boolean = { false }

        internal fun ensureDispatch(context: Context) {
            if (!allowed()) throw StorageFailure("control_paused_or_disconnected")
            if (
                !context.getSystemService(UserManager::class.java).isUserUnlocked ||
                    context.getSystemService(KeyguardManager::class.java).isKeyguardLocked ||
                    !context.getSystemService(PowerManager::class.java).isInteractive
            )
                throw StorageFailure("android_user_locked_or_display_not_interactive")
        }

        @Suppress("DEPRECATION")
        private fun dispatchConfirmation(context: Context, pending: PendingIntent) {
            ensureDispatch(context)
            val options =
                if (Build.VERSION.SDK_INT >= 34)
                    ActivityOptions.makeBasic()
                        .apply {
                            setPendingIntentBackgroundActivityStartMode(
                                if (Build.VERSION.SDK_INT >= 36)
                                    ActivityOptions.MODE_BACKGROUND_ACTIVITY_START_ALLOW_ALWAYS
                                else ActivityOptions.MODE_BACKGROUND_ACTIVITY_START_ALLOWED
                            )
                        }
                        .toBundle()
                else null
            // Explicit remote control may operate while another app is foreground. This opts
            // into our existing activity-start privileges; Android still enforces its policy.
            pending.send(context, 0, null, null, null, null, options)
        }

        @Suppress("DEPRECATION")
        private fun confirmationPending(
            context: Context,
            id: String,
            intent: Intent,
            create: Boolean,
        ): PendingIntent? {
            // Uninstall confirmations contain a Binder callback. Intent.toUri loses it;
            // Android retains the complete intent inside this activity PendingIntent.
            // The persisted URI is used only for filter lookup, never reconstructed dispatch.
            if (create && intent.component == null) {
                // Resolve before adding our unique category. System confirmation intents may
                // be implicit; an extra category would otherwise prevent their filters matching.
                intent.component =
                    intent.resolveActivity(context.packageManager)
                        ?: throw StorageFailure("system_confirmation_handler_unavailable")
            }
            intent
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                .addCategory(context.packageName + ".package-change." + id)
            val options =
                if (create && Build.VERSION.SDK_INT >= 35)
                    ActivityOptions.makeBasic()
                        .apply {
                            setPendingIntentCreatorBackgroundActivityStartMode(
                                if (Build.VERSION.SDK_INT >= 36)
                                    ActivityOptions.MODE_BACKGROUND_ACTIVITY_START_ALLOW_ALWAYS
                                else ActivityOptions.MODE_BACKGROUND_ACTIVITY_START_ALLOWED
                            )
                        }
                        .toBundle()
                else null
            return PendingIntent.getActivity(
                context,
                0,
                intent,
                PendingIntent.FLAG_IMMUTABLE or
                    if (create) PendingIntent.FLAG_UPDATE_CURRENT else PendingIntent.FLAG_NO_CREATE,
                options,
            )
        }

        @Suppress("DEPRECATION")
        fun receive(context: Context, intent: Intent) {
            val uri = intent.data ?: return
            val parts = uri.pathSegments
            if (uri.scheme != "pixels-package" || uri.host != "result" || parts.size != 2) return
            PackageChangeStore(context).use { store ->
                val row =
                    try {
                        store.read(parts[0])
                    } catch (_: StorageFailure) {
                        return
                    }
                if (row.getString("nonce") != parts[1]) return
                when (intent.getIntExtra(PackageInstaller.EXTRA_STATUS, Int.MIN_VALUE)) {
                    PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                        val confirmation = intent.getParcelableExtra<Intent>(Intent.EXTRA_INTENT)
                        if (confirmation == null)
                            store.transition(parts[0], "unconfirmed", "missing_system_confirmation")
                        else {
                            val pending =
                                try {
                                    confirmationPending(context, parts[0], confirmation, true)
                                } catch (_: Exception) {
                                    store.transition(
                                        parts[0],
                                        "unconfirmed",
                                        "system_confirmation_handler_unavailable",
                                    )
                                    return
                                }
                            store.transition(
                                parts[0],
                                "waiting_user_action",
                                confirmation = PackageConfirmations.encode(confirmation),
                            )
                            if (
                                store.read(parts[0]).getJSONObject("result").getString("state") ==
                                    "waiting_user_action"
                            ) {
                                try {
                                    if (pending != null) {
                                        runCatching {
                                            PackageConfirmations.retain(context, parts[0], pending)
                                        }
                                        dispatchConfirmation(context, pending)
                                    }
                                } catch (_: Exception) {
                                    /* Resume explicitly after unlock/unpause. */
                                }
                            }
                        }
                    }
                    PackageInstaller.STATUS_SUCCESS -> store.transition(parts[0], "succeeded")
                    PackageInstaller.STATUS_FAILURE_ABORTED ->
                        store.transition(
                            parts[0],
                            "cancelled",
                            "system_action_aborted",
                            note = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE),
                        )
                    PackageInstaller.STATUS_FAILURE,
                    PackageInstaller.STATUS_FAILURE_BLOCKED,
                    PackageInstaller.STATUS_FAILURE_CONFLICT,
                    PackageInstaller.STATUS_FAILURE_INCOMPATIBLE,
                    PackageInstaller.STATUS_FAILURE_INVALID,
                    PackageInstaller.STATUS_FAILURE_STORAGE,
                    PackageInstaller.STATUS_FAILURE_TIMEOUT ->
                        store.transition(
                            parts[0],
                            "failed",
                            "package_installer_status_${intent.getIntExtra(PackageInstaller.EXTRA_STATUS, 1)}",
                        )
                    else ->
                        store.transition(
                            parts[0],
                            "unconfirmed",
                            "unknown_package_installer_status",
                        )
                }
                if (
                    store.read(parts[0]).getJSONObject("result").getString("state") in
                        listOf("succeeded", "cancelled", "failed")
                ) {
                    PackageConfirmations.release(context, parts[0])
                }
            }
        }
    }
}
