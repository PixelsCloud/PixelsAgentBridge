package com.pixels.agent.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.pixels.agent.R
import com.pixels.agent.core.DeviceAccessPassword

internal val AccessPasswordMask = "•".repeat(DeviceAccessPassword.LENGTH)

@Composable
internal fun ProductCard(
    modifier: Modifier = Modifier,
    tint: Boolean = false,
    content: @Composable ColumnScope.() -> Unit,
) {
    Surface(
        modifier.fillMaxWidth(),
        shape = RoundedCornerShape(16.dp),
        color =
            if (tint) MaterialTheme.colorScheme.surfaceVariant
            else MaterialTheme.colorScheme.surface,
        contentColor = MaterialTheme.colorScheme.onSurface,
    ) {
        Column(content = content)
    }
}

@Composable
internal fun PageHeading(title: String, subtitle: String? = null) {
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(title, style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold)
        if (subtitle != null)
            Text(
                subtitle,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodyMedium,
            )
    }
}

@Composable
internal fun SectionHeading(title: String) {
    Text(
        title,
        style = MaterialTheme.typography.titleSmall,
        fontWeight = FontWeight.SemiBold,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(top = 4.dp, start = 4.dp),
    )
}

@Composable
internal fun SettingRow(
    icon: Glyph,
    title: String,
    detail: String? = null,
    positive: Boolean = false,
    onClick: (() -> Unit)? = null,
) {
    Row(
        Modifier.fillMaxWidth()
            .then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier)
            .heightIn(min = 56.dp)
            .padding(horizontal = 16.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        PixelsIcon(icon, tint = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(title, Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge)
        if (detail != null)
            Text(
                detail,
                Modifier.widthIn(max = 140.dp),
                style = MaterialTheme.typography.bodySmall,
                color =
                    if (positive) MaterialTheme.colorScheme.tertiary
                    else MaterialTheme.colorScheme.onSurfaceVariant,
            )
        if (onClick != null)
            PixelsIcon(
                Glyph.CHEVRON,
                Modifier.size(16.dp),
                MaterialTheme.colorScheme.onSurfaceVariant,
            )
    }
}

@Composable
internal fun CardDivider() {
    HorizontalDivider(
        Modifier.padding(start = 54.dp, end = 16.dp),
        color = MaterialTheme.colorScheme.outlineVariant,
    )
}

@Composable
internal fun ProductDialog(title: String, onDismiss: () -> Unit, content: @Composable () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = content,
        confirmButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(R.string.product_close)) }
        },
    )
}

@Composable
internal fun ConfirmDialog(
    title: String,
    message: String,
    onDismiss: () -> Unit,
    onConfirm: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = { Text(message) },
        confirmButton = {
            TextButton(
                onClick = {
                    onConfirm()
                    onDismiss()
                }
            ) {
                Text(stringResource(R.string.product_confirm))
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(R.string.product_cancel)) }
        },
    )
}
