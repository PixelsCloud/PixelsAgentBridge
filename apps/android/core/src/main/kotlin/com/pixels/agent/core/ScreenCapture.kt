package com.pixels.agent.core

sealed interface CaptureResult {
    data class Frame(val width: Int, val height: Int, val jpeg: ByteArray) : CaptureResult

    data class Failed(val reason: CaptureFailure) : CaptureResult
}

enum class CaptureFailure {
    CONSENT_REQUIRED,
    SERVICE_UNAVAILABLE,
    FRAME_UNAVAILABLE,
    PROTECTED_OR_REJECTED,
}

interface ScreenCaptureProvider {
    suspend fun capture(): CaptureResult
}
