package com.pixels.agent.core

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.async
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Test

@OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
class CommandDispatcherTest {
    @Test
    fun pauseRejectsWaitingInputBeforePlatformDispatch() = runTest {
        val backend = FakeBackend()
        val dispatcher = CommandDispatcher(backend)
        val first = async { dispatcher.execute(AutomationCommand.Click("first")) }
        runCurrent()
        val waiting = async { dispatcher.execute(AutomationCommand.Click("waiting")) }
        runCurrent()
        dispatcher.setPaused(true)
        dispatcher.setPaused(false)
        backend.release.complete(Unit)
        first.await()
        assertEquals(AutomationResult.Failed(AutomationError.CONTROL_PAUSED), waiting.await())
        assertEquals(listOf("first"), backend.dispatched)
    }

    @Test
    fun cancellingDispatchedInputKeepsLaneUntilPlatformReturns() = runTest {
        val backend = FakeBackend()
        val dispatcher = CommandDispatcher(backend)
        val first = async { dispatcher.execute(AutomationCommand.Click("first")) }
        runCurrent()
        first.cancel()
        val waiting = async { dispatcher.execute(AutomationCommand.Click("waiting")) }
        runCurrent()
        assertEquals(listOf("first"), backend.dispatched)
        backend.release.complete(Unit)
        first.join()
        waiting.await()
        assertEquals(listOf("first", "waiting"), backend.dispatched)
    }

    @Test
    fun cancelledWaiterReleasesCapacityWithoutDispatching() = runTest {
        val backend = FakeBackend()
        val dispatcher = CommandDispatcher(backend, capacity = 2)
        val first = async { dispatcher.execute(AutomationCommand.Click("first")) }
        runCurrent()
        val waiting = async { dispatcher.execute(AutomationCommand.Click("waiting")) }
        runCurrent()
        assertEquals(
            AutomationResult.Failed(AutomationError.BUSY),
            dispatcher.execute(AutomationCommand.Click("overflow")),
        )
        waiting.cancel()
        waiting.join()
        backend.release.complete(Unit)
        first.await()
        dispatcher.execute(AutomationCommand.Click("last"))
        assertEquals(listOf("first", "last"), backend.dispatched)
    }

    @Test
    fun waitingObservesWithoutBlockingOrRepeatingInput() = runTest {
        val backend = FakeBackend()
        val dispatcher = CommandDispatcher(backend)
        val waiting = async {
            dispatcher.execute(AutomationCommand.WaitFor("ready", true, 2000, 100))
        }
        runCurrent()
        val click = async { dispatcher.execute(AutomationCommand.Click("once")) }
        runCurrent()
        assertEquals(listOf("once"), backend.dispatched)
        advanceTimeBy(300)
        backend.names = listOf("ready")
        backend.release.complete(Unit)
        click.await()
        assertEquals(1, (waiting.await() as AutomationResult.Observed).screen.elements.size)
        assertEquals(listOf("once"), backend.dispatched)
    }

    @Test
    fun truncatedObservationCannotProveAbsenceAndPauseInvalidatesWaiting() = runTest {
        val backend = FakeBackend()
        backend.truncated = true
        val dispatcher = CommandDispatcher(backend)
        assertEquals(
            AutomationResult.Failed(AutomationError.CONDITION_TIMEOUT),
            dispatcher.execute(AutomationCommand.WaitFor("missing", false, 300, 100)),
        )
        val waiting = async {
            dispatcher.execute(AutomationCommand.WaitFor("ready", true, 2000, 100))
        }
        runCurrent()
        dispatcher.setPaused(true)
        dispatcher.setPaused(false)
        assertEquals(AutomationResult.Failed(AutomationError.CONTROL_PAUSED), waiting.await())
        assertEquals(emptyList<String>(), backend.dispatched)
    }

    private class FakeBackend : AutomationBackend {
        val release = CompletableDeferred<Unit>()
        val dispatched = mutableListOf<String>()
        var names = emptyList<String>()
        var truncated = false

        override suspend fun query(query: ObservationQuery): AutomationResult =
            AutomationResult.Observed(
                ScreenObservation(
                    1,
                    ScreenGeometry(100, 100, 0),
                    names.map { name ->
                        ScreenElement(
                            name,
                            "label",
                            name,
                            Bounds(0, 0, 10, 10),
                            true,
                            false,
                            false,
                            false,
                        )
                    },
                    truncated,
                )
            )

        override suspend fun click(reference: String): AutomationResult {
            dispatched.add(reference)
            release.await()
            return AutomationResult.Dispatched(true)
        }

        override suspend fun observe(): AutomationResult = error("unused")

        override suspend fun setText(reference: String, text: String): AutomationResult =
            error("unused")

        override suspend fun touch(gesture: TouchGesture): AutomationResult = error("unused")

        override suspend fun navigate(action: NavigationAction): AutomationResult = error("unused")
    }
}
