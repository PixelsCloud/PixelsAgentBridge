package com.pixels.agent.core

import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

class TextInsertionTest {
    @Test
    fun delayedTextReadbackConfirmsWithoutRepeatingInsertion() = runTest {
        var reads = 0
        var moves = 0
        val result =
            confirmTextInsertionWhenReady(
                "hello",
                5,
                {
                    reads++
                    TextInsertionObservation(if (reads < 4) "old" else "hello", 5, 5)
                },
                { moves++ },
            )
        assertEquals(AutomationResult.Dispatched(true), result)
        assertEquals(4, reads)
        assertEquals(0, moves)
    }

    @Test
    fun missingTextReadbackStopsAtBoundAndNeverMovesCaret() = runTest {
        var reads = 0
        val result =
            confirmTextInsertionWhenReady(
                "hello",
                5,
                {
                    reads++
                    null
                },
            ) {
                error("No verified text")
            }
        assertEquals(AutomationResult.Failed(AutomationError.UNCONFIRMED), result)
        assertEquals(21, reads)
    }

    @Test
    fun matchingTextAndCaretNeedsNoSecondAction() {
        val result =
            confirmTextInsertion("hello", 5, { TextInsertionObservation("hello", 5, 5) }) {
                error("Caret already matches")
            }
        assertEquals(AutomationResult.Dispatched(true), result)
    }

    @Test
    fun missingOrDifferentTextCannotBeConfirmedByMovingCaret() {
        for (observation in listOf(null, TextInsertionObservation("old", 0, 0))) {
            val result =
                confirmTextInsertion("hello", 5, { observation }) {
                    error("Do not move the caret when text was not verified")
                }
            assertEquals(AutomationResult.Failed(AutomationError.UNCONFIRMED), result)
        }
    }

    @Test
    fun verifiedTextWithUnsupportedSelectionRemainsSuccessfulWithWarning() {
        var moves = 0
        val result =
            confirmTextInsertion("hello", 2, { TextInsertionObservation("hello", -1, -1) }) {
                moves++
            }
                as AutomationResult.Dispatched
        assertEquals(1, moves)
        assertTrue(result.platformAcknowledged)
        assertEquals(1, result.warnings.size)
    }

    @Test
    fun selectionFailureDoesNotTurnInsertedTextIntoFailure() {
        val result =
            confirmTextInsertion("hello", 2, { TextInsertionObservation("hello", 5, 5) }) {
                throw IllegalStateException("Selection unavailable")
            }
        assertTrue(result is AutomationResult.Dispatched)
        assertFalse((result as AutomationResult.Dispatched).warnings.isEmpty())
    }

    @Test
    fun verifiesSelectionReadbackAndDetectsConcurrentTextChanges() {
        for (changed in listOf(false, true)) {
            var moved = false
            val result =
                confirmTextInsertion(
                    "hello",
                    2,
                    {
                        if (!moved) TextInsertionObservation("hello", 5, 5)
                        else TextInsertionObservation(if (changed) "other" else "hello", 2, 2)
                    },
                ) {
                    moved = true
                }
            assertEquals(
                if (changed) AutomationResult.Failed(AutomationError.UNCONFIRMED)
                else AutomationResult.Dispatched(true),
                result,
            )
        }
    }
}
