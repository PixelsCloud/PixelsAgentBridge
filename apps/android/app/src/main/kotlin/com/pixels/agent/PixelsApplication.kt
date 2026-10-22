package com.pixels.agent

import android.app.Application
import com.pixels.agent.core.CommandDispatcher
import com.pixels.agent.platform.AndroidAutomationBackend
import com.pixels.agent.platform.DeviceEnvironmentMonitor
import com.pixels.agent.platform.PlatformOwner
import com.pixels.agent.platform.capture.ProjectionCaptureProvider
import com.pixels.agent.platform.capture.ScreenCaptureFactory
import com.pixels.agent.platform.storage.AndroidFileStorage
import com.pixels.agent.platform.system.AndroidSystemQueries
import com.pixels.agent.transport.AndroidConnectionRepository
import com.pixels.agent.transport.AndroidControlIntentRepository
import com.pixels.agent.transport.NativeBridge
import com.pixels.agent.transport.NativeFilePublisher
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch

class PixelsApplication : Application(), PlatformOwner {
    val remoteControlAllowed = AtomicBoolean(false)
    override val automationBackend by lazy { AndroidAutomationBackend(this) }
    val environment by lazy { DeviceEnvironmentMonitor(this) }
    override val projectionCapture = ProjectionCaptureProvider()
    val screenCapture by lazy { ScreenCaptureFactory.create(automationBackend, projectionCapture) }
    val dispatcher by lazy { CommandDispatcher(automationBackend, initiallyPaused = true) }
    val nativeBridge by lazy { NativeBridge(this) }
    internal val productPreferences by lazy { ProductPreferences(noBackupFilesDir) }
    val fileStorage by lazy { AndroidFileStorage(this, NativeFilePublisher(nativeBridge)) }
    val controlIntent by lazy {
        AndroidControlIntentRepository(noBackupFilesDir, nativeBridge, dispatcher::setPaused)
    }
    val connection: AndroidConnectionRepository by lazy {
        AndroidConnectionRepository(
            noBackupFilesDir,
            nativeBridge,
            RemoteExecutionAdapter(
                this,
                RemoteDocumentAdapter(
                    this,
                    fileStorage,
                    RemoteSystemAdapter(
                        AndroidSystemQueries(
                            this,
                            fileStorage::exportForViewing,
                            fileStorage::packageSnapshot,
                            automationBackend,
                            dispatcher,
                        ) {
                            remoteControlAllowed.get() &&
                                connection.status.value.remoteReady &&
                                !dispatcher.paused.value
                        },
                        RemoteFileAdapter(
                            fileStorage,
                            RemoteAutomationAdapter(
                                dispatcher,
                                screenCapture,
                                getSystemService(android.app.KeyguardManager::class.java),
                                getSystemService(android.os.PowerManager::class.java),
                                {
                                    remoteControlAllowed.get() &&
                                        connection.status.value.remoteReady
                                },
                            ),
                            { remoteControlAllowed.get() && connection.status.value.remoteReady },
                            { dispatcher.paused.value },
                            { connection.isStorageGenerationActive(it) },
                        ),
                        { remoteControlAllowed.get() && connection.status.value.remoteReady },
                        { dispatcher.paused.value },
                    ),
                    { remoteControlAllowed.get() && connection.status.value.remoteReady },
                    { dispatcher.paused.value },
                ),
                { remoteControlAllowed.get() && connection.status.value.remoteReady },
                { dispatcher.paused.value },
            ),
            fileStorage.stagingDirectory,
        )
    }

    override fun onCreate() {
        super.onCreate()
        // The isolated parser must not initialize credentials, transport or UI state.
        if (getProcessName() != packageName) return
        CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate).launch {
            try {
                controlIntent.load()
            } catch (_: Exception) {
                // The repository keeps input paused and the home screen offers retry.
            }
        }
    }
}
