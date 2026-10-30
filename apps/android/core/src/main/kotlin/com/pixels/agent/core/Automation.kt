package com.pixels.agent.core

data class ScreenGeometry(val width: Int, val height: Int, val rotation: Int)

data class Bounds(val left: Int, val top: Int, val right: Int, val bottom: Int)

data class ScreenElement(
    val reference: String,
    val role: String,
    val name: String?,
    val bounds: Bounds,
    val enabled: Boolean,
    val clickable: Boolean,
    val editable: Boolean,
    val protected: Boolean,
    val identifier: String? = null,
    val value: String? = null,
    val focused: Boolean = false,
    val checkable: Boolean = false,
    val checked: Boolean = false,
    val selected: Boolean = false,
    val actions: Set<Int> = emptySet(),
    val valueTruncated: Boolean = false,
)

data class ScreenObservation(
    val revision: Long,
    val geometry: ScreenGeometry,
    val elements: List<ScreenElement>,
    val truncated: Boolean,
    val packageName: String? = null,
    val windowId: Int = -1,
    val visitedCount: Int = elements.size,
)

enum class AutomationError {
    SERVICE_UNAVAILABLE,
    CONTROL_PAUSED,
    SCREEN_LOCKED,
    TARGET_STALE,
    UNSUPPORTED_ACTION,
    INVALID_ARGUMENT,
    BUSY,
    PLATFORM_REJECTED,
    UNCONFIRMED,
    CONDITION_TIMEOUT,
    APP_NOT_FOUND,
    APP_LAUNCH_BLOCKED,
    PRECONDITION_FAILED,
}

sealed interface AutomationResult {
    data class Observed(val screen: ScreenObservation) : AutomationResult

    /** Platform acknowledgement alone does not establish a business postcondition. */
    data class Dispatched(
        val platformAcknowledged: Boolean,
        val actionDispatched: Boolean = true,
        val warnings: List<String> = emptyList(),
    ) : AutomationResult

    data class Failed(val error: AutomationError) : AutomationResult
}

enum class NavigationAction {
    BACK,
    HOME,
    RECENTS,
}

data class TouchGesture(
    val revision: Long,
    val fromX: Int,
    val fromY: Int,
    val toX: Int,
    val toY: Int,
    val durationMs: Long,
)

enum class ScrollDirection {
    FORWARD,
    BACKWARD,
    UP,
    DOWN,
    LEFT,
    RIGHT,
}

data class ObservationQuery(
    val rootReference: String? = null,
    val textContains: String? = null,
    val role: String? = null,
    val maxNodes: Int = 128,
    val maxDepth: Int = 32,
    val maxVisited: Int = 2048,
    val timeoutMs: Long = 0,
    val identifier: String? = null,
)

sealed interface AutomationCommand {
    data class TypeText(val reference: String, val text: String) : AutomationCommand

    data class Control(val reference: String, val action: ControlAction) : AutomationCommand

    data object Observe : AutomationCommand

    data class Query(val query: ObservationQuery) : AutomationCommand

    data class WaitFor(
        val text: String,
        val present: Boolean,
        val timeoutMs: Long,
        val intervalMs: Long,
    ) : AutomationCommand

    data class Click(val reference: String) : AutomationCommand

    data class LongClick(val reference: String) : AutomationCommand

    data class Scroll(val reference: String, val direction: ScrollDirection) : AutomationCommand

    data class LaunchApp(val packageName: String) : AutomationCommand

    data class SetText(val reference: String, val text: String) : AutomationCommand

    data class Touch(val gesture: TouchGesture) : AutomationCommand

    data class Navigate(val action: NavigationAction) : AutomationCommand
}

/** Implementations own all platform objects and return immutable observations. */
interface AutomationBackend {
    suspend fun typeText(reference: String, text: String): AutomationResult =
        AutomationResult.Failed(AutomationError.UNSUPPORTED_ACTION)

    suspend fun control(reference: String, action: ControlAction): AutomationResult =
        AutomationResult.Failed(AutomationError.UNSUPPORTED_ACTION)

    suspend fun observe(): AutomationResult

    suspend fun click(reference: String): AutomationResult

    suspend fun setText(reference: String, text: String): AutomationResult

    suspend fun touch(gesture: TouchGesture): AutomationResult

    suspend fun navigate(action: NavigationAction): AutomationResult

    suspend fun query(query: ObservationQuery): AutomationResult =
        AutomationResult.Failed(AutomationError.UNSUPPORTED_ACTION)

    suspend fun longClick(reference: String): AutomationResult =
        AutomationResult.Failed(AutomationError.UNSUPPORTED_ACTION)

    suspend fun scroll(reference: String, direction: ScrollDirection): AutomationResult =
        AutomationResult.Failed(AutomationError.UNSUPPORTED_ACTION)

    suspend fun launchApp(packageName: String): AutomationResult =
        AutomationResult.Failed(AutomationError.UNSUPPORTED_ACTION)
}

/** Platform-independent accessibility intent; native objects remain in the backend. */
data class ControlAction(
    val type: String,
    val value: String? = null,
    val checked: Boolean? = null,
    val expectedEnabled: Boolean? = null,
    val expectedReadOnly: Boolean? = null,
    val expectedChecked: String? = null,
    val expectedSelected: Boolean? = null,
    val expectedValue: String? = null,
)
