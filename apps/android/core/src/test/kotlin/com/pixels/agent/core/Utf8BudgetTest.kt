package com.pixels.agent.core

import org.junit.Assert.assertEquals
import org.junit.Test

class Utf8BudgetTest {
    @Test
    fun chineseAndSupplementaryCharactersStayWholeAtTheWireBoundary() {
        assertEquals("中🙂", boundedUtf8("中🙂文", 7))
        assertEquals("中", boundedUtf8("中🙂文", 6))
        assertEquals("a", boundedUtf8("a🙂", 4))
        assertEquals("", boundedUtf8("中", 2))
        assertEquals("x".repeat(4096), boundedUtf8("x".repeat(5000), 4096))
    }
}
