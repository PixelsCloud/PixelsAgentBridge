package com.pixels.agent

import android.Manifest
import android.accessibilityservice.AccessibilityServiceInfo
import android.app.Activity
import android.app.NotificationManager
import android.content.ComponentName
import android.content.Intent
import android.content.pm.PackageManager
import android.media.projection.MediaProjectionManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.DocumentsContract
import android.provider.Settings
import android.view.accessibility.AccessibilityManager
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.lifecycleScope
import com.pixels.agent.core.PermissionSetupSequence
import com.pixels.agent.core.SetupPermission
import com.pixels.agent.platform.capture.ProjectionCaptureService
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** Activity-scoped startup preparation; granting permission never connects or resumes control. */
internal class StartupPermissions(
    private val activity: ComponentActivity,
    private val graph: PixelsApplication,
    savedState: Bundle?,
) {
    private val preferences = PermissionSetupPreferences(activity.noBackupFilesDir)
    private val sequence =
        PermissionSetupSequence(
            savedState
                ?.getStringArrayList("permission_attempted")
                .orEmpty()
                .mapNotNull { name ->
                    SetupPermission.entries.find { it.name == name }
                }
                .toSet()
        )
    var pending by
        mutableStateOf(
            savedState?.getString("permission_pending")?.let { name ->
                SetupPermission.entries.find { it.name == name }
            }
        )
        private set

    var busy by mutableStateOf(savedState?.getBoolean("permission_external_pending") ?: false)
        private set

    var missing by mutableStateOf(emptySet<SetupPermission>())
        private set

    var error by mutableStateOf(false)
        private set

    private var workspaceOnly = savedState?.getBoolean("permission_workspace_only") ?: false
    private var includeFiles = savedState?.getBoolean("permission_include_files") ?: false
    private var loaded = false
    private var loading = false
    private var externalPending = savedState?.getBoolean("permission_external_pending") ?: false

    private val notification =
        activity.registerForActivityResult(ActivityResultContracts.RequestPermission()) { finish() }
    private val settings =
        activity.registerForActivityResult(ActivityResultContracts.StartActivityForResult()) {
            finish()
        }
    private val capture =
        activity.registerForActivityResult(ActivityResultContracts.StartActivityForResult()) {
            result ->
            if (result.resultCode == Activity.RESULT_OK && result.data != null) {
                ProjectionCaptureService.start(
                    activity,
                    result.resultCode,
                    requireNotNull(result.data),
                )
            }
            finish()
        }
    private val directory =
        activity.registerForActivityResult(ActivityResultContracts.StartActivityForResult()) {
            result ->
            externalPending = false
            activity.lifecycleScope.launch {
                error =
                    try {
                        withContext(Dispatchers.IO) {
                            val data = result.data
                            if (result.resultCode == Activity.RESULT_OK && data?.data != null)
                                graph.fileStorage.grant(requireNotNull(data.data), data.flags)
                            else {
                                preferences.useWorkspaceOnly()
                            }
                        }
                        if (result.resultCode != Activity.RESULT_OK || result.data?.data == null) {
                            workspaceOnly = true
                            includeFiles = false
                        }
                        false
                    } catch (cancelled: kotlinx.coroutines.CancellationException) {
                        throw cancelled
                    } catch (_: Exception) {
                        true
                    }
                finish()
            }
        }

    fun refresh() {
        if (!loaded) {
            if (loading) return
            loading = true
            activity.lifecycleScope.launch {
                workspaceOnly =
                    withContext(Dispatchers.IO) {
                        runCatching { preferences.workspaceOnly() }.getOrDefault(false)
                    }
                loaded = true
                loading = false
                refresh()
            }
            return
        }
        missing = SetupPermission.entries.toSet() - ready()
        if (!busy && (pending == null || pending in ready())) advance()
    }

    fun retry() {
        retry(includeFiles = true)
    }

    fun authorizeMissing() {
        retry(includeFiles = false)
    }

    private fun retry(includeFiles: Boolean) {
        if (busy) return
        this.includeFiles = includeFiles
        sequence.retry()
        advance()
    }

    fun open(permission: SetupPermission) {
        if (busy) return
        // A row manages one permission; returning must not start the startup sequence.
        dismiss()
        launch(permission, manage = true)
    }

    fun dismiss() {
        if (busy) return
        pending = null
        includeFiles = false
        missing = SetupPermission.entries.toSet() - ready()
        // Postpone the remaining steps for this activity session; the home retry stays available.
        while (sequence.next(emptySet()) != null) {}
    }

    fun notificationSettings() {
        if (busy || pending != SetupPermission.NOTIFICATIONS) return
        busy = true
        externalPending = true
        try {
            settings.launch(
                Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS)
                    .putExtra(Settings.EXTRA_APP_PACKAGE, activity.packageName)
            )
        } catch (_: Exception) {
            error = true
            finish()
        }
    }

    fun request() {
        val step = pending ?: return
        launch(step)
    }

    private fun launch(step: SetupPermission, manage: Boolean = false) {
        if (busy) return
        busy = true
        externalPending = true
        error = false
        try {
            when (step) {
                SetupPermission.NOTIFICATIONS -> {
                    if (
                        !manage &&
                            Build.VERSION.SDK_INT >= 33 &&
                            activity.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) !=
                                PackageManager.PERMISSION_GRANTED
                    )
                        notification.launch(Manifest.permission.POST_NOTIFICATIONS)
                    else
                        settings.launch(
                            Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS)
                                .putExtra(Settings.EXTRA_APP_PACKAGE, activity.packageName)
                        )
                }
                SetupPermission.ACCESSIBILITY ->
                    settings.launch(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS))
                SetupPermission.NOTIFICATION_ACCESS ->
                    settings.launch(Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS))
                SetupPermission.INSTALL_APPS ->
                    settings.launch(
                        Intent(
                            Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
                            Uri.parse("package:${activity.packageName}"),
                        )
                    )
                SetupPermission.FILES ->
                    directory.launch(
                        Intent(Intent.ACTION_OPEN_DOCUMENT_TREE)
                            .addFlags(
                                Intent.FLAG_GRANT_READ_URI_PERMISSION or
                                    Intent.FLAG_GRANT_WRITE_URI_PERMISSION or
                                    Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION or
                                    Intent.FLAG_GRANT_PREFIX_URI_PERMISSION
                            )
                    )
                SetupPermission.SCREEN_CAPTURE ->
                    capture.launch(
                        activity
                            .getSystemService(MediaProjectionManager::class.java)
                            .createScreenCaptureIntent()
                    )
            }
        } catch (_: Exception) {
            error = true
            finish()
        }
    }

    fun workspaceOnly() {
        if (busy || pending != SetupPermission.FILES) return
        busy = true
        activity.lifecycleScope.launch {
            error =
                try {
                    withContext(Dispatchers.IO) { preferences.useWorkspaceOnly() }
                    workspaceOnly = true
                    includeFiles = false
                    false
                } catch (cancelled: kotlinx.coroutines.CancellationException) {
                    throw cancelled
                } catch (_: Exception) {
                    true
                }
            finish()
        }
    }

    fun save(state: Bundle) {
        state.putStringArrayList(
            "permission_attempted",
            ArrayList(sequence.snapshot().map { it.name }),
        )
        state.putString("permission_pending", pending?.name)
        state.putBoolean("permission_external_pending", externalPending)
        state.putBoolean("permission_workspace_only", workspaceOnly)
        state.putBoolean("permission_include_files", includeFiles)
    }

    private fun finish() {
        externalPending = false
        pending = null
        busy = false
        refresh()
    }

    private fun advance() {
        val ready = ready()
        missing = SetupPermission.entries.toSet() - ready
        pending = sequence.next(ready)
    }

    private fun ready(): Set<SetupPermission> = buildSet {
        val manager = activity.getSystemService(NotificationManager::class.java)
        if (
            manager.areNotificationsEnabled() &&
                (Build.VERSION.SDK_INT < 33 ||
                    activity.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) ==
                        PackageManager.PERMISSION_GRANTED)
        )
            add(SetupPermission.NOTIFICATIONS)
        val service =
            ComponentName(
                activity.packageName,
                "com.pixels.agent.platform.accessibility.PixelsAccessibilityService",
            )
        if (
            activity
                .getSystemService(AccessibilityManager::class.java)
                .getEnabledAccessibilityServiceList(AccessibilityServiceInfo.FEEDBACK_ALL_MASK)
                .any { ComponentName.unflattenFromString(it.id) == service }
        )
            add(SetupPermission.ACCESSIBILITY)
        if (activity.packageManager.canRequestPackageInstalls()) add(SetupPermission.INSTALL_APPS)
        if (com.pixels.agent.platform.notifications.AndroidNotifications.hasAccess(activity))
            add(SetupPermission.NOTIFICATION_ACCESS)
        if (
            (workspaceOnly && !includeFiles) ||
                activity.contentResolver.persistedUriPermissions.any {
                    it.isReadPermission && DocumentsContract.isTreeUri(it.uri)
                }
        )
            add(SetupPermission.FILES)
        if (Build.VERSION.SDK_INT >= 30 || graph.projectionCapture.active.value)
            add(SetupPermission.SCREEN_CAPTURE)
    }
}
