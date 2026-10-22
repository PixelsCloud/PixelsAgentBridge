package com.pixels.agent

import android.util.Base64
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.pixels.agent.core.ConnectionPhase
import com.pixels.agent.core.ConnectionSettings
import com.pixels.agent.transport.AndroidConnectionRepository
import java.io.File
import java.util.UUID
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class ConnectionDeviceTest {
    @Test
    fun signedTlsRegistrationReconnectAndEncryptedIdentityRecovery() = runBlocking {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val arguments = InstrumentationRegistry.getArguments()
        val port = requireNotNull(arguments.getString("fixture_port")).toInt()
        require(port in 1024..65535)
        val ca =
            Base64.decode(requireNotNull(arguments.getString("fixture_ca")), Base64.DEFAULT)
                .toString(Charsets.UTF_8)
        val app = instrumentation.targetContext.applicationContext as PixelsApplication
        val directory = File(app.noBackupFilesDir, "control-tests/${UUID.randomUUID()}")
        val repo = AndroidConnectionRepository(directory, app.nativeBridge)
        val settings = ConnectionSettings("wss://localhost:$port/control", "Pixels 测试手机")
        repo.saveSettings(settings)
        repo.runConnection()
        assertEquals(ConnectionPhase.FAILED, repo.status.value.phase)
        assertEquals("registration_failed", repo.status.value.error)
        repo.saveSettings(settings.copy(trustedCaPem = ca))
        var session = launch { repo.runConnection() }
        try {
            withTimeout(15000) { repo.status.first { it.phase == ConnectionPhase.AUTHENTICATED } }
            assertEquals("987654321", repo.status.value.deviceCode)
            // Duplicate starts are harmless and cannot steal the existing native session.
            repo.runConnection()
            assertEquals(ConnectionPhase.AUTHENTICATED, repo.status.value.phase)
            // Fixture deliberately closes the first authenticated socket.
            withTimeout(15000) {
                repo.status.first {
                    it.phase == ConnectionPhase.AUTHENTICATED && it.generation >= 2
                }
            }
        } finally {
            session.cancelAndJoin()
        }
        assertEquals(ConnectionPhase.DISCONNECTED, repo.status.value.phase)
        val seedFile =
            requireNotNull(
                File(directory, "credentials").listFiles()?.single { it.name.endsWith("-seed.bin") }
            )
        val seedCiphertext = seedFile.readBytes()
        assertEquals(61, seedCiphertext.size) // version + nonce + encrypted seed + GCM tag
        assertFalse(File(directory, "settings.toml").readText().contains("endpoint_key"))
        val restored = AndroidConnectionRepository(directory, app.nativeBridge)
        restored.loadSettings()
        assertEquals("Pixels 测试手机", restored.settings.value.deviceName)
        session = launch { restored.runConnection() }
        try {
            withTimeout(15000) {
                restored.status.first { it.phase == ConnectionPhase.AUTHENTICATED }
            }
            assertEquals("987654321", restored.status.value.deviceCode)
        } finally {
            session.cancelAndJoin()
        }
        assertArrayEquals(seedCiphertext, seedFile.readBytes())
        // Authentication failure of local ciphertext must never generate a new identity.
        val corrupt =
            seedCiphertext.clone().also { it[it.lastIndex] = (it.last().toInt() xor 1).toByte() }
        seedFile.writeBytes(corrupt)
        restored.runConnection()
        assertEquals(ConnectionPhase.FAILED, restored.status.value.phase)
        assertArrayEquals(corrupt, seedFile.readBytes())
        assertTrue(app.nativeBridge.status().contains("disconnected"))
        delay(200)
    }
}
