package com.pixels.agent.platform

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.app.KeyguardManager
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.graphics.Path
import android.graphics.Rect
import android.os.Build
import android.os.Bundle
import android.os.PowerManager
import android.util.DisplayMetrics
import android.view.WindowManager
import android.view.accessibility.AccessibilityNodeInfo
import com.pixels.agent.core.AutomationBackend
import com.pixels.agent.core.AutomationError
import com.pixels.agent.core.AutomationResult
import com.pixels.agent.core.Bounds
import com.pixels.agent.core.ControlAction
import com.pixels.agent.core.NavigationAction
import com.pixels.agent.core.ObservationQuery
import com.pixels.agent.core.ScreenElement
import com.pixels.agent.core.ScreenGeometry
import com.pixels.agent.core.ScreenObservation
import com.pixels.agent.core.ScrollDirection
import com.pixels.agent.core.TouchGesture
import com.pixels.agent.core.boundedUtf8
import java.util.UUID
import kotlin.coroutines.resume
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull

/** All service references, nodes and reference tokens are confined to the main thread. */
class AndroidAutomationBackend(private val context: Context) : AutomationBackend {
    private var service: AccessibilityService? = null
    private var revision = 0L
    private val references = mutableMapOf<String, NodeToken>()
    private val mutableAvailable = MutableStateFlow(false)
    val available = mutableAvailable.asStateFlow()

    internal fun activeService(): AccessibilityService? = service

    fun attach(service: AccessibilityService) {
        this.service = service
        resetReferences()
        mutableAvailable.value = true
    }

    fun invalidate(service: AccessibilityService) {
        if (this.service === service) resetReferences()
    }

    fun detach(service: AccessibilityService) {
        if (this.service !== service) return
        this.service = null
        resetReferences()
        mutableAvailable.value = false
    }

    private fun resetReferences() {
        revision++
        references.clear()
    }

    override suspend fun observe(): AutomationResult = query(ObservationQuery())

    suspend fun activeWindow(): ScreenObservation =
        withContext(Dispatchers.Main.immediate) {
            val active =
                service ?: throw com.pixels.agent.core.StorageFailure("service_unavailable")
            if (locked()) throw com.pixels.agent.core.StorageFailure("screen_locked")
            val root =
                active.rootInActiveWindow
                    ?: throw com.pixels.agent.core.StorageFailure("target_stale")
            try {
                ScreenObservation(
                    revision,
                    geometry(),
                    emptyList(),
                    false,
                    root.packageName?.toString(),
                    root.windowId,
                )
            } finally {
                release(root)
            }
        }

    override suspend fun query(query: ObservationQuery): AutomationResult =
        withContext(Dispatchers.Main.immediate) {
            val active = service ?: return@withContext failure(AutomationError.SERVICE_UNAVAILABLE)
            if (locked()) return@withContext failure(AutomationError.SCREEN_LOCKED)
            if (query.maxNodes !in 1..128 || query.maxDepth !in 1..32) {
                return@withContext failure(AutomationError.INVALID_ARGUMENT)
            }
            val token = query.rootReference?.let { references[it] }
            if (query.rootReference != null && token == null)
                return@withContext failure(AutomationError.TARGET_STALE)
            val root = if (token == null) active.rootInActiveWindow else resolve(active, token)
            if (root == null) return@withContext failure(AutomationError.TARGET_STALE)
            if (!root.refresh()) {
                release(root)
                return@withContext failure(AutomationError.TARGET_STALE)
            }
            val basePath = token?.path ?: emptyList()
            val packageName = root.packageName?.toString()
            val windowId = root.windowId
            val maxVisited = query.maxVisited.coerceIn(1, MAX_NODES)
            val deadline =
                if (query.timeoutMs > 0) android.os.SystemClock.elapsedRealtime() + query.timeoutMs
                else Long.MAX_VALUE
            resetReferences()
            val elements = mutableListOf<ScreenElement>()
            val pending = ArrayDeque<Pair<AccessibilityNodeInfo, List<Int>>>()
            pending.add(root to basePath)
            var visited = 0
            var truncated = false
            val textContains = query.textContains
            try {
                while (
                    pending.isNotEmpty() &&
                        elements.size < query.maxNodes &&
                        visited < maxVisited &&
                        android.os.SystemClock.elapsedRealtime() < deadline
                ) {
                    val (node, path) = pending.removeFirst()
                    visited++
                    try {
                        // Refresh children too: refreshing only the root can leave cached
                        // focus, text and checked states stale after native actions.
                        if (!node.refresh()) {
                            truncated = true
                            continue
                        }
                        val name =
                            if (node.isPassword) null
                            else (node.contentDescription ?: node.text)?.toString()
                        if (
                            node.isVisibleToUser &&
                                (query.identifier == null ||
                                    node.viewIdResourceName == query.identifier) &&
                                (textContains == null || name?.contains(textContains) == true) &&
                                (query.role == null || node.className?.toString() == query.role)
                        ) {
                            val element = describe(node)
                            references[element.reference] =
                                NodeToken(revision, path, node.windowId, fingerprint(node))
                            elements.add(element)
                        }
                        if (path.size - basePath.size < query.maxDepth) {
                            val count =
                                minOf(node.childCount, maxVisited - pending.size - visited)
                                    .coerceAtLeast(0)
                            if (count < node.childCount) truncated = true
                            for (index in 0 until count) {
                                node.getChild(index)?.let { pending.add(it to (path + index)) }
                            }
                        } else if (node.childCount > 0) {
                            truncated = true
                        }
                    } finally {
                        release(node)
                    }
                }
                AutomationResult.Observed(
                    ScreenObservation(
                        revision,
                        geometry(),
                        elements.toList(),
                        truncated || pending.isNotEmpty(),
                        packageName,
                        windowId,
                        visited,
                    )
                )
            } finally {
                pending.forEach { release(it.first) }
            }
        }

