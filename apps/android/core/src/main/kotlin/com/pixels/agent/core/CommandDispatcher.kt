package com.pixels.agent.core

import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull

/** Serializes input, bounds pending work and checks pause again at dispatch time. */
class CommandDispatcher(
    private val backend: AutomationBackend,
    capacity: Int = 16,
    initiallyPaused: Boolean = false,
) {
    private val slots = Semaphore(capacity)
    private val input = Mutex()
    private val mutablePaused = MutableStateFlow(initiallyPaused)
    private val pauseGeneration = AtomicLong()
    val paused = mutablePaused.asStateFlow()

    fun setPaused(paused: Boolean) {
        if (paused && !mutablePaused.value) pauseGeneration.incrementAndGet()
        mutablePaused.value = paused
    }

    suspend fun execute(command: AutomationCommand): AutomationResult {
        val generation = pauseGeneration.get()
        if (mutablePaused.value) return AutomationResult.Failed(AutomationError.CONTROL_PAUSED)
        if (!slots.tryAcquire()) return AutomationResult.Failed(AutomationError.BUSY)
        try {
            if (command is AutomationCommand.Observe) return backend.observe()
            if (command is AutomationCommand.Query) return backend.query(command.query)
            if (command is AutomationCommand.WaitFor) return waitFor(command, generation)
            return input.withLock {
                if (mutablePaused.value || generation != pauseGeneration.get()) {
                    AutomationResult.Failed(AutomationError.CONTROL_PAUSED)
                } else {
                    // Once dispatched, cancellation must not release the input lane while
                    // the platform can still be executing a gesture.
                    withContext(NonCancellable) {
                        when (command) {
                            is AutomationCommand.TypeText ->
                                backend.typeText(command.reference, command.text)
                            is AutomationCommand.Control ->
                                backend.control(command.reference, command.action)
                            is AutomationCommand.Click -> backend.click(command.reference)
                            is AutomationCommand.LongClick -> backend.longClick(command.reference)
                            is AutomationCommand.Scroll ->
                                backend.scroll(command.reference, command.direction)
                            is AutomationCommand.LaunchApp -> backend.launchApp(command.packageName)
                            is AutomationCommand.SetText -> {
                                if (
                                    command.text.toByteArray(Charsets.UTF_8).size > 4096 ||
                                        command.text.contains('\u0000')
                                ) {
                                    AutomationResult.Failed(AutomationError.INVALID_ARGUMENT)
                                } else {
                                    backend.setText(command.reference, command.text)
                                }
                            }
                            is AutomationCommand.Touch -> backend.touch(command.gesture)
                            is AutomationCommand.Navigate -> backend.navigate(command.action)
                            AutomationCommand.Observe,
                            is AutomationCommand.Query,
                            is AutomationCommand.WaitFor ->
                                error("Read request entered the input queue")
                        }
                    }
                }
            }
        } finally {
            slots.release()
        }
    }

    private suspend fun waitFor(
        command: AutomationCommand.WaitFor,
        generation: Long,
    ): AutomationResult {
        if (
            command.text.isEmpty() ||
                command.text.toByteArray().size > 1024 ||
                command.timeoutMs !in 100..20000 ||
                command.intervalMs !in 100..1000 ||
                command.intervalMs > command.timeoutMs
        ) {
            return AutomationResult.Failed(AutomationError.INVALID_ARGUMENT)
        }
        return withTimeoutOrNull(command.timeoutMs) {
            var matched: AutomationResult? = null
            while (matched == null) {
                if (mutablePaused.value || generation != pauseGeneration.get()) {
                    return@withTimeoutOrNull AutomationResult.Failed(AutomationError.CONTROL_PAUSED)
                }
                val result = backend.query(ObservationQuery(textContains = command.text))
                if (result !is AutomationResult.Observed) return@withTimeoutOrNull result
                val found = result.screen.elements.any { it.name?.contains(command.text) == true }
                if (found == command.present && (found || !result.screen.truncated))
                    matched = result
                else delay(command.intervalMs)
            }
            matched
        } ?: AutomationResult.Failed(AutomationError.CONDITION_TIMEOUT)
    }
}
