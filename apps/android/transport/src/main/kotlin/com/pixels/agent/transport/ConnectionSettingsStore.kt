package com.pixels.agent.transport

import android.util.AtomicFile
import com.pixels.agent.core.ConnectionSettings
import java.io.File
import org.json.JSONArray
import org.json.JSONObject

internal data class ValidatedSettings(
    val settings: ConnectionSettings,
    val toml: String,
    val credentialScope: String,
)

/** TOML is the only settings file. JSON is confined to the in-memory JNI boundary. */
internal class ConnectionSettingsStore(directory: File, private val native: NativeBridge) {
    private val file = AtomicFile(File(directory, "settings.toml"))

    fun defaults(): ValidatedSettings = decode(native.defaultSettings())

    fun read(): ValidatedSettings? {
        val bytes =
            try {
                file.openRead().use { it.readBounded(65536) }
            } catch (_: java.io.FileNotFoundException) {
                return null
            }
        check(bytes.size <= 65536) { "config_too_large" }
        return decode(native.decodeSettings(bytes.toString(Charsets.UTF_8)))
    }

    fun validate(settings: ConnectionSettings): ValidatedSettings {
        // An unsupported or corrupt existing document must survive an older app's edits.
        // Refuse the write instead of replacing fields this version cannot understand.
        read()
        val input =
            JSONObject()
                .put("schema_version", 1)
                .put("control_url", settings.controlUrl)
                .put("device_name", settings.deviceName)
        settings.trustedCaPem?.let { input.put("trusted_ca_pem", it) }
        if (settings.relayUrls.isNotEmpty()) input.put("relay_urls", JSONArray(settings.relayUrls))
        return decode(native.encodeSettings(input.toString()))
    }

    fun save(settings: ConnectionSettings): ValidatedSettings {
        val validated = validate(settings)
        file.baseFile.parentFile?.mkdirs()
        val stream = file.startWrite()
        try {
            stream.write(validated.toml.toByteArray(Charsets.UTF_8))
            file.finishWrite(stream)
        } catch (error: Exception) {
            file.failWrite(stream)
            throw error
        }
        return validated
    }

    private fun decode(reply: String): ValidatedSettings {
        val value = nativeValue(reply)
        val config = value.getJSONObject("config")
        return ValidatedSettings(
            ConnectionSettings(
                config.getString("control_url"),
                config.getString("device_name"),
                config.optString("trusted_ca_pem").takeIf { it.isNotEmpty() },
                config.optJSONArray("relay_urls")?.let { array ->
                    (0 until array.length()).map { array.getString(it) }
                } ?: emptyList(),
            ),
            value.getString("toml"),
            value.getString("scope"),
        )
    }
}

internal class ConnectionFailure(val code: String) : Exception(code)

internal fun java.io.InputStream.readBounded(limit: Int): ByteArray {
    val output = java.io.ByteArrayOutputStream()
    val buffer = ByteArray(4096)
    while (true) {
        val count = read(buffer, 0, minOf(buffer.size, limit + 1 - output.size()))
        if (count == -1) break
        output.write(buffer, 0, count)
        check(output.size() <= limit) { "input_too_large" }
    }
    return output.toByteArray()
}

internal fun nativeValue(reply: String): JSONObject {
    val result = JSONObject(reply)
    if (!result.getBoolean("ok")) throw ConnectionFailure(result.getString("error"))
    return result.getJSONObject("value")
}
