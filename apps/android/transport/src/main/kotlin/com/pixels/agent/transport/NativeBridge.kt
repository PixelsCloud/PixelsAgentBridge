package com.pixels.agent.transport

import android.content.Context

/** JNI lifecycle belongs to this adapter; callers receive no native pointers. */
class NativeBridge(context: Context) {
    init {
        System.loadLibrary("pab_android_agent")
        nativeValue(initialize(context.applicationContext))
    }

    external fun probe(): String

    external fun defaultSettings(): String

    external fun encodeSettings(input: String): String

    external fun decodeSettings(toml: String): String

    external fun decodeUserIntent(toml: String): String

    external fun encodeUserIntent(input: String): String

    external fun register(toml: String, identity: ByteArray): String

    external fun start(
        toml: String,
        identity: ByteArray,
        registration: String,
        options: String,
    ): String

    private external fun initialize(context: Context): String

    external fun passwordHash(password: String): String

    external fun updateAccessPassword(hash: String, version: Long): String

    external fun renameDevice(
        toml: String,
        identity: ByteArray,
        registration: String,
        name: String,
    ): String

    external fun pollPlatform(): String

    external fun completePlatform(ticket: String, output: String, jpeg: ByteArray): String

    external fun status(): String

    external fun recentActivity(directory: String): String

    external fun stop(): String

    external fun publishFile(source: String, destination: String, replace: Boolean): String
}
