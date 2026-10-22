package com.pixels.agent.core

import org.junit.Assert.assertEquals
import org.junit.Test

class ControlAvailabilityTest {
    private val authenticated = ConnectionStatus(ConnectionPhase.AUTHENTICATED, remoteReady = true)

    @Test
    fun transportAloneDoesNotMakeScreenControlReady() {
        assertEquals(ControlAvailability.PERMISSION_REQUIRED, resolve(serviceReady = false))
        assertEquals(ControlAvailability.PAUSED, resolve(paused = true))
        assertEquals(ControlAvailability.SCREEN_LOCKED, resolve(locked = true))
        assertEquals(ControlAvailability.READY, resolve())
    }

    @Test
    fun localPauseRemainsVisibleWhenPermissionAndNetworkDisappear() {
        assertEquals(
            ControlAvailability.PAUSED,
            resolve(paused = true, serviceReady = false, online = false),
        )
    }

    @Test
    fun distinguishesOfflineRetryAndStoppedConnections() {
        assertEquals(ControlAvailability.OFFLINE, resolve(online = false))
        assertEquals(
            ControlAvailability.RECONNECTING,
            resolve(connection = ConnectionStatus(ConnectionPhase.RECONNECTING)),
        )
        assertEquals(
            ControlAvailability.DISCONNECTED,
            resolve(online = false, connection = ConnectionStatus()),
        )
        assertEquals(
            ControlAvailability.CONNECTION_FAILED,
            resolve(online = false, connection = ConnectionStatus(ConnectionPhase.FAILED)),
        )
        assertEquals(
            ControlAvailability.STOPPING,
            resolve(online = false, connection = ConnectionStatus(ConnectionPhase.STOPPING)),
        )
    }

    @Test
    fun authenticatedControlWithoutDeviceChannelIsNotReady() {
        assertEquals(
            ControlAvailability.CHANNEL_UNAVAILABLE,
            resolve(connection = authenticated.copy(remoteReady = false)),
        )
        assertEquals(
            ControlAvailability.CONNECTING,
            resolve(connection = ConnectionStatus(ConnectionPhase.REGISTERING)),
        )
    }

    @Test
    fun unreadableConfigurationAndNativeFailureAreNotPermissionProblems() {
        assertEquals(ControlAvailability.PREPARING, resolve(nativeChecked = false))
        assertEquals(ControlAvailability.CORE_UNAVAILABLE, resolve(nativeLoaded = false))
        assertEquals(ControlAvailability.SETTINGS_UNAVAILABLE, resolve(settingsFailed = true))
        assertEquals(ControlAvailability.PREPARING, resolve(settingsLoaded = false))
    }

    private fun resolve(
        nativeChecked: Boolean = true,
        nativeLoaded: Boolean = true,
        settingsLoaded: Boolean = true,
        settingsFailed: Boolean = false,
        serviceReady: Boolean = true,
        paused: Boolean = false,
        online: Boolean = true,
        locked: Boolean = false,
        connection: ConnectionStatus = authenticated,
    ) =
        ControlAvailability.resolve(
            nativeChecked,
            nativeLoaded,
            settingsLoaded,
            settingsFailed,
            serviceReady,
            paused,
            DeviceEnvironment(online, locked),
            connection,
        )
}
