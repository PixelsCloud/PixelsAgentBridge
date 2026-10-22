package com.pixels.agent.core

/** Bounds external text without cutting a UTF-8 code point or a UTF-16 surrogate pair. */
fun boundedUtf8(value: String, maxBytes: Int): String {
    var end = 0
    var used = 0
    while (end < value.length) {
        val point = Character.codePointAt(value, end)
        val size =
            when {
                point <= 0x7f -> 1
                point <= 0x7ff -> 2
                point <= 0xffff -> 3
                else -> 4
            }
        if (used + size > maxBytes) break
        used += size
        end += Character.charCount(point)
    }
    return value.substring(0, end)
}
