package com.pixels.agent

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.testTagsAsResourceId
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.pixels.agent.core.AppearancePreferences
import com.pixels.agent.core.ConnectionSettings
import com.pixels.agent.core.ConnectionStatus
import com.pixels.agent.core.SetupPermission
import com.pixels.agent.platform.storage.StorageGrant
import com.pixels.agent.ui.AccessPage
import com.pixels.agent.ui.ActivityPage
import com.pixels.agent.ui.Glyph
import com.pixels.agent.ui.HomePage
import com.pixels.agent.ui.PixelsIcon
import com.pixels.agent.ui.ProductNavigation
import com.pixels.agent.ui.SettingsPage

internal data class ProductDevice(
    val state: DeviceUiState,
    val settings: ConnectionSettings,
    val status: ConnectionStatus,
    val password: String?,
    val grants: List<StorageGrant>,
    val grantError: Boolean,
    val missingPermissions: Int,
    val permissionError: Boolean,
    val notificationAccess: Boolean,
    val grantedPermissions: Set<SetupPermission>,
    val permissionsBusy: Boolean,
)

internal data class ProductActions(
    val connect: (ConnectionSettings) -> Unit,
    val disconnect: () -> Unit,
    val rotatePassword: () -> Unit,
    val accessibility: () -> Unit,
    val notifications: () -> Unit,
    val reload: () -> Unit,
    val authorizeCapture: () -> Unit,
    val stopCapture: () -> Unit,
    val observe: () -> Unit,
    val capture: () -> Unit,
    val testTarget: (() -> Unit)?,
    val authorizeFiles: () -> Unit,
    val revokeFiles: (String) -> Unit,
    val readOnlyFiles: (String) -> Unit,
    val installSettings: () -> Unit,
    val preparePermissions: () -> Unit,
    val saveSettings: (ConnectionSettings, () -> Unit) -> Unit,
    val managePermission: (SetupPermission) -> Unit,
)

@Composable
internal fun DeviceScreen(
    state: DeviceUiState,
    settings: ConnectionSettings,
    status: ConnectionStatus,
    password: String?,
    onConnect: (ConnectionSettings) -> Unit,
    onDisconnect: () -> Unit,
    onRotatePassword: () -> Unit,
    onSettings: () -> Unit,
    onReload: () -> Unit,
    onAuthorizeCapture: () -> Unit,
    onStopCapture: () -> Unit,
    onObserve: () -> Unit,
    onCapture: () -> Unit,
    onTestTarget: (() -> Unit)?,
    fileGrants: List<StorageGrant>,
    fileGrantError: Boolean,
    onAuthorizeFiles: () -> Unit,
    onRevokeFiles: (String) -> Unit,
    onReadOnlyFiles: (String) -> Unit,
    onInstallSettings: () -> Unit,
    onPreparePermissions: () -> Unit,
    missingPermissions: Int,
    permissionSetupError: Boolean,
    notificationAccess: Boolean,
    onNotificationSettings: () -> Unit,
    preferences: AppearancePreferences,
    preferencesFailed: Boolean,
    onPreferences: (AppearancePreferences) -> Unit,
    history: ActivityHistory,
    onRefreshHistory: () -> Unit,
    onSaveSettings: (ConnectionSettings, () -> Unit) -> Unit,
    grantedPermissions: Set<SetupPermission>,
    permissionsBusy: Boolean,
    onManagePermission: (SetupPermission) -> Unit,
) {
    var page by rememberSaveable { mutableIntStateOf(0) }
    var detail by rememberSaveable { mutableStateOf<String?>(null) }
    val device =
        ProductDevice(
            state,
            settings,
            status,
            password,
            fileGrants,
            fileGrantError,
            missingPermissions,
            permissionSetupError,
            notificationAccess,
            grantedPermissions,
            permissionsBusy,
        )
    val actions =
        ProductActions(
            onConnect,
            onDisconnect,
            onRotatePassword,
            onSettings,
            onNotificationSettings,
            onReload,
            onAuthorizeCapture,
            onStopCapture,
            onObserve,
            onCapture,
            onTestTarget,
            onAuthorizeFiles,
            onRevokeFiles,
            onReadOnlyFiles,
            onInstallSettings,
            onPreparePermissions,
            onSaveSettings,
            onManagePermission,
        )
    BackHandler(detail != null || page != 0) { if (detail != null) detail = null else page = 0 }
    Scaffold(
        modifier = Modifier.semantics { testTagsAsResourceId = true },
        containerColor = MaterialTheme.colorScheme.background,
        bottomBar = {
            ProductNavigation(page) {
                page = it
                detail = null
            }
        },
    ) { insets ->
        Column(Modifier.fillMaxSize().padding(insets)) {
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 20.dp).height(64.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                if (detail != null)
                    IconButton(onClick = { detail = null }) {
                        PixelsIcon(Glyph.BACK, description = stringResource(R.string.product_back))
                    }
                else
                    PixelsIcon(Glyph.BRAND, Modifier.size(28.dp), MaterialTheme.colorScheme.primary)
                Text(
                    stringResource(R.string.app_name),
                    style = MaterialTheme.typography.titleLarge,
                    fontWeight = FontWeight.Bold,
                    modifier = Modifier.weight(1f),
                )
                if (page == 0) {
                    Text(
                        "●",
                        color =
                            if (status.remoteReady) MaterialTheme.colorScheme.tertiary
                            else MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Text(
                        stringResource(
                            if (status.remoteReady) R.string.product_online
                            else R.string.product_offline
                        ),
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
            }
            when (page) {
                0 ->
                    HomePage(
                        device,
                        actions,
                        history,
                        { page = 1 },
                        { page = 2 },
                    )
                1 -> AccessPage(device, actions)
                2 -> ActivityPage(history, onRefreshHistory)
                else ->
                    SettingsPage(
                        device,
                        actions,
                        preferences,
                        preferencesFailed,
                        onPreferences,
                        detail,
                        { detail = it },
                    )
            }
        }
    }
}
