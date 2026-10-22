package com.pixels.agent.core

import org.junit.Assert.assertEquals
import org.junit.Test

class ActivityOutcomeTest {
    @Test
    fun unsupportedIsNotPermissionFailureOrRetry() {
        assertEquals(
            ActivityOutcome.UNSUPPORTED,
            activityOutcome("failed", "android_desktop_action_unavailable", ""),
        )
        assertEquals(
            ActivityOutcome.UNSUPPORTED,
            activityOutcome("failed", "unsupported_on_android", ""),
        )
        assertEquals(
            ActivityOutcome.INCOMPLETE,
            activityOutcome("failed", "permission_required", ""),
        )
        assertEquals(
            ActivityOutcome.INCOMPLETE,
            activityOutcome("failed", "service_unavailable", ""),
        )
    }

    @Test
    fun acknowledgedActionDoesNotClaimConfirmedCompletion() {
        assertEquals(ActivityOutcome.DISPATCHED, activityOutcome("completed", "", "dispatched"))
        for (outcome in listOf("unconfirmed", "rejected", "timed_out")) {
            assertEquals(ActivityOutcome.INCOMPLETE, activityOutcome("completed", "", outcome))
        }
        assertEquals(ActivityOutcome.INCOMPLETE, activityOutcome("unconfirmed", "", ""))
    }

    @Test
    fun unknownAndRunningStatesNeverBecomeCompleted() {
        for (state in listOf("", "running", "accepted", "cancel_requested", "new_future_state")) {
            assertEquals(ActivityOutcome.PENDING, activityOutcome(state, "", ""))
        }
        assertEquals(ActivityOutcome.COMPLETED, activityOutcome("succeeded", "", ""))
        assertEquals(ActivityOutcome.COMPLETED, activityOutcome("completed", "", "removed"))
    }
}
