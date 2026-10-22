package com.pixels.agent.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class PermissionSetupSequenceTest {
    @Test
    fun alreadyGrantedPermissionsAreNeverRequested() {
        assertNull(PermissionSetupSequence().next(SetupPermission.entries.toSet()))
        assertEquals(
            SetupPermission.INSTALL_APPS,
            PermissionSetupSequence()
                .next(SetupPermission.entries.toSet() - SetupPermission.INSTALL_APPS),
        )
    }

    @Test
    fun denialAndReturnFromSettingsAdvanceWithoutRepeatedPrompts() {
        val sequence = PermissionSetupSequence()
        assertEquals(
            SetupPermission.entries,
            List(SetupPermission.entries.size) { sequence.next(emptySet()) },
        )
        assertNull(sequence.next(emptySet()))
    }

    @Test
    fun recreationKeepsAttemptedStepsAndRetryRechecksCurrentGrants() {
        val first = PermissionSetupSequence()
        assertEquals(SetupPermission.NOTIFICATIONS, first.next(emptySet()))
        val restored = PermissionSetupSequence(first.snapshot())
        assertEquals(SetupPermission.NOTIFICATION_ACCESS, restored.next(emptySet()))
        restored.retry()
        assertEquals(
            SetupPermission.INSTALL_APPS,
            restored.next(
                setOf(
                    SetupPermission.NOTIFICATIONS,
                    SetupPermission.NOTIFICATION_ACCESS,
                    SetupPermission.ACCESSIBILITY,
                )
            ),
        )
    }
}
