package com.pixels.agent.core

/** Reconnect a previously requested connection without changing its input pause state. */
fun shouldRestoreConnection(startOnBoot: Boolean, intent: ControlIntentState): Boolean =
    startOnBoot && intent.loaded && intent.error == null && intent.connectionRequested
