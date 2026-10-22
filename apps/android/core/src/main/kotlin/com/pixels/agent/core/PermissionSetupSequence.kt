package com.pixels.agent.core

enum class SetupPermission {
    NOTIFICATIONS,
    NOTIFICATION_ACCESS,
    ACCESSIBILITY,
    INSTALL_APPS,
    FILES,
    SCREEN_CAPTURE,
}

/** One attempt per missing permission, including when a system page returns without a grant. */
class PermissionSetupSequence(attempted: Set<SetupPermission> = emptySet()) {
    private val attempted = attempted.toMutableSet()

    fun next(ready: Set<SetupPermission>): SetupPermission? =
        SetupPermission.entries
            .firstOrNull { it !in ready && it !in attempted }
            ?.also { attempted.add(it) }

    fun snapshot(): Set<SetupPermission> = attempted.toSet()

    fun retry() = attempted.clear()
}