    override suspend fun click(reference: String): AutomationResult =
        nodeAction(reference) { node ->
            if (!node.isClickable || !node.isEnabled) failure(AutomationError.UNSUPPORTED_ACTION)
            else acknowledgement(node.performAction(AccessibilityNodeInfo.ACTION_CLICK))
        }

    override suspend fun longClick(reference: String): AutomationResult =
        nodeAction(reference) { node ->
            if (!node.isLongClickable || !node.isEnabled)
                failure(AutomationError.UNSUPPORTED_ACTION)
            else acknowledgement(node.performAction(AccessibilityNodeInfo.ACTION_LONG_CLICK))
        }

    override suspend fun scroll(reference: String, direction: ScrollDirection): AutomationResult =
        nodeAction(reference) { node ->
            val action =
                when (direction) {
                    ScrollDirection.FORWARD -> AccessibilityNodeInfo.ACTION_SCROLL_FORWARD
                    ScrollDirection.BACKWARD -> AccessibilityNodeInfo.ACTION_SCROLL_BACKWARD
                    ScrollDirection.UP ->
                        AccessibilityNodeInfo.AccessibilityAction.ACTION_SCROLL_UP.id
                    ScrollDirection.DOWN ->
                        AccessibilityNodeInfo.AccessibilityAction.ACTION_SCROLL_DOWN.id
                    ScrollDirection.LEFT ->
                        AccessibilityNodeInfo.AccessibilityAction.ACTION_SCROLL_LEFT.id
                    ScrollDirection.RIGHT ->
                        AccessibilityNodeInfo.AccessibilityAction.ACTION_SCROLL_RIGHT.id
                }
            if (!node.isEnabled || node.actionList.none { it.id == action })
                failure(AutomationError.UNSUPPORTED_ACTION)
            else acknowledgement(node.performAction(action))
        }

    override suspend fun launchApp(packageName: String): AutomationResult =
        withContext(Dispatchers.Main.immediate) {
            val active = service ?: return@withContext failure(AutomationError.SERVICE_UNAVAILABLE)
            if (locked()) return@withContext failure(AutomationError.SCREEN_LOCKED)
            if (
                packageName.length > 255 ||
                    !packageName.matches(
                        Regex("[A-Za-z_][A-Za-z0-9_]*(\\.[A-Za-z_][A-Za-z0-9_]*)+")
                    )
            ) {
                return@withContext failure(AutomationError.INVALID_ARGUMENT)
            }
            val intent =
                context.packageManager.getLaunchIntentForPackage(packageName)
                    ?: return@withContext failure(AutomationError.APP_NOT_FOUND)
            try {
                active.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                resetReferences()
                // Some OEMs silently refuse background launches without throwing. An
                // accepted startActivity call alone does not establish the visible target.
                val visible =
                    withTimeoutOrNull(5000) {
                        while (true) {
                            val root = active.rootInActiveWindow
                            val matches =
                                try {
                                    root?.packageName?.toString() == packageName
                                } finally {
                                    root?.let(::release)
                                }
                            if (matches) return@withTimeoutOrNull true
                            delay(100)
                        }
                    }
                if (visible == true) acknowledgement(true)
                // startActivity was accepted. Late visibility is not proof of a
                // pre-dispatch refusal and must never invite automatic replay.
                else failure(AutomationError.UNCONFIRMED)
            } catch (_: ActivityNotFoundException) {
                failure(AutomationError.APP_NOT_FOUND)
            } catch (_: SecurityException) {
                failure(AutomationError.APP_LAUNCH_BLOCKED)
            }
        }

