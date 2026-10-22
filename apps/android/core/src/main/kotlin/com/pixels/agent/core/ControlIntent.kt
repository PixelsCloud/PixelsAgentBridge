package com.pixels.agent.core

import kotlinx.coroutines.flow.StateFlow

data class ControlIntentState(
    val loaded: Boolean = false,
    val paused: Boolean = true,
    val connectionRequested: Boolean = false,
    val error: String? = null,
)

interface ControlIntentRepository {
    val state: StateFlow<ControlIntentState>

    suspend fun load()

    suspend fun setPaused(paused: Boolean)

    suspend fun setConnectionRequested(requested: Boolean)
}
