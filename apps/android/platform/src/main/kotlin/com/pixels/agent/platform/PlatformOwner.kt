package com.pixels.agent.platform

import com.pixels.agent.platform.capture.ProjectionCaptureProvider

/** Application composition root supplies the service dependency without a global locator. */
interface PlatformOwner {
    val automationBackend: AndroidAutomationBackend
    val projectionCapture: ProjectionCaptureProvider
}
