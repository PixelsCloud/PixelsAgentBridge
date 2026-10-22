package com.pixels.agent.core

import java.security.SecureRandom

/** Matches the desktop's eight-character temporary access password. */
object DeviceAccessPassword {
    const val LENGTH = 8
    private const val ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789"
    private val random = SecureRandom()

    fun generate(): String =
        CharArray(LENGTH) { ALPHABET[random.nextInt(ALPHABET.length)] }.concatToString()
}
