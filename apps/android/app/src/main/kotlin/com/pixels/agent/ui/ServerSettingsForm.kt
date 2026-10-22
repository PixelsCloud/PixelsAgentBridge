package com.pixels.agent.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import com.pixels.agent.ProductActions
import com.pixels.agent.ProductDevice
import com.pixels.agent.R

@Composable
internal fun ServerSettingsForm(
    device: ProductDevice,
    actions: ProductActions,
    onSaved: () -> Unit,
) {
    var url by
        rememberSaveable(device.settings.controlUrl) { mutableStateOf(device.settings.controlUrl) }
    var relay by
        rememberSaveable(device.settings.relayUrls) {
            mutableStateOf(device.settings.relayUrls.firstOrNull().orEmpty())
        }
    ProductCard {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            OutlinedTextField(
                url,
                { if (it.length <= 2048) url = it },
                label = { Text(stringResource(R.string.control_url)) },
                singleLine = true,
                enabled = !device.status.active,
                modifier = Modifier.fillMaxWidth().testTag("control_url"),
            )
            OutlinedTextField(
                relay,
                { if (it.length <= 2048) relay = it },
                label = { Text(stringResource(R.string.relay_url)) },
                singleLine = true,
                enabled = !device.status.active,
                modifier = Modifier.fillMaxWidth().testTag("relay_url"),
            )
            val updated =
                device.settings.copy(
                    controlUrl = url.trim(),
                    relayUrls =
                        if (relay.trim() == device.settings.relayUrls.firstOrNull().orEmpty())
                            device.settings.relayUrls
                        else if (relay.isBlank()) emptyList() else listOf(relay.trim()),
                )
            // Save uses the existing native validator, preserving trusted CAs and all relay
            // entries.
            Button(
                onClick = { actions.saveSettings(updated, onSaved) },
                enabled = !device.status.active && device.state.settingsLoaded && url.isNotBlank(),
                modifier = Modifier.fillMaxWidth(),
            ) {
                Text(stringResource(R.string.product_save))
            }
            OutlinedButton(
                onClick = {
                    if (device.status.active) actions.disconnect() else actions.connect(updated)
                },
                enabled =
                    device.state.settingsLoaded && device.state.nativeLoaded && url.isNotBlank(),
                modifier = Modifier.fillMaxWidth(),
            ) {
                Text(
                    stringResource(
                        if (device.status.active) R.string.disconnect else R.string.connect
                    )
                )
            }
        }
    }
}
