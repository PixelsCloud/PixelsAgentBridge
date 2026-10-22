package com.pixels.agent.core

import kotlinx.coroutines.flow.StateFlow

data class ConnectionSettings(
    val controlUrl: String = "",
    val deviceName: String = "Pixels Android",
    val trustedCaPem: String? = null,
    val relayUrls: List<String> = emptyList(),
)

enum class ConnectionPhase {
    DISCONNECTED,
    REGISTERING,
    CONNECTING,
    AUTHENTICATED,
    RECONNECTING,
    STOPPING,
    FAILED,
}

data class ConnectionStatus(
    val phase: ConnectionPhase = ConnectionPhase.DISCONNECTED,
    val deviceCode: String? = null,
    val generation: Long = 0,
    val error: String? = null,
    val remoteReady: Boolean = false,
) {
    val active: Boolean
        get() = phase != ConnectionPhase.DISCONNECTED && phase != ConnectionPhase.FAILED
}

interface ConnectionRepository {
    val settings: StateFlow<ConnectionSettings>
    val status: StateFlow<ConnectionStatus>
    val accessPassword: StateFlow<String?>

    suspend fun loadSettings()

    suspend fun saveSettings(settings: ConnectionSettings)

    /** Changes the gate for new sessions while preserving already authenticated sessions. */
    suspend fun rotateAccessPassword() {
        throw UnsupportedOperationException("password_rotation_unavailable")
    }

    /** Runs until cancelled by the user-visible service; cleanup must finish before returning. */
    suspend fun runConnection()
}

data class RemotePlatformResult(val metadata: String, val jpeg: ByteArray = byteArrayOf())

fun interface RemoteCommandHandler {
    suspend fun execute(command: String): RemotePlatformResult
}
