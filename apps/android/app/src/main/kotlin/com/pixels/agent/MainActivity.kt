package com.pixels.agent

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.luminance
import androidx.core.view.WindowCompat
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.lifecycleScope
import com.pixels.agent.core.SetupPermission
import com.pixels.agent.platform.capture.ProjectionCaptureService
import com.pixels.agent.ui.PixelsTheme
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

class MainActivity : ComponentActivity() {
    private lateinit var startupPermissions: StartupPermissions
    private var fileGrantError by mutableStateOf(false)
    private val graph
        get() = application as PixelsApplication

    private val notificationConsent =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) {
            ConnectionService.start(this)
        }
    private val captureConsent =
        registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
            val data = result.data
            if (result.resultCode == Activity.RESULT_OK && data != null) {
                ProjectionCaptureService.start(this, result.resultCode, data)
            }
        }

    private val directoryConsent =
        registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
            val data = result.data
            val uri = data?.data
            if (result.resultCode == Activity.RESULT_OK && uri != null) {
                updateFileGrant { graph.fileStorage.grant(uri, data.flags) }
            }
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        if (Build.VERSION.SDK_INT >= 29) window.isNavigationBarContrastEnforced = false
        startupPermissions = StartupPermissions(this, graph, savedInstanceState)
        val factory =
            object : ViewModelProvider.Factory {
                @Suppress("UNCHECKED_CAST")
                override fun <T : ViewModel> create(modelClass: Class<T>): T {
                    if (modelClass == ProductViewModel::class.java) {
                        return ProductViewModel(
                            graph.productPreferences,
                            ActivityHistoryReader(noBackupFilesDir, graph.nativeBridge),
                            graph.connection,
                        )
                            as T
                    }
                    require(modelClass == DeviceViewModel::class.java)
                    return DeviceViewModel(
                        graph.automationBackend.available,
                        graph.dispatcher,
                        { graph.nativeBridge.probe() },
                        graph.screenCapture,
                        graph.connection,
                        graph.controlIntent,
                        graph.environment.state,
                        graph.projectionCapture.active,
                    )
                        as T
                }
            }
        val viewModel = ViewModelProvider(this, factory)[DeviceViewModel::class.java]
        val product = ViewModelProvider(this, factory)[ProductViewModel::class.java]
        setContent {
            val state by viewModel.state.collectAsStateWithLifecycle()
            val status by viewModel.connectionStatus.collectAsStateWithLifecycle()
            val settings by viewModel.connectionSettings.collectAsStateWithLifecycle()
            val password by viewModel.accessPassword.collectAsStateWithLifecycle()
            val fileGrants by graph.fileStorage.grants.collectAsStateWithLifecycle()
            val preferences by product.preferences.collectAsStateWithLifecycle()
            val preferencesFailed by product.preferencesFailed.collectAsStateWithLifecycle()
            val history by product.history.collectAsStateWithLifecycle()
            LaunchedEffect(state.serviceReady, state.captureActive) { startupPermissions.refresh() }
            PixelsTheme(preferences) {
                val lightBars = MaterialTheme.colorScheme.background.luminance() > 0.5f
                SideEffect {
                    WindowCompat.getInsetsController(window, window.decorView).apply {
                        isAppearanceLightStatusBars = lightBars
                        isAppearanceLightNavigationBars = lightBars
                    }
                }
                DeviceScreen(
                    state,
                    settings,
                    status,
                    password,
                    preferences = preferences,
                    preferencesFailed = preferencesFailed,
                    onPreferences = product::savePreferences,
                    history = history,
                    onRefreshHistory = product::refreshHistory,
                    onSaveSettings = viewModel::saveConnection,
                    notificationAccess =
                        SetupPermission.NOTIFICATION_ACCESS !in startupPermissions.missing,
                    onNotificationSettings = {
                        startActivity(Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS))
                    },
                    onPreparePermissions = startupPermissions::authorizeMissing,
                    grantedPermissions =
                        SetupPermission.entries.toSet() - startupPermissions.missing,
                    permissionsBusy = startupPermissions.busy,
                    onManagePermission = startupPermissions::open,
                    missingPermissions = startupPermissions.missing.size,
                    permissionSetupError = startupPermissions.error,
                    onConnect = { updated ->
                        viewModel.saveConnection(updated) {
                            if (
                                Build.VERSION.SDK_INT >= 33 &&
                                    checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) !=
                                        PackageManager.PERMISSION_GRANTED
                            ) {
                                notificationConsent.launch(Manifest.permission.POST_NOTIFICATIONS)
                            } else ConnectionService.start(this@MainActivity)
                        }
                    },
                    onDisconnect = { ConnectionService.stop(this@MainActivity) },
                    onRotatePassword = viewModel::rotateAccessPassword,
                    onSettings = { startActivity(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS)) },
                    onInstallSettings = {
                        startActivity(
                            Intent(
                                Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
                                android.net.Uri.parse("package:$packageName"),
                            )
                        )
                    },
                    onReload = viewModel::reloadSettings,
                    onAuthorizeCapture = {
                        val manager = getSystemService(MediaProjectionManager::class.java)
                        captureConsent.launch(manager.createScreenCaptureIntent())
                    },
                    onStopCapture = {
                        stopService(Intent(this@MainActivity, ProjectionCaptureService::class.java))
                    },
                    onObserve = viewModel::observe,
                    onCapture = viewModel::capture,
                    fileGrants = fileGrants,
                    fileGrantError = fileGrantError,
                    onAuthorizeFiles = {
                        directoryConsent.launch(
                            Intent(Intent.ACTION_OPEN_DOCUMENT_TREE)
                                .addFlags(
                                    Intent.FLAG_GRANT_READ_URI_PERMISSION or
                                        Intent.FLAG_GRANT_WRITE_URI_PERMISSION or
                                        Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION or
                                        Intent.FLAG_GRANT_PREFIX_URI_PERMISSION
                                )
                        )
                    },
                    onRevokeFiles = { id ->
                        updateFileGrant { graph.fileStorage.revoke(id) }
                    },
                    onReadOnlyFiles = { id ->
                        updateFileGrant { graph.fileStorage.makeReadOnly(id) }
                    },
                    onTestTarget =
                        if (
                            applicationInfo.flags and
                                android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE != 0
                        ) {
                            {
                                startActivity(
                                    Intent()
                                        .setClassName(
                                            packageName,
                                            "com.pixels.agent.fixture.AutomationFixtureActivity",
                                        )
                                        .addFlags(Intent.FLAG_ACTIVITY_REORDER_TO_FRONT)
                                )
                            }
                        } else null,
                )
                PermissionSetupDialog(startupPermissions)
            }
        }
    }

    override fun onStart() {
        super.onStart()
        graph.environment.start()
    }

    override fun onResume() {
        super.onResume()
        // A visible Activity also recovers when an OEM suppressed the boot broadcast.
        lifecycleScope.launch {
            graph.productPreferences.load()
            try {
                graph.controlIntent.load()
                if (
                    !graph.productPreferences.failed.value &&
                        com.pixels.agent.core.shouldRestoreConnection(
                            graph.productPreferences.state.value.startOnBoot,
                            graph.controlIntent.state.value,
                        ) &&
                        !graph.connection.status.value.active
                )
                    ConnectionService.restore(this@MainActivity)
            } catch (_: Exception) {
                // The existing home status provides configuration and connection recovery.
            }
        }
        graph.environment.refreshScreen()
        updateFileGrant { graph.fileStorage.refresh() }
        startupPermissions.refresh()
    }

    override fun onSaveInstanceState(outState: Bundle) {
        startupPermissions.save(outState)
        super.onSaveInstanceState(outState)
    }

    override fun onStop() {
        graph.environment.stop()
        super.onStop()
    }

    private fun updateFileGrant(action: () -> Unit) {
        lifecycleScope.launch {
            fileGrantError = withContext(Dispatchers.IO) { runCatching(action).isFailure }
        }
    }
}
