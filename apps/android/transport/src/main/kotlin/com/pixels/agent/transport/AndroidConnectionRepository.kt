package com.pixels.agent.transport

import com.pixels.agent.core.ConnectionPhase
import com.pixels.agent.core.ConnectionRepository
import com.pixels.agent.core.ConnectionSettings
import com.pixels.agent.core.ConnectionStatus
import com.pixels.agent.core.RemoteCommandHandler
import java.io.File
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.json.JSONObject

private fun isSystemWait(command: JSONObject): Boolean {
    if (command.optString("domain") != "system") return false
    val query = command.optJSONObject("query") ?: return false
    val action = query.optString("action")
    val operation = query.optJSONObject("query") ?: return false
    return (action == "notifications" && operation.optString("operation") == "wait") ||
        (action == "desktop" &&
            operation.optString("operation") == "ui" &&
            operation.optJSONObject("query")?.optString("operation") == "wait")
}

class AndroidConnectionRepository(
    private val directory: File,
    private val native: NativeBridge,
    private val handler: RemoteCommandHandler? = null,
    private val storageStagingDirectory: File? = null,
) : ConnectionRepository {
    private val store = ConnectionSettingsStore(directory, native)
    private val credentials = EndpointCredentialStore(File(directory, "credentials"))
    private val lifecycle = Mutex()
    private val credentialsLock = Mutex()
    private var lastRenameMs: Long? = null
    private val currentSettings = MutableStateFlow(ConnectionSettings())
    private val currentStatus = MutableStateFlow(ConnectionStatus())
    override val settings = currentSettings.asStateFlow()
    override val status = currentStatus.asStateFlow()
    private val password = MutableStateFlow<String?>(null)
    private val storageLane = StorageWorkLane()

    fun isStorageGenerationActive(epoch: Long): Boolean = storageLane.current(epoch)

    override val accessPassword = password.asStateFlow()

    override suspend fun loadSettings(): Unit =
        withContext(Dispatchers.IO) {
            if (currentStatus.value.active) return@withContext
            credentialsLock.withLock {
                if (currentStatus.value.active) return@withLock
                val config = store.read() ?: store.defaults()
                currentSettings.value = config.settings
                if (config.settings.controlUrl.isNotBlank()) {
                    password.value =
                        credentials.access(config.credentialScope, native).getString("password")
                }
            }
        }

    override suspend fun saveSettings(settings: ConnectionSettings) =
        withContext(Dispatchers.IO) {
            credentialsLock.withLock {
                val previous = store.read() ?: store.defaults()
                store.validate(settings)
                val nameOnly =
                    settings.copy(deviceName = previous.settings.deviceName) == previous.settings
                check(nameOnly || !currentStatus.value.active) { "connection_active" }
                if (nameOnly && settings.deviceName != previous.settings.deviceName) {
                    check(
                        settings.deviceName.trim().length in 1..80 &&
                            settings.deviceName.none { it.isISOControl() }
                    ) {
                        "invalid_device_name"
                    }
                    val now = android.os.SystemClock.elapsedRealtime()
                    if (lastRenameMs?.let { now - it < 10000 } == true)
                        throw ConnectionFailure("device_rename_rate_limited")
                    val registration = credentials.registration(previous.credentialScope)
                    if (registration != null) {
                        val seed = credentials.seed(previous.credentialScope, allowCreate = false)
                        try {
                            nativeValue(
                                native.renameDevice(
                                    previous.toml,
                                    seed,
                                    registration,
                                    settings.deviceName,
                                )
                            )
                        } finally {
                            seed.fill(0)
                        }
                    }
                }
                currentSettings.value = store.save(settings).settings
                if (nameOnly && settings.deviceName != previous.settings.deviceName) {
                    lastRenameMs = android.os.SystemClock.elapsedRealtime()
                }
            }
        }

    override suspend fun rotateAccessPassword(): Unit =
        withContext(Dispatchers.IO) {
            credentialsLock.withLock {
                val config = store.read() ?: store.defaults()
                val access = credentials.rotateAccess(config.credentialScope, native)
                password.value = access.getString("password")
            }
        }

    override suspend fun runConnection() {
        // A duplicate start cannot take ownership of, or stop, the running session.
        if (!lifecycle.tryLock()) return
        var failed = false
        var nativeStarted = false
        val storageEpoch = storageLane.begin()
        val documentJobs = SupervisorJob(currentCoroutineContext()[Job])
        val documentScope = CoroutineScope(Dispatchers.IO + documentJobs)
        val documentSlots = Semaphore(4)
        try {
            currentStatus.value = ConnectionStatus(ConnectionPhase.REGISTERING)
            withContext(Dispatchers.IO) {
                credentialsLock.withLock {
                    val config = store.read() ?: throw ConnectionFailure("settings_required")
                    currentSettings.value = config.settings
                    val storedRegistration = credentials.registration(config.credentialScope)
                    val seed =
                        credentials.seed(
                            config.credentialScope,
                            allowCreate = storedRegistration == null,
                        )
                    try {
                        val registration =
                            storedRegistration
                                ?: nativeValue(native.register(config.toml, seed)).toString().also {
                                    // Durable identity metadata precedes authentication. A crash
                                    // never
                                    // forces an already claimed device through open registration
                                    // again.
                                    credentials.saveRegistration(config.credentialScope, it)
                                }
                        var options = ""
                        if (config.settings.relayUrls.isNotEmpty()) {
                            check(handler != null) { "platform_handler_required" }
                            val access = credentials.access(config.credentialScope, native)
                            password.value = access.getString("password")
                            val scopeHash =
                                java.security.MessageDigest.getInstance("SHA-256")
                                    .digest(config.credentialScope.toByteArray())
                                    .joinToString("") { "%02x".format(it.toInt() and 0xff) }
                            val runtime = File(directory, "runtime/$scopeHash").apply { mkdirs() }
                            options =
                                JSONObject()
                                    .put(
                                        "database_file",
                                        File(runtime, "mobile.sqlite").absolutePath,
                                    )
                                    .put("password_hash", access.getString("hash"))
                                    .put("password_version", access.getLong("version"))
                                    .put("android_release", android.os.Build.VERSION.RELEASE)
                                    .put("android_sdk", android.os.Build.VERSION.SDK_INT)
                                    .put(
                                        "storage_staging_directory",
                                        storageStagingDirectory?.absolutePath,
                                    )
                                    .toString()
                        }
                        nativeValue(native.start(config.toml, seed, registration, options))
                        // Assign inside IO before its cancellable return to the owner context.
                        nativeStarted = true
                    } finally {
                        seed.fill(0)
                    }
                }
            }
            while (currentCoroutineContext().isActive) {
                val value = withContext(Dispatchers.IO) { nativeValue(native.status()) }
                if (value.optString("error") in setOf("authentication", "configuration")) {
                    throw ConnectionFailure("control_credentials_or_configuration_rejected")
                }
                val phase =
                    when (value.getString("phase")) {
                        "authenticated" -> ConnectionPhase.AUTHENTICATED
                        "reconnecting" -> ConnectionPhase.RECONNECTING
                        "connecting",
                        "disconnected" -> ConnectionPhase.CONNECTING
                        else -> throw ConnectionFailure("control_stopped")
                    }
                currentStatus.value =
                    ConnectionStatus(
                        phase,
                        value.optString("device_code").takeIf { it.isNotEmpty() },
                        value.optLong("generation"),
                        value.optString("error").takeIf { it.isNotEmpty() && it != "null" },
                        value.optBoolean("remote_ready"),
                    )
                // Only the connection owner polls. Platform objects remain in Kotlin.
                if (handler != null) {
                    for (index in 0 until 16) {
                        val envelope =
                            withContext(Dispatchers.IO) { JSONObject(native.pollPlatform()) }
                        if (!envelope.optBoolean("ok"))
                            throw ConnectionFailure("platform_mailbox_failed")
                        val job = envelope.optJSONObject("value") ?: break
                        val command = job.getJSONObject("command")
                        if (command.optString("domain") == "storage") {
                            val submitted =
                                storageLane.submit(storageEpoch) { current ->
                                    var delivered = false
                                    try {
                                        command.put("storage_epoch", storageEpoch)
                                        val result = handler.execute(command.toString())
                                        if (current()) {
                                            delivered =
                                                JSONObject(
                                                        native.completePlatform(
                                                            job.getString("ticket"),
                                                            result.metadata,
                                                            result.jpeg,
                                                        )
                                                    )
                                                    .optBoolean("ok")
                                        }
                                    } catch (cancelled: CancellationException) {
                                        throw cancelled
                                    } catch (_: Exception) {
                                        if (current())
                                            runCatching {
                                                native.completePlatform(
                                                    job.getString("ticket"),
                                                    "{\"error\":\"storage_outcome_unconfirmed\"}",
                                                    byteArrayOf(),
                                                )
                                            }
                                    } finally {
                                        // A timed-out native snapshot may have been created late.
                                        // Delete only the exact UUID stage owned by this old
                                        // generation.
                                        if (
                                            !delivered && command.optString("action") == "snapshot"
                                        ) {
                                            val id = command.optString("staging_id")
                                            if (
                                                runCatching {
                                                        java.util.UUID.fromString(id).toString() ==
                                                            id
                                                    }
                                                    .getOrDefault(false)
                                            ) {
                                                storageStagingDirectory?.let {
                                                    File(it, "$id.part").delete()
                                                }
                                            }
                                        }
                                    }
                                }
                            if (!submitted) {
                                withContext(Dispatchers.IO) {
                                    native.completePlatform(
                                        job.getString("ticket"),
                                        "{\"error\":\"storage_busy\"}",
                                        byteArrayOf(),
                                    )
                                }
                            }
                        } else if (
                            command.optString("domain") == "document" || isSystemWait(command)
                        ) {
                            // Parsing owns a separate bounded lane; status polling, files,
                            // accessibility and heartbeat work continue while it runs.
                            if (documentSlots.tryAcquire()) {
                                documentScope.launch {
                                    try {
                                        val result = handler.execute(command.toString())
                                        native.completePlatform(
                                            job.getString("ticket"),
                                            result.metadata,
                                            result.jpeg,
                                        )
                                    } finally {
                                        documentSlots.release()
                                    }
                                }
                            } else {
                                withContext(Dispatchers.IO) {
                                    native.completePlatform(
                                        job.getString("ticket"),
                                        if (command.optString("domain") == "document")
                                            "{\"error\":\"document_busy\"}"
                                        else "{\"error\":\"system_busy\"}",
                                        byteArrayOf(),
                                    )
                                }
                            }
                        } else {
                            val result = handler.execute(command.toString())
                            withContext(Dispatchers.IO) {
                                // A timed-out ticket is never replayed; a late result is discarded.
                                native.completePlatform(
                                    job.getString("ticket"),
                                    result.metadata,
                                    result.jpeg,
                                )
                            }
                        }
                    }
                }
                delay(50)
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: Exception) {
            failed = true
            currentStatus.value =
                ConnectionStatus(
                    ConnectionPhase.FAILED,
                    error = (error as? ConnectionFailure)?.code ?: "local_storage_or_native_error",
                )
        } finally {
            storageLane.end(storageEpoch)
            currentStatus.value =
                currentStatus.value.copy(
                    phase = if (failed) ConnectionPhase.FAILED else ConnectionPhase.STOPPING,
                    remoteReady = false,
                )
            withContext(NonCancellable) { documentJobs.cancelAndJoin() }
            if (!failed)
                currentStatus.value = currentStatus.value.copy(phase = ConnectionPhase.STOPPING)
            try {
                // Keep the return dispatcher inside a non-cancelled context too. Otherwise
                // prompt cancellation on the IO return path looks like a shutdown failure.
                if (nativeStarted)
                    withContext(NonCancellable) {
                        withContext(Dispatchers.IO) { nativeValue(native.stop()) }
                    }
            } catch (_: Exception) {
                failed = true
                currentStatus.value =
                    ConnectionStatus(ConnectionPhase.FAILED, error = "shutdown_failed")
            } finally {
                if (!failed) currentStatus.value = ConnectionStatus()
                lifecycle.unlock()
            }
        }
    }
}
