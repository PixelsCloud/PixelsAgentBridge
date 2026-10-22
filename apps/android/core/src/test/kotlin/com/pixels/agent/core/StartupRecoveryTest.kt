package com.pixels.agent.core

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class StartupRecoveryTest {
    @Test
    fun recoveryRequiresReadableIntentAndEnabledStartup() {
        val active = ControlIntentState(loaded = true, paused = false, connectionRequested = true)
        assertTrue(shouldRestoreConnection(true, active))
        assertFalse(shouldRestoreConnection(false, active))
        assertFalse(shouldRestoreConnection(true, active.copy(loaded = false)))
        assertFalse(shouldRestoreConnection(true, active.copy(error = "unreadable")))
        assertFalse(shouldRestoreConnection(true, ControlIntentState()))
    }

    @Test
    fun explicitStopSurvivesRebootButPausedConnectionsCanReconnect() {
        assertFalse(shouldRestoreConnection(true, ControlIntentState(loaded = true)))
        assertTrue(
            shouldRestoreConnection(
                true,
                ControlIntentState(loaded = true, paused = true, connectionRequested = true),
            )
        )
    }
}
