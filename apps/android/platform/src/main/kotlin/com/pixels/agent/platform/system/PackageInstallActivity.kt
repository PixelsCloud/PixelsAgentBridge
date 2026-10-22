package com.pixels.agent.platform.system

import android.app.Activity
import android.content.ClipData
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Bundle
import android.widget.ProgressBar
import java.io.File
import java.security.MessageDigest
import java.util.UUID

/** Normal system package confirmation for OEMs whose observer callbacks are unavailable. */
@Suppress("DEPRECATION")
class PackageInstallActivity : Activity() {
    private var change: String? = null
    private var launched = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(ProgressBar(this))
        val id =
            intent.getStringExtra("change_ref")?.let {
                runCatching { UUID.fromString(it).toString() }.getOrNull()
            }
        if (id == null) {
            finishAndRemoveTask()
            return
        }
        change = id
        PackageChangeStore(this).use { store ->
            val fact = store.read(id).getJSONObject("result")
            if (
                fact.optString("installer_mode") != "activity" ||
                    fact.getString("state") !in listOf("submitted", "waiting_user_action")
            ) {
                finishAndRemoveTask()
                return
            }
            if (savedInstanceState?.getBoolean("installer_launched") == true) {
                launched = true
                return
            }
            if (fact.optBoolean("activity_dispatched")) {
                store.transition(id, "unconfirmed", "installer_activity_restarted_without_result")
                finishAndRemoveTask()
                return
            }
            try {
                AndroidPackageChanges.ensureDispatch(this)
                if (fact.optBoolean("uninstall")) {
                    val current =
                        packageManager.getPackageInfo(
                            fact.getString("package_id"),
                            PackageManager.GET_SIGNING_CERTIFICATES,
                        )
                    val currentSigners =
                        current.signingInfo
                            ?.apkContentsSigners
                            ?.map {
                                MessageDigest.getInstance("SHA-256")
                                    .digest(it.toByteArray())
                                    .joinToString("") { byte -> "%02x".format(byte) }
                            }
                            ?.toSet()
                            .orEmpty()
                    val expected = fact.getJSONArray("expected_signer_sha256")
                    if (
                        current.longVersionCode != fact.getLong("expected_version_code") ||
                            currentSigners !=
                                (0 until expected.length()).map { expected.getString(it) }.toSet()
                    ) {
                        store.transition(
                            id,
                            "failed",
                            "installed_package_changed_before_confirmation",
                        )
                        finishAndRemoveTask()
                        return
                    }
                }
                if (!store.claimActivity(id, taskId)) {
                    finishAndRemoveTask()
                    return
                }
                val install =
                    if (fact.optBoolean("uninstall")) {
                        Intent(Intent.ACTION_UNINSTALL_PACKAGE)
                            .setData(Uri.fromParts("package", fact.getString("package_id"), null))
                            .putExtra(Intent.EXTRA_RETURN_RESULT, true)
                    } else {
                        val uri = Uri.parse(fact.getString("apk_uri"))
                        check(
                            uri.scheme == "content" &&
                                uri.authority == packageName + ".remote-files" &&
                                uri.pathSegments == listOf(id, "base.apk")
                        )
                        Intent(Intent.ACTION_INSTALL_PACKAGE)
                            .setDataAndType(uri, "application/vnd.android.package-archive")
                            .putExtra(Intent.EXTRA_RETURN_RESULT, true)
                            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                            .apply { clipData = ClipData.newRawUri("APK", uri) }
                    }
                launched = true
                startActivityForResult(install, 1)
            } catch (_: Exception) {
                store.transition(
                    id,
                    if (launched) "unconfirmed" else "failed",
                    "system_installer_activity_unavailable",
                )
                if (!launched && !fact.optBoolean("uninstall")) cleanup(id)
                finishAndRemoveTask()
            }
        }
    }

    override fun onSaveInstanceState(outState: Bundle) {
        outState.putBoolean("installer_launched", launched)
        super.onSaveInstanceState(outState)
    }

    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != 1) return
        val id = change ?: return
        PackageChangeStore(this).use { store ->
            val fact = store.read(id).getJSONObject("result")
            val removing = fact.optBoolean("uninstall")
            val expected = if (removing) fact else fact.getJSONObject("package")
            val installed =
                try {
                    packageManager.getPackageInfo(
                        fact.getString("package_id"),
                        PackageManager.GET_SIGNING_CERTIFICATES,
                    )
                } catch (_: PackageManager.NameNotFoundException) {
                    null
                }
            val signers =
                installed
                    ?.signingInfo
                    ?.apkContentsSigners
                    ?.map { signer ->
                        MessageDigest.getInstance("SHA-256")
                            .digest(signer.toByteArray())
                            .joinToString("") { "%02x".format(it) }
                    }
                    ?.toSet()
                    .orEmpty()
            val expectedSigners =
                expected
                    .getJSONArray(if (removing) "expected_signer_sha256" else "signer_sha256")
                    .let { values ->
                        (0 until values.length()).map { values.getString(it) }.toSet()
                    }
            val matches =
                installed?.longVersionCode ==
                    expected.getLong(if (removing) "expected_version_code" else "version_code") &&
                    signers == expectedSigners
            when {
                resultCode == RESULT_OK && (if (removing) installed == null else matches) ->
                    store.transition(id, "succeeded")
                resultCode == RESULT_CANCELED && (if (removing) matches else !matches) ->
                    store.transition(id, "cancelled", "system_action_aborted")
                else ->
                    store.transition(
                        id,
                        "unconfirmed",
                        "installer_result_not_verified",
                        note =
                            "System package result: $resultCode; removal: $removing; installed: ${installed != null}; expected package match: $matches",
                    )
            }
            if (
                !removing &&
                    store.read(id).getJSONObject("result").getString("state") in
                        listOf("succeeded", "cancelled", "failed")
            )
                cleanup(id)
        }
        finishAndRemoveTask()
    }

    private fun cleanup(id: String) {
        val uri =
            Uri.Builder()
                .scheme("content")
                .authority(packageName + ".remote-files")
                .appendPath(id)
                .appendPath("base.apk")
                .build()
        revokeUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION)
        val directory = File(File(cacheDir, "remote-open"), id)
        File(directory, "base.apk").delete()
        directory.delete()
    }
}
