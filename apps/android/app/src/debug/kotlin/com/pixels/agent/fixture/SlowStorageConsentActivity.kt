package com.pixels.agent.fixture

import android.content.Intent
import android.os.Bundle
import android.provider.DocumentsContract
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import com.pixels.agent.PixelsApplication

/** Opens the real system consent UI, restricted to this debug provider's own root. */
class SlowStorageConsentActivity : ComponentActivity() {
    private val picker =
        registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
            val data = result.data
            val uri = data?.data
            if (
                result.resultCode == RESULT_OK &&
                    uri != null &&
                    uri.authority == "$packageName.slow-documents" &&
                    DocumentsContract.isTreeUri(uri) &&
                    DocumentsContract.getTreeDocumentId(uri) == "root"
            ) {
                (application as PixelsApplication).fileStorage.grant(uri, data.flags)
            }
            finish()
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (savedInstanceState == null) {
            picker.launch(
                Intent(Intent.ACTION_OPEN_DOCUMENT_TREE).apply {
                    addFlags(
                        Intent.FLAG_GRANT_READ_URI_PERMISSION or
                            Intent.FLAG_GRANT_WRITE_URI_PERMISSION or
                            Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION or
                            Intent.FLAG_GRANT_PREFIX_URI_PERMISSION
                    )
                    putExtra(
                        DocumentsContract.EXTRA_INITIAL_URI,
                        DocumentsContract.buildDocumentUri("$packageName.slow-documents", "root"),
                    )
                }
            )
        }
    }
}
