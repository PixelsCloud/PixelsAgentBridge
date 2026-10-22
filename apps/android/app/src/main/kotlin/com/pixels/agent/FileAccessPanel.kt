package com.pixels.agent

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.res.stringResource
import com.pixels.agent.platform.storage.StorageGrant

@Composable
internal fun FileAccessPanel(
    grants: List<StorageGrant>,
    failed: Boolean,
    onAuthorize: () -> Unit,
    onRevoke: (String) -> Unit,
    onReadOnly: (String) -> Unit,
) {
    Text(stringResource(R.string.files_title), style = MaterialTheme.typography.titleLarge)
    Text(stringResource(R.string.files_workspace))
    TextButton(onClick = onAuthorize) { Text(stringResource(R.string.files_authorize)) }
    for (grant in grants) {
        Text("${grant.name} — /${grant.mount}")
        Text(
            stringResource(
                if (grant.writable) R.string.files_read_write else R.string.files_read_only
            )
        )
        TextButton(onClick = { onRevoke(grant.mount) }) {
            Text(stringResource(R.string.files_revoke))
        }
        if (grant.writable) {
            TextButton(onClick = { onReadOnly(grant.mount) }) {
                Text(stringResource(R.string.files_make_read_only))
            }
        }
    }
    if (failed)
        Text(stringResource(R.string.files_grant_failed), color = MaterialTheme.colorScheme.error)
}
