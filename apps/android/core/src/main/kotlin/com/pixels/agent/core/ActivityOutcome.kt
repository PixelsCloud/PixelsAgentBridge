package com.pixels.agent.core

enum class ActivityOutcome {
    COMPLETED,
    INCOMPLETE,
    UNSUPPORTED,
    PENDING,
    DISPATCHED,
}

/** Acknowledgement and a completed operation are deliberately different states. */
fun activityOutcome(state: String, error: String, outcome: String): ActivityOutcome =
    when {
        error.contains("unsupported", true) || error.endsWith("_action_unavailable") ->
            ActivityOutcome.UNSUPPORTED
        error.isNotEmpty() ||
            state in setOf("failed", "cancelled", "rejected", "interrupted", "unconfirmed") ||
            outcome in setOf("rejected", "unconfirmed", "timed_out") -> ActivityOutcome.INCOMPLETE
        outcome == "dispatched" -> ActivityOutcome.DISPATCHED
        state in setOf("completed", "succeeded") -> ActivityOutcome.COMPLETED
        else -> ActivityOutcome.PENDING
    }
