package com.pixels.agent.core

data class DeviceEnvironment(
    val networkAvailable: Boolean = false,
    val screenLocked: Boolean = false,
)

/** Local prerequisites and transport readiness are separate from a remote operator connecting. */
enum class ControlAvailability {
    PREPARING,
    CORE_UNAVAILABLE,
    SETTINGS_UNAVAILABLE,
    PERMISSION_REQUIRED,
    PAUSED,
    SCREEN_LOCKED,
    OFFLINE,
    DISCONNECTED,
    CONNECTING,
    RECONNECTING,
    STOPPING,
    CONNECTION_FAILED,
    CHANNEL_UNAVAILABLE,
    READY;

    companion object {
        fun resolve(
            nativeChecked: Boolean,
            nativeLoaded: Boolean,
            settingsLoaded: Boolean,
            settingsFailed: Boolean,
            serviceReady: Boolean,
            paused: Boolean,
            environment: DeviceEnvironment,
            connection: ConnectionStatus,
        ): ControlAvailability =
            when {
                !nativeChecked -> PREPARING
                !nativeLoaded -> CORE_UNAVAILABLE
                settingsFailed -> SETTINGS_UNAVAILABLE
                !settingsLoaded -> PREPARING
                paused -> PAUSED
                !serviceReady -> PERMISSION_REQUIRED
                environment.screenLocked -> SCREEN_LOCKED
                connection.phase == ConnectionPhase.STOPPING -> STOPPING
                connection.phase == ConnectionPhase.FAILED -> CONNECTION_FAILED
                connection.phase == ConnectionPhase.DISCONNECTED -> DISCONNECTED
                !environment.networkAvailable -> OFFLINE
                connection.phase == ConnectionPhase.RECONNECTING -> RECONNECTING
                connection.phase != ConnectionPhase.AUTHENTICATED -> CONNECTING
                !connection.remoteReady -> CHANNEL_UNAVAILABLE
                else -> READY
            }
    }
}
