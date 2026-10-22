package com.pixels.agent.ui

import android.os.Build
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.pixels.agent.ProductActions
import com.pixels.agent.ProductDevice
import com.pixels.agent.R
import com.pixels.agent.core.SetupPermission
import com.pixels.agent.platform.storage.StorageGrant

@Composable
internal fun AccessPage(device: ProductDevice, actions: ProductActions) {
    var menu by remember { mutableStateOf<String?>(null) }
    var confirmation by remember { mutableStateOf<Pair<StorageGrant, Boolean>?>(null) }
    val permissions =
        listOf(
            SetupPermission.ACCESSIBILITY to Glyph.SCREEN,
            SetupPermission.NOTIFICATIONS to Glyph.BELL,
            SetupPermission.NOTIFICATION_ACCESS to Glyph.BELL,
            SetupPermission.INSTALL_APPS to Glyph.UPLOAD,
        ) +
            if (Build.VERSION.SDK_INT < 30) listOf(SetupPermission.SCREEN_CAPTURE to Glyph.PHONE)
            else emptyList()
    Column(
        Modifier.fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(horizontal = 20.dp)
            .padding(bottom = 24.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Row(
            Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            SectionHeading(stringResource(R.string.product_app_permissions))
            if (permissions.any { (permission, _) -> permission !in device.grantedPermissions })
                TextButton(
                    onClick = actions.preparePermissions,
                    enabled = !device.permissionsBusy,
                ) {
                    Text(stringResource(R.string.product_grant_all))
                }
        }
        ProductCard {
            permissions.forEachIndexed { index, (permission, icon) ->
                if (index > 0) CardDivider()
                val granted = permission in device.grantedPermissions
                SettingRow(
                    icon,
                    stringResource(permissionLabel(permission)),
                    stringResource(
                        if (granted) R.string.product_permission_granted
                        else R.string.product_permission_grant
                    ),
                    granted,
                    onClick =
                        if (permission == SetupPermission.SCREEN_CAPTURE && granted) null
                        else {
                            { actions.managePermission(permission) }
                        },
                )
            }
        }
        if (device.permissionError)
            Text(
                stringResource(R.string.product_permission_failed),
                color = MaterialTheme.colorScheme.error,
                style = MaterialTheme.typography.bodySmall,
            )
        Row(
            Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            SectionHeading(stringResource(R.string.product_folders))
            TextButton(onClick = actions.authorizeFiles) {
                PixelsIcon(Glyph.PLUS, Modifier.size(18.dp))
                Spacer(Modifier.width(6.dp))
                Text(stringResource(R.string.product_add_folder))
            }
        }
        ProductCard {
            FolderRow(
                stringResource(R.string.product_app_workspace),
                null,
                true,
            )
            device.grants.forEach { grant ->
                CardDivider()
                FolderRow(grant.name, "/${grant.mount}", grant.writable) {
                    Box {
                        IconButton(onClick = { menu = grant.mount }) {
                            PixelsIcon(
                                Glyph.MORE,
                                description = stringResource(R.string.product_folder_actions),
                            )
                        }
                        DropdownMenu(
                            expanded = menu == grant.mount,
                            onDismissRequest = { menu = null },
                        ) {
                            if (grant.writable)
                                DropdownMenuItem(
                                    text = { Text(stringResource(R.string.files_make_read_only)) },
                                    onClick = {
                                        menu = null
                                        confirmation = grant to false
                                    },
                                )
                            DropdownMenuItem(
                                text = { Text(stringResource(R.string.files_revoke)) },
                                onClick = {
                                    menu = null
                                    confirmation = grant to true
                                },
                            )
                        }
                    }
                }
            }
        }
        if (device.grantError)
            Text(
                stringResource(R.string.files_grant_failed),
                color = MaterialTheme.colorScheme.error,
            )
    }
    confirmation?.let { (grant, revoke) ->
        ConfirmDialog(
            stringResource(if (revoke) R.string.files_revoke else R.string.files_make_read_only),
            stringResource(
                if (revoke) R.string.product_folder_revoke_confirm
                else R.string.product_folder_readonly_confirm
            ),
            { confirmation = null },
        ) {
            if (revoke) actions.revokeFiles(grant.mount) else actions.readOnlyFiles(grant.mount)
        }
    }
}

private fun permissionLabel(permission: SetupPermission): Int =
    when (permission) {
        SetupPermission.ACCESSIBILITY -> R.string.product_interface_control
        SetupPermission.NOTIFICATIONS -> R.string.product_app_notifications
        SetupPermission.NOTIFICATION_ACCESS -> R.string.product_notification_access
        SetupPermission.INSTALL_APPS -> R.string.product_install_apps
        SetupPermission.SCREEN_CAPTURE -> R.string.product_screen_view
        SetupPermission.FILES -> R.string.product_folders
    }

@Composable
private fun FolderRow(
    name: String,
    path: String?,
    writable: Boolean,
    trailing: (@Composable () -> Unit)? = null,
) {
    Row(
        Modifier.fillMaxWidth()
            .padding(start = 16.dp, end = if (trailing == null) 16.dp else 0.dp)
            .heightIn(min = 76.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        PixelsIcon(Glyph.FOLDER, tint = MaterialTheme.colorScheme.onSurfaceVariant)
        Column(
            Modifier.weight(1f).padding(vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Text(name, style = MaterialTheme.typography.bodyLarge, fontWeight = FontWeight.Medium)
            if (path != null)
                Text(
                    path,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
        }
        Surface(
            shape = androidx.compose.foundation.shape.RoundedCornerShape(6.dp),
            color =
                if (writable) MaterialTheme.colorScheme.primaryContainer
                else MaterialTheme.colorScheme.background,
        ) {
            Text(
                stringResource(
                    if (writable) R.string.product_read_write else R.string.product_read_only
                ),
                Modifier.padding(horizontal = 8.dp, vertical = 5.dp),
                style = MaterialTheme.typography.labelSmall,
                color =
                    if (writable) MaterialTheme.colorScheme.primary
                    else MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        trailing?.invoke()
    }
}