    override suspend fun setText(reference: String, text: String): AutomationResult =
        nodeAction(reference) { node ->
            if (!node.isEditable || node.isPassword || !node.isEnabled) {
                failure(AutomationError.UNSUPPORTED_ACTION)
            } else {
                val arguments =
                    Bundle().apply {
                        putCharSequence(
                            AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE,
                            text,
                        )
                    }
                acknowledgement(
                    node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, arguments)
                )
            }
        }

    /** Inserts at the current selection; setText remains the explicit replacement operation. */
    override suspend fun typeText(reference: String, text: String): AutomationResult =
        nodeAction(reference) { node ->
            if (!node.isEditable || node.isPassword || !node.isEnabled || !node.isFocused)
                return@nodeAction failure(AutomationError.UNSUPPORTED_ACTION)
            val old = node.text?.toString().orEmpty()
            // Some native empty fields expose no selection (-1), even while focused.
            // Their sole insertion point is 0; unknown selection in nonempty text is rejected.
            val emptySelection =
                old.isEmpty() && node.textSelectionStart == -1 && node.textSelectionEnd == -1
            val start = if (emptySelection) 0 else node.textSelectionStart
            val end = if (emptySelection) 0 else node.textSelectionEnd
            if (
                text.isEmpty() ||
                    text.toByteArray().size > 4096 ||
                    text.contains('\u0000') ||
                    start !in 0..old.length ||
                    end !in 0..old.length
            )
                return@nodeAction failure(AutomationError.INVALID_ARGUMENT)
            val replacement =
                old.substring(0, minOf(start, end)) + text + old.substring(maxOf(start, end))
            if (replacement.toByteArray().size > 65536)
                return@nodeAction failure(AutomationError.INVALID_ARGUMENT)
            try {
                val changed =
                    node.performAction(
                        AccessibilityNodeInfo.ACTION_SET_TEXT,
                        Bundle().apply {
                            putCharSequence(
                                AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE,
                                replacement,
                            )
                        },
                    )
                if (!changed) return@nodeAction failure(AutomationError.UNCONFIRMED)
                val caret = minOf(start, end) + text.length
                if (!node.refresh()) return@nodeAction failure(AutomationError.UNCONFIRMED)
                // ACTION_SET_TEXT normally moves the caret to the end. Avoid a second
                // action when this is already the requested insertion position.
                if (
                    node.text?.toString() == replacement &&
                        node.textSelectionStart == caret &&
                        node.textSelectionEnd == caret
                )
                    return@nodeAction acknowledgement(true)
                if (
                    node.performAction(
                        AccessibilityNodeInfo.ACTION_SET_SELECTION,
                        Bundle().apply {
                            putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, caret)
                            putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, caret)
                        },
                    )
                )
                    acknowledgement(true)
                else failure(AutomationError.UNCONFIRMED)
            } catch (_: RuntimeException) {
                failure(AutomationError.UNCONFIRMED)
            }
        }

    /** Read an existing token without retargeting by text, coordinates or a new traversal. */
    suspend fun inspect(reference: String): ScreenElement? =
        withContext(Dispatchers.Main.immediate) {
            val active = service ?: return@withContext null
            if (locked()) return@withContext null
            val token = references[reference] ?: return@withContext null
            if (token.revision != revision) return@withContext null
            val node = resolve(active, token) ?: return@withContext null
            try {
                describe(node).copy(reference = reference)
            } finally {
                release(node)
            }
        }

    @Suppress("DEPRECATION") // Boolean checked state remains available on Android 9+.
    override suspend fun control(reference: String, action: ControlAction): AutomationResult =
        nodeAction(reference) { node ->
            val protected = node.isPassword
            if (
                action.expectedEnabled?.let { it != node.isEnabled } == true ||
                    action.expectedReadOnly?.let { it != (node.isEditable.not()) } == true ||
                    action.expectedChecked?.let {
                        !node.isCheckable || it != (if (node.isChecked) "on" else "off")
                    } == true ||
                    action.expectedSelected?.let { it != node.isSelected } == true ||
                    action.expectedValue?.let { protected || it != node.text?.toString() } == true
            )
                return@nodeAction failure(AutomationError.PRECONDITION_FAILED)
            if (!node.isEnabled || protected)
                return@nodeAction failure(AutomationError.UNSUPPORTED_ACTION)
            val args = Bundle()
            val id =
                when (action.type) {
                    "invoke" -> AccessibilityNodeInfo.ACTION_CLICK
                    "set_value" -> {
                        val text =
                            action.value
                                ?: return@nodeAction failure(AutomationError.INVALID_ARGUMENT)
                        if (
                            !node.isEditable ||
                                text.toByteArray().size > 4096 ||
                                text.contains('\u0000')
                        )
                            return@nodeAction failure(AutomationError.INVALID_ARGUMENT)
                        args.putCharSequence(
                            AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE,
                            text,
                        )
                        AccessibilityNodeInfo.ACTION_SET_TEXT
                    }
                    "set_checked" -> {
                        if (!node.isCheckable || action.checked == null)
                            return@nodeAction failure(AutomationError.UNSUPPORTED_ACTION)
                        if (node.isChecked == action.checked)
                            return@nodeAction AutomationResult.Dispatched(true, false)
                        AccessibilityNodeInfo.ACTION_CLICK
                    }
                    "select" ->
                        if (node.isSelected)
                            return@nodeAction AutomationResult.Dispatched(true, false)
                        else AccessibilityNodeInfo.ACTION_SELECT
                    "expand" -> AccessibilityNodeInfo.ACTION_EXPAND
                    "collapse" -> AccessibilityNodeInfo.ACTION_COLLAPSE
                    "focus" ->
                        if (node.isFocused)
                            return@nodeAction AutomationResult.Dispatched(true, false)
                        else AccessibilityNodeInfo.ACTION_FOCUS
                    else -> return@nodeAction failure(AutomationError.UNSUPPORTED_ACTION)
                }
            if (node.actionList.none { it.id == id }) failure(AutomationError.UNSUPPORTED_ACTION)
            else
                try {
                    acknowledgement(node.performAction(id, args))
                } catch (_: RuntimeException) {
                    failure(AutomationError.UNCONFIRMED)
                }
        }

    private suspend fun nodeAction(
        reference: String,
        action: (AccessibilityNodeInfo) -> AutomationResult,
    ): AutomationResult =
        withContext(Dispatchers.Main.immediate) {
            val active = service ?: return@withContext failure(AutomationError.SERVICE_UNAVAILABLE)
            if (locked()) return@withContext failure(AutomationError.SCREEN_LOCKED)
            val token =
                references[reference] ?: return@withContext failure(AutomationError.TARGET_STALE)
            if (token.revision != revision) return@withContext failure(AutomationError.TARGET_STALE)
            val node =
                resolve(active, token) ?: return@withContext failure(AutomationError.TARGET_STALE)
            try {
                action(node)
            } finally {
                release(node)
                resetReferences()
            }
        }

    private fun resolve(active: AccessibilityService, token: NodeToken): AccessibilityNodeInfo? {
        if (token.revision != revision) return null
        var node = active.rootInActiveWindow ?: return null
        for (index in token.path) {
            val next = node.getChild(index)
            release(node)
            node = next ?: return null
        }
        if (
            !node.refresh() ||
                node.windowId != token.windowId ||
                fingerprint(node) != token.fingerprint
        ) {
            release(node)
            return null
        }
        return node
    }

    override suspend fun touch(gesture: TouchGesture): AutomationResult =
        withContext(Dispatchers.Main.immediate) {
            val active = service ?: return@withContext failure(AutomationError.SERVICE_UNAVAILABLE)
            if (locked()) return@withContext failure(AutomationError.SCREEN_LOCKED)
            if (gesture.revision != revision)
                return@withContext failure(AutomationError.TARGET_STALE)
            val screen = geometry()
            if (
                gesture.durationMs !in 1..2000 ||
                    listOf(gesture.fromX, gesture.toX).any { it !in 0 until screen.width } ||
                    listOf(gesture.fromY, gesture.toY).any { it !in 0 until screen.height }
            ) {
                return@withContext failure(AutomationError.INVALID_ARGUMENT)
            }
            val path =
                Path().apply {
                    moveTo(gesture.fromX.toFloat(), gesture.fromY.toFloat())
                    lineTo(gesture.toX.toFloat(), gesture.toY.toFloat())
                }
            val description =
                GestureDescription.Builder()
                    .addStroke(GestureDescription.StrokeDescription(path, 0, gesture.durationMs))
                    .build()
            val accepted =
                withTimeoutOrNull(gesture.durationMs + 2000) {
                    suspendCancellableCoroutine { continuation ->
                        val callback =
                            object : AccessibilityService.GestureResultCallback() {
                                override fun onCompleted(gestureDescription: GestureDescription?) {
                                    if (continuation.isActive)
                                        continuation.resume(acknowledgement(true))
                                }

                                override fun onCancelled(gestureDescription: GestureDescription?) {
                                    if (continuation.isActive)
                                        continuation.resume(failure(AutomationError.UNCONFIRMED))
                                }
                            }
                        if (
                            !active.dispatchGesture(description, callback, null) &&
                                continuation.isActive
                        ) {
                            continuation.resume(failure(AutomationError.PLATFORM_REJECTED))
                        }
                    }
                }
            resetReferences()
            accepted ?: failure(AutomationError.UNCONFIRMED)
        }

    override suspend fun navigate(action: NavigationAction): AutomationResult =
        withContext(Dispatchers.Main.immediate) {
            val active = service ?: return@withContext failure(AutomationError.SERVICE_UNAVAILABLE)
            if (locked()) return@withContext failure(AutomationError.SCREEN_LOCKED)
            val native =
                when (action) {
                    NavigationAction.BACK -> AccessibilityService.GLOBAL_ACTION_BACK
                    NavigationAction.HOME -> AccessibilityService.GLOBAL_ACTION_HOME
                    NavigationAction.RECENTS -> AccessibilityService.GLOBAL_ACTION_RECENTS
                }
            resetReferences()
            acknowledgement(active.performGlobalAction(native))
        }

    @Suppress("DEPRECATION")
    private fun geometry(): ScreenGeometry {
        val display = context.getSystemService(WindowManager::class.java).defaultDisplay
        val metrics = DisplayMetrics()
        display.getRealMetrics(metrics)
        return ScreenGeometry(metrics.widthPixels, metrics.heightPixels, display.rotation)
    }

    private fun locked() =
        context.getSystemService(KeyguardManager::class.java).isKeyguardLocked ||
            !context.getSystemService(PowerManager::class.java).isInteractive

    @Suppress("DEPRECATION") // Android 9 compatibility for the public checked-state accessor.
    private fun describe(node: AccessibilityNodeInfo): ScreenElement {
        val bounds = Rect().also(node::getBoundsInScreen)
        return ScreenElement(
            UUID.randomUUID().toString(),
            node.className?.toString().orEmpty(),
            if (node.isPassword) null
            else
                (node.contentDescription ?: node.text)?.toString()?.take(512)?.let {
                    boundedUtf8(it, 1024)
                },
            Bounds(bounds.left, bounds.top, bounds.right, bounds.bottom),
            node.isEnabled,
            node.isClickable,
            node.isEditable && !node.isPassword,
            node.isPassword,
            node.viewIdResourceName?.take(160),
            if (node.isPassword || !node.isEditable) null
            else node.text?.toString()?.let { boundedUtf8(it, 4096) },
            node.isFocused,
            node.isCheckable,
            node.isChecked,
            node.isSelected,
            node.actionList.map { it.id }.toSet(),
            !node.isPassword &&
                node.isEditable &&
                (node.text?.toString()?.toByteArray()?.size ?: 0) > 4096,
        )
    }

    private fun fingerprint(node: AccessibilityNodeInfo): List<Any?> =
        listOf(
            node.packageName?.toString(),
            node.viewIdResourceName,
            node.className?.toString(),
            Rect().also(node::getBoundsInScreen),
            node.isPassword,
            node.contentDescription?.toString(),
            node.isEnabled,
            node.isClickable,
            node.isScrollable,
            node.isLongClickable,
            if (node.isPassword) null else node.text?.toString(),
        )

    @Suppress("DEPRECATION")
    private fun release(node: AccessibilityNodeInfo) {
        if (Build.VERSION.SDK_INT < 33) node.recycle()
    }

    private fun acknowledgement(accepted: Boolean): AutomationResult =
        if (accepted) AutomationResult.Dispatched(true)
        else failure(AutomationError.PLATFORM_REJECTED)

    private fun failure(error: AutomationError) = AutomationResult.Failed(error)

    private data class NodeToken(
        val revision: Long,
        val path: List<Int>,
        val windowId: Int,
        val fingerprint: List<Any?>,
    )

    companion object {
        private const val MAX_NODES = 2048
    }
}
