package com.pixels.agent

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import com.pixels.agent.core.ConnectionPhase
import com.pixels.agent.core.ConnectionSettings
import com.pixels.agent.core.ConnectionStatus

@Composable
internal fun ConnectionPanel(
    settings: ConnectionSettings,
    status: ConnectionStatus,
    accessPassword: String?,
    canConnect: Boolean,
    onConnect: (ConnectionSettings) -> Unit,
    onDisconnect: () -> Unit,
    onRotatePassword: () -> Unit,
) {
    var url by rememberSaveable(settings.controlUrl) { mutableStateOf(settings.controlUrl) }
    var name by rememberSaveable(settings.deviceName) { mutableStateOf(settings.deviceName) }
    var relay by
        rememberSaveable(settings.relayUrls) {
            mutableStateOf(settings.relayUrls.firstOrNull().orEmpty())
        }
    var serverExpanded by rememberSaveable { mutableStateOf(false) }
    var revealPassword by remember(accessPassword) { mutableStateOf(false) }
    val deviceCode = status.deviceCode
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        OutlinedTextField(
            value = name,
            onValueChange = { if (it.length <= 128) name = it },
            label = { Text(stringResource(R.string.device_name)) },
            singleLine = true,
            enabled = !status.active,
            modifier = Modifier.fillMaxWidth().testTag("device_name"),
        )
        Button(onClick = { serverExpanded = !serverExpanded }, enabled = !status.active) {
            Text(stringResource(R.string.server_settings))
        }
        if (serverExpanded) {
            OutlinedTextField(
                value = url,
                onValueChange = { if (it.length <= 2048) url = it },
                label = { Text(stringResource(R.string.control_url)) },
                singleLine = true,
                enabled = !status.active,
                modifier = Modifier.fillMaxWidth().testTag("control_url"),
            )
            OutlinedTextField(
                value = relay,
                onValueChange = { if (it.length <= 2048) relay = it },
                label = { Text(stringResource(R.string.relay_url)) },
                singleLine = true,
                enabled = !status.active,
                modifier = Modifier.fillMaxWidth().testTag("relay_url"),
            )
        }
        val label =
            when (status.phase) {
                ConnectionPhase.DISCONNECTED -> R.string.connection_disconnected
                ConnectionPhase.REGISTERING -> R.string.connection_registering
                ConnectionPhase.CONNECTING -> R.string.connection_connecting
                ConnectionPhase.AUTHENTICATED -> R.string.connection_authenticated
                ConnectionPhase.RECONNECTING -> R.string.connection_reconnecting
                ConnectionPhase.STOPPING -> R.string.connection_stopping
                ConnectionPhase.FAILED -> R.string.connection_failed
            }
        Text(stringResource(label))
        if (status.remoteReady) Text(stringResource(R.string.remote_channel_ready))
        if (status.error != null)
            Text(stringResource(R.string.connection_error), color = MaterialTheme.colorScheme.error)
        if (status.active) {
            Button(onClick = onDisconnect, enabled = status.phase != ConnectionPhase.STOPPING) {
                Text(stringResource(R.string.disconnect))
            }
        } else {
            Button(
                onClick = {
                    onConnect(
                        settings.copy(
                            controlUrl = url.trim(),
                            deviceName = name.trim(),
                            relayUrls =
                                if (relay.trim() == settings.relayUrls.firstOrNull().orEmpty())
                                    settings.relayUrls
                                else if (relay.isBlank()) emptyList() else listOf(relay.trim()),
                        )
                    )
                },
                enabled = canConnect && url.isNotBlank() && name.isNotBlank(),
                modifier = Modifier.testTag("connect_device"),
            ) {
                Text(stringResource(R.string.connect))
            }
        }
        Text(stringResource(R.string.setup_access), style = MaterialTheme.typography.titleLarge)
        if (accessPassword != null && deviceCode != null) {
            Text(stringResource(R.string.access_instructions))
            SelectionContainer { Text(stringResource(R.string.device_code, deviceCode)) }
            OutlinedTextField(
                value = accessPassword,
                onValueChange = {},
                readOnly = true,
                label = { Text(stringResource(R.string.access_password)) },
                visualTransformation =
                    if (revealPassword) VisualTransformation.None
                    else PasswordVisualTransformation(),
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            Button(onClick = { revealPassword = !revealPassword }) {
                Text(
                    stringResource(
                        if (revealPassword) R.string.hide_password else R.string.show_password
                    )
                )
            }
        }
        if (accessPassword == null || status.deviceCode == null)
            Text(stringResource(R.string.access_pending))
        run {
            Text(stringResource(R.string.rotate_password_explanation))
            Button(onClick = onRotatePassword, enabled = canConnect) {
                Text(stringResource(R.string.rotate_password))
            }
        }
    }
}
