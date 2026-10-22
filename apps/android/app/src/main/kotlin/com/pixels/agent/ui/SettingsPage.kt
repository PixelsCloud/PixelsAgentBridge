package com.pixels.agent.ui

import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import com.pixels.agent.ProductActions
import com.pixels.agent.ProductDevice
import com.pixels.agent.R
import com.pixels.agent.core.AppLanguage
import com.pixels.agent.core.Appearance
import com.pixels.agent.core.AppearancePreferences

@Composable
internal fun SettingsPage(
    device: ProductDevice,
    actions: ProductActions,
    preferences: AppearancePreferences,
    preferencesFailed: Boolean,
    onPreferences: (AppearancePreferences) -> Unit,
    detail: String?,
    onDetail: (String?) -> Unit,
) {
    val context = LocalContext.current
    val version =
        remember(context) {
            val info = context.packageManager.getPackageInfo(context.packageName, 0)
            "v${info.longVersionCode}"
        }
    var dialog by rememberSaveable { mutableStateOf<String?>(null) }
    Column(
        Modifier.fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(horizontal = 20.dp)
            .padding(bottom = 24.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        if (detail == "connection") {
            PageHeading(stringResource(R.string.product_connection_settings))
            Text(
                stringResource(R.string.product_edit_disconnected),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            ServerSettingsForm(device, actions) { onDetail(null) }
            device.state.error?.let {
                Text(
                    stringResource(deviceErrorMessage(it)),
                    color = MaterialTheme.colorScheme.error,
                )
            }
        } else if (detail == "about") {
            PageHeading(stringResource(R.string.product_about_pixels))
            ProductCard {
                Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    PixelsIcon(Glyph.BRAND, Modifier.size(48.dp), MaterialTheme.colorScheme.primary)
                    Text(
                        stringResource(R.string.app_name),
                        style = MaterialTheme.typography.titleLarge,
                    )
                    Text(version, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    Text(
                        "${Build.MODEL} · Android ${Build.VERSION.RELEASE}",
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            SectionHeading(stringResource(R.string.product_diagnostics))
            ProductCard {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Text(
                        stringResource(
                            if (device.state.nativeLoaded) R.string.native_loaded
                            else R.string.native_failed
                        )
                    )
                    Text(stringResource(controlAvailabilityMessage(device.state.availability)))
                    TextButton(onClick = actions.reload) {
                        Text(stringResource(R.string.reload_settings))
                    }
                    Button(
                        onClick = actions.observe,
                        enabled = device.state.serviceReady && !device.state.paused,
                    ) {
                        Text(stringResource(R.string.observe_screen))
                    }
                    device.state.elementCount?.let {
                        Text(stringResource(R.string.screen_result, it))
                    }
                    Button(
                        onClick = actions.capture,
                        enabled = device.state.serviceReady && !device.state.paused,
                    ) {
                        Text(stringResource(R.string.capture_screen))
                    }
                    device.state.captureSize?.let {
                        Text(stringResource(R.string.capture_size, it.first, it.second))
                    }
                    actions.testTarget?.let { open ->
                        TextButton(onClick = open) {
                            Text(stringResource(R.string.open_test_target))
                        }
                    }
                }
            }
        } else if (detail == "help") {
            PageHeading(stringResource(R.string.product_help_feedback))
            ProductCard {
                Text(
                    stringResource(R.string.product_help_content),
                    Modifier.padding(20.dp),
                    style = MaterialTheme.typography.bodyLarge,
                )
            }
        } else {
            PageHeading(stringResource(R.string.product_tab_settings))
            SectionHeading(stringResource(R.string.product_this_device))
            ProductCard {
                SettingRow(
                    Glyph.PHONE,
                    stringResource(R.string.product_device_name),
                    device.settings.deviceName,
                ) {
                    dialog = "name"
                }
            }
            SectionHeading(stringResource(R.string.product_preferences))
            ProductCard {
                SettingRow(
                    Glyph.PALETTE,
                    stringResource(R.string.product_appearance),
                    stringResource(appearanceLabel(preferences.appearance)),
                ) {
                    dialog = "appearance"
                }
                CardDivider()
                SettingRow(
                    Glyph.GLOBE,
                    stringResource(R.string.product_language),
                    when (preferences.language) {
                        AppLanguage.SYSTEM -> stringResource(R.string.product_follow_system)
                        AppLanguage.CHINESE -> stringResource(R.string.product_language_chinese)
                        AppLanguage.ENGLISH -> stringResource(R.string.product_language_english)
                    },
                ) {
                    dialog = "language"
                }
            }
            if (preferencesFailed)
                Text(
                    stringResource(R.string.product_preferences_failed),
                    color = MaterialTheme.colorScheme.error,
                )
            SectionHeading(stringResource(R.string.startup_title))
            ProductCard {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                        Text(stringResource(R.string.startup_title), Modifier.weight(1f))
                        Switch(
                            checked = preferences.startOnBoot,
                            enabled = !preferencesFailed,
                            onCheckedChange = { onPreferences(preferences.copy(startOnBoot = it)) },
                        )
                    }
                    Text(
                        stringResource(R.string.startup_help),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        style = MaterialTheme.typography.bodySmall,
                    )
                    TextButton(
                        onClick = {
                            context.startActivity(
                                Intent(
                                        Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                                        Uri.parse("package:${context.packageName}"),
                                    )
                                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                            )
                        }
                    ) {
                        Text(stringResource(R.string.startup_app_settings))
                    }
                    TextButton(
                        onClick = {
                            val settings =
                                Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)
                                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                            try {
                                context.startActivity(settings)
                            } catch (_: android.content.ActivityNotFoundException) {
                                context.startActivity(
                                    Intent(
                                            Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                                            Uri.parse("package:${context.packageName}"),
                                        )
                                        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                                )
                            }
                        }
                    ) {
                        Text(stringResource(R.string.startup_battery_settings))
                    }
                }
            }
            SectionHeading(stringResource(R.string.product_connection))
            ProductCard {
                SettingRow(
                    Glyph.LINK,
                    stringResource(R.string.product_connection_settings),
                    stringResource(R.string.product_configured_server),
                ) {
                    onDetail("connection")
                }
            }
            SectionHeading(stringResource(R.string.product_help_about))
            ProductCard {
                SettingRow(Glyph.HELP, stringResource(R.string.product_help_feedback)) {
                    onDetail("help")
                }
                CardDivider()
                SettingRow(Glyph.INFO, stringResource(R.string.product_about_pixels), version) {
                    onDetail("about")
                }
            }
            Text(
                stringResource(R.string.product_app_footer),
                Modifier.align(Alignment.CenterHorizontally),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
    when (dialog) {
        "appearance" ->
            ChoiceDialog(
                stringResource(R.string.product_appearance),
                Appearance.entries.map { stringResource(appearanceLabel(it)) },
                preferences.appearance.ordinal,
                { dialog = null },
            ) { index ->
                onPreferences(preferences.copy(appearance = Appearance.entries[index]))
                dialog = null
            }
        "language" ->
            ChoiceDialog(
                stringResource(R.string.product_language),
                listOf(
                    stringResource(R.string.product_follow_system),
                    stringResource(R.string.product_language_chinese),
                    stringResource(R.string.product_language_english),
                ),
                preferences.language.ordinal,
                { dialog = null },
            ) { index ->
                onPreferences(preferences.copy(language = AppLanguage.entries[index]))
                dialog = null
            }
        "name" -> DeviceNameDialog(device, actions) { dialog = null }
    }
}

@Composable
private fun DeviceNameDialog(
    device: ProductDevice,
    actions: ProductActions,
    onDismiss: () -> Unit,
) {
    var value by rememberSaveable { mutableStateOf(device.settings.deviceName) }
    var saving by remember { mutableStateOf(false) }
    androidx.compose.runtime.LaunchedEffect(device.state.error) {
        if (device.state.error != null) saving = false
    }
    val valid = value.trim().length in 1..80 && value.none { it.isISOControl() }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.product_device_name)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(
                    value,
                    {
                        value = it
                        saving = false
                    },
                    singleLine = true,
                    label = { Text(stringResource(R.string.product_device_name)) },
                    modifier = Modifier.testTag("device_name"),
                    isError = !valid,
                )
                if (!valid)
                    Text(
                        stringResource(R.string.product_name_invalid),
                        color = MaterialTheme.colorScheme.error,
                    )
                device.state.error?.let {
                    Text(
                        stringResource(deviceErrorMessage(it)),
                        color = MaterialTheme.colorScheme.error,
                    )
                }
            }
        },
        confirmButton = {
            TextButton(
                enabled = valid && !saving,
                onClick = {
                    saving = true
                    actions.saveSettings(device.settings.copy(deviceName = value.trim()), onDismiss)
                },
            ) {
                Text(stringResource(R.string.product_save))
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(R.string.product_cancel)) }
        },
    )
}

@Composable
private fun ChoiceDialog(
    title: String,
    labels: List<String>,
    selected: Int,
    onDismiss: () -> Unit,
    onSelected: (Int) -> Unit,
) {
    ProductDialog(title, onDismiss) {
        Column {
            labels.forEachIndexed { index, label ->
                Row(
                    Modifier.fillMaxWidth().clickable { onSelected(index) }.heightIn(min = 52.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    RadioButton(selected == index, { onSelected(index) })
                    Text(label, style = MaterialTheme.typography.bodyLarge)
                }
            }
        }
    }
}

private fun appearanceLabel(appearance: Appearance): Int =
    when (appearance) {
        Appearance.SYSTEM -> R.string.product_follow_system
        Appearance.LIGHT -> R.string.product_light
        Appearance.DARK -> R.string.product_dark
    }
