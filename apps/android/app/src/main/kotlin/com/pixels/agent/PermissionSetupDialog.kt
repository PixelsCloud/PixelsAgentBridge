package com.pixels.agent

import androidx.compose.foundation.layout.Column
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.res.stringResource
import com.pixels.agent.core.SetupPermission

@Composable
internal fun PermissionSetupDialog(setup: StartupPermissions) {
    val permission = setup.pending ?: return
    AlertDialog(
        onDismissRequest = { if (!setup.busy) setup.dismiss() },
        title = { Text(stringResource(R.string.permission_setup_title)) },
        text = {
            Column {
                Text(
                    stringResource(
                        when (permission) {
                            SetupPermission.NOTIFICATIONS -> R.string.permission_setup_notifications
                            SetupPermission.NOTIFICATION_ACCESS ->
                                R.string.permission_setup_notification_access
                            SetupPermission.ACCESSIBILITY -> R.string.permission_setup_accessibility
                            SetupPermission.INSTALL_APPS -> R.string.permission_setup_install
                            SetupPermission.FILES -> R.string.permission_setup_files
                            SetupPermission.SCREEN_CAPTURE -> R.string.permission_setup_capture
                        }
                    )
                )
                if (setup.error) Text(stringResource(R.string.permission_setup_failed))
            }
        },
        confirmButton = {
            TextButton(onClick = setup::request, enabled = !setup.busy) {
                Text(stringResource(R.string.permission_setup_continue))
            }
        },
        dismissButton = {
            Column {
                if (permission == SetupPermission.NOTIFICATIONS) {
                    TextButton(onClick = setup::notificationSettings, enabled = !setup.busy) {
                        Text(stringResource(R.string.permission_setup_notification_settings))
                    }
                }
                if (permission == SetupPermission.FILES) {
                    TextButton(onClick = setup::workspaceOnly, enabled = !setup.busy) {
                        Text(stringResource(R.string.permission_setup_workspace_only))
                    }
                }
                TextButton(onClick = setup::dismiss, enabled = !setup.busy) {
                    Text(stringResource(R.string.permission_setup_later))
                }
            }
        },
    )
}
