package com.pixels.agent

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import com.pixels.agent.core.ControlAvailability

@Composable
internal fun ControlStatusPanel(
    state: DeviceUiState,
    onPause: (Boolean) -> Unit,
    onSettings: () -> Unit,
    onReload: () -> Unit,
) {
    val message =
        when (state.availability) {
            ControlAvailability.PREPARING -> R.string.status_preparing
            ControlAvailability.CORE_UNAVAILABLE -> R.string.status_core_unavailable
            ControlAvailability.SETTINGS_UNAVAILABLE -> R.string.status_settings_unavailable
            ControlAvailability.PERMISSION_REQUIRED -> R.string.status_permission_required
            ControlAvailability.PAUSED -> R.string.status_paused
            ControlAvailability.SCREEN_LOCKED -> R.string.status_locked
            ControlAvailability.OFFLINE -> R.string.status_offline
            ControlAvailability.DISCONNECTED -> R.string.status_disconnected
            ControlAvailability.CONNECTING -> R.string.status_connecting
            ControlAvailability.RECONNECTING -> R.string.status_reconnecting
            ControlAvailability.STOPPING -> R.string.status_stopping
            ControlAvailability.CONNECTION_FAILED -> R.string.status_failed
            ControlAvailability.CHANNEL_UNAVAILABLE -> R.string.status_channel_unavailable
            ControlAvailability.READY -> R.string.status_ready
        }
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(
                stringResource(message),
                style = MaterialTheme.typography.titleMedium,
                modifier = Modifier.testTag("control_status"),
            )
            if (state.availability == ControlAvailability.PERMISSION_REQUIRED) {
                Button(onClick = onSettings) { Text(stringResource(R.string.open_accessibility)) }
            }
            if (state.availability == ControlAvailability.SETTINGS_UNAVAILABLE) {
                Button(onClick = onReload) { Text(stringResource(R.string.reload_settings)) }
            }
            Button(onClick = { onPause(!state.paused) }) {
                Text(
                    stringResource(
                        if (state.paused) R.string.resume_control else R.string.pause_control
                    )
                )
            }
        }
    }
}
