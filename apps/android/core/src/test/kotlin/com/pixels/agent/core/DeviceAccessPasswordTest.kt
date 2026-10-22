package com.pixels.agent.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class DeviceAccessPasswordTest {
    @Test
    fun generatesEightUnambiguousUppercaseCharactersLikeDesktop() {
        val passwords = List(128) { DeviceAccessPassword.generate() }
        for (password in passwords) {
            assertEquals(8, password.length)
            assertTrue(password.matches(Regex("[A-HJ-NP-Z2-9]{8}")))
        }
        assertTrue(passwords.toSet().size > 1)
    }
}
