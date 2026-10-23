package com.pixels.agent.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.os.Build
import android.os.PersistableBundle
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.pixels.agent.ActivityHistory
import com.pixels.agent.ProductActions
import com.pixels.agent.ProductDevice
import com.pixels.agent.R
import com.pixels.agent.core.ControlAvailability

@Composable
internal fun HomePage(
    device: ProductDevice,
    actions: ProductActions,
    history: ActivityHistory,
    onAccess: () -> Unit,
    onActivity: () -> Unit,
) {
    val context = LocalContext.current
    var revealed by remember(device.password) { mutableStateOf(true) }
    var copied by remember(device.password) { mutableStateOf<Int?>(null) }
    LaunchedEffect(copied) {
        if (copied != null) {
            kotlinx.coroutines.delay(2000)
            copied = null
        }
    }
    Column(
        Modifier.fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(horizontal = 20.dp)
            .padding(bottom = 20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(
                stringResource(R.string.product_my_phone),
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.Bold,
            )
            Text(
                "${Build.MODEL} · Android ${Build.VERSION.RELEASE}",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodyMedium,
            )
        }
        ProductCard(tint = true) {
            Column(Modifier.padding(start = 20.dp, end = 8.dp, top = 12.dp, bottom = 8.dp)) {
                Text(
                    stringResource(R.string.product_device_code),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        device.status.deviceCode?.chunked(3)?.joinToString(" ") ?: "— — —",
                        style = MaterialTheme.typography.headlineMedium,
                        fontWeight = FontWeight.Bold,
                        letterSpacing = 1.5.sp,
                        modifier = Modifier.weight(1f).testTag("device_code"),
                    )
                    IconButton(
                        enabled = !device.status.deviceCode.isNullOrEmpty(),
                        onClick = {
                            context
                                .getSystemService(ClipboardManager::class.java)
                                .setPrimaryClip(
                                    ClipData.newPlainText(
                                        context.getString(R.string.product_device_code),
                                        device.status.deviceCode,
                                    )
                                )
                            copied = R.string.product_code_copied
                        },
                    ) {
                        PixelsIcon(
                            Glyph.COPY,
                            description = stringResource(R.string.product_copy_code),
                        )
                    }
                }
                HorizontalDivider(
                    color = MaterialTheme.colorScheme.outline.copy(alpha = .6f),
                    modifier = Modifier.padding(end = 12.dp),
                )
                Spacer(Modifier.height(10.dp))
                Text(
                    stringResource(R.string.product_access_password),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        if (revealed) device.password ?: "—" else AccessPasswordMask,
                        fontSize = 22.sp,
                        letterSpacing = 3.sp,
                        modifier = Modifier.weight(1f).testTag("access_password"),
                    )
                    IconButton(
                        enabled = !device.password.isNullOrEmpty(),
                        onClick = {
                            val password = device.password ?: return@IconButton
                            val clip =
                                ClipData.newPlainText(
                                    context.getString(R.string.product_access_password),
                                    password,
                                )
                            clip.description.extras =
                                PersistableBundle().apply {
                                    putBoolean("android.content.extra.IS_SENSITIVE", true)
                                }
                            context
                                .getSystemService(ClipboardManager::class.java)
                                .setPrimaryClip(clip)
                            copied = R.string.product_password_copied
                        },
                    ) {
                        PixelsIcon(
                            Glyph.COPY,
                            description = stringResource(R.string.product_copy_password),
                        )
                    }
                    IconButton(
                        enabled = device.password != null,
                        onClick = { revealed = !revealed },
                    ) {
                        PixelsIcon(
                            if (revealed) Glyph.HIDE else Glyph.EYE,
                            description =
                                stringResource(
                                    if (revealed) R.string.hide_password else R.string.show_password
                                ),
                        )
                    }
                    IconButton(
                        onClick = actions.rotatePassword,
                        enabled = !device.state.passwordRotating,
                    ) {
                        PixelsIcon(
                            Glyph.ROTATE,
                            description = stringResource(R.string.rotate_password),
                        )
                    }
                }
                copied?.let { message ->
                    Text(
                        stringResource(message),
                        color = MaterialTheme.colorScheme.primary,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
            }
        }
        ProductCard {
            Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                TextButton(onClick = onAccess, contentPadding = PaddingValues(0.dp)) {
                    Text(
                        stringResource(
                            if (device.missingPermissions == 0) R.string.product_access_ready
                            else R.string.product_access_missing
                        ),
                        fontWeight = FontWeight.SemiBold,
                        color = MaterialTheme.colorScheme.onSurface,
                    )
                    Spacer(Modifier.weight(1f))
                    PixelsIcon(Glyph.CHEVRON, Modifier.size(16.dp))
                }
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                    Readiness(
                        stringResource(R.string.product_interface_control),
                        device.state.serviceReady,
                    )
                    Readiness(stringResource(R.string.product_folders), true)
                    Readiness(
                        stringResource(R.string.product_notification_access),
                        device.notificationAccess,
                    )
                }
                if (device.permissionError)
                    Text(
                        stringResource(R.string.permission_setup_failed),
                        color = MaterialTheme.colorScheme.error,
                    )
            }
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                stringResource(R.string.product_recent_activity),
                style = MaterialTheme.typography.titleSmall,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier.weight(1f),
            )
            TextButton(onClick = onActivity) {
                Text(stringResource(R.string.product_view_all))
                PixelsIcon(Glyph.CHEVRON, Modifier.size(14.dp))
            }
        }
        ProductCard {
            if (history.records.isEmpty()) EmptyActivity(compact = true)
            else
                history.records.take(2).forEachIndexed { index, record ->
                    ActivityRow(record, compact = true)
                    if (index == 0 && history.records.size > 1) CardDivider()
                }
        }
        device.state.error?.let {
            Text(
                stringResource(deviceErrorMessage(it)),
                color = MaterialTheme.colorScheme.error,
                style = MaterialTheme.typography.bodySmall,
            )
        }
        if (device.state.settingsFailed)
            TextButton(onClick = actions.reload) {
                Text(stringResource(R.string.reload_settings))
            }
    }
}

@Composable
private fun Readiness(label: String, ready: Boolean) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        PixelsIcon(
            if (ready) Glyph.CHECK else Glyph.INFO,
            Modifier.size(14.dp),
            if (ready) MaterialTheme.colorScheme.primary
            else MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Text(
            label,
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

internal fun controlAvailabilityMessage(value: ControlAvailability): Int =
    when (value) {
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

internal fun deviceErrorMessage(error: String): Int =
    when (error) {
        "device_rename_rate_limited" -> R.string.product_name_rate_limited
        "device_rename_failed" -> R.string.product_name_failed
        "settings_unavailable" -> R.string.status_settings_unavailable
        "settings_invalid_or_unavailable" -> R.string.error_settings_invalid
        "SERVICE_UNAVAILABLE" -> R.string.status_permission_required
        "CONTROL_PAUSED" -> R.string.status_paused
        "SCREEN_LOCKED" -> R.string.status_locked
        "CONSENT_REQUIRED" -> R.string.capture_session_missing
        "FRAME_UNAVAILABLE" -> R.string.error_capture_wait
        "PROTECTED_OR_REJECTED" -> R.string.error_capture_protected
        "TARGET_STALE" -> R.string.error_screen_changed
        "BUSY" -> R.string.error_busy
        else -> R.string.error_operation_failed
    }
