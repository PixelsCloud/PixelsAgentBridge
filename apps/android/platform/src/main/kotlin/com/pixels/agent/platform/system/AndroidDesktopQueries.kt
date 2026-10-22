package com.pixels.agent.platform.system

import android.os.SystemClock
import android.view.accessibility.AccessibilityNodeInfo
import com.pixels.agent.core.AutomationCommand
import com.pixels.agent.core.AutomationResult
import com.pixels.agent.core.CommandDispatcher
import com.pixels.agent.core.ControlAction
import com.pixels.agent.core.ObservationQuery
import com.pixels.agent.core.ScreenElement
import com.pixels.agent.core.ScreenObservation
import com.pixels.agent.core.StorageFailure
import com.pixels.agent.platform.AndroidAutomationBackend
import java.util.UUID
import kotlinx.coroutines.delay
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import org.json.JSONArray
import org.json.JSONObject

/** Adapts the shared desktop/UI contract to the same Android backend and input dispatcher. */
class AndroidDesktopQueries(
    private val backend: AndroidAutomationBackend,
    private val dispatcher: CommandDispatcher,
    private val allowed: () -> Boolean,
) {
    private data class Reference(
        val caller: String,
        val scope: String,
        val window: String,
        val node: String?,
        val created: Long,
    )

    private val references = linkedMapOf<String, Reference>()
    private val gate = Mutex()
    private val instance = UUID.randomUUID().toString()

    suspend fun execute(input: JSONObject): JSONObject {
        val query = input.getJSONObject("query").getJSONObject("query")
        if (
            query.optString("operation") == "ui" &&
                query.getJSONObject("query").optString("operation") == "wait"
        )
            return wait(input, query.getJSONObject("query"))
        return gate.withLock { executeLocked(input, query) }
    }

    private suspend fun executeLocked(input: JSONObject, query: JSONObject): JSONObject {
        ensureAllowed()
        return when (query.getString("operation")) {
            "capture" -> {
                val screen = backend.activeWindow()
                if (!query.isNull("window_ref")) {
                    val reference = resolve(input, query.getString("window_ref"))
                    if (reference.node != null || reference.window != windowKey(screen))
                        throw StorageFailure("target_stale")
                }
                obj(
                    "window_key" to windowKey(screen),
                    "width" to screen.geometry.width,
                    "height" to screen.geometry.height,
                )
            }
            "windows",
            "monitors" -> {
                val screen = observe()
                val snapshot = snapshot()
                if (query.getString("operation") == "windows") {
                    val ref = remember(input, windowKey(screen), null)
                    snapshot.put(
                        "windows",
                        array(
                            listOf(
                                obj(
                                    "window_ref" to ref,
                                    "title" to screen.packageName,
                                    "process_id" to 0,
                                    "x" to 0,
                                    "y" to 0,
                                    "width" to screen.geometry.width,
                                    "height" to screen.geometry.height,
                                    "minimized" to false,
                                    "maximized" to false,
                                    "focused" to true,
                                    "monitor_id" to 0,
                                    "control_error" to "active_android_window_only",
                                )
                            )
                        ),
                    )
                    snapshot.put("window_ref", ref)
                } else
                    snapshot.put(
                        "monitors",
                        array(
                            listOf(
                                obj(
                                    "id" to 0,
                                    "name" to "Android active display",
                                    "x" to 0,
                                    "y" to 0,
                                    "width" to screen.geometry.width,
                                    "height" to screen.geometry.height,
                                    "primary" to true,
                                    "scale_percent" to 100,
                                    "rotation_degrees" to screen.geometry.rotation * 90,
                                )
                            )
                        ),
                    )
                response(
                    obj("type" to "desktop", "snapshot" to snapshot),
                    warnings =
                        listOf(
                            "Only the current unlocked Android accessibility window/display is reported; foreign process IDs and desktop window management are unavailable."
                        ),
                )
            }
            "focus",
            "type_text" -> {
                val reference = resolve(input, query.getString("window_ref"))
                val screen = observe()
                if (reference.node != null || reference.window != windowKey(screen))
                    throw StorageFailure("target_stale")
                var started = false
                if (query.getString("operation") == "type_text") {
                    if (screen.truncated) throw StorageFailure("focused_control_query_incomplete")
                    val focused =
                        screen.elements.filter { it.focused && it.editable && !it.protected }
                    if (focused.size != 1) throw StorageFailure("focused_editable_control_required")
                    val action =
                        dispatcher.execute(
                            AutomationCommand.TypeText(
                                focused.single().reference,
                                query.getString("text"),
                            )
                        )
                    requireAction(action)
                    started = true
                }
                response(
                    obj(
                        "type" to "desktop",
                        "snapshot" to
                            snapshot()
                                .put("window_ref", query.getString("window_ref"))
                                .put("action_started", started)
                                .put("verification", if (started) "native_returned" else "matched"),
                    )
                )
            }
            "ui" -> ui(input, query.getJSONObject("query"))
            else -> throw StorageFailure("android_desktop_action_unavailable")
        }
    }

    private suspend fun ui(input: JSONObject, query: JSONObject): JSONObject {
        val from = System.currentTimeMillis()
        val operation = query.getString("operation")
        var outcome = "completed"
        var dispatched = false
        var verification = "not_applicable"
        var truncated = false
        var visited = 0
        var error: String? = null
        val rows = JSONArray()
        when (operation) {
            "query",
            "wait" -> {
                val scope = query.getJSONObject("scope")
                val token =
                    resolve(
                        input,
                        scope.optString("window_ref").ifEmpty { scope.getString("element_ref") },
                    )
                val selector = query.optJSONObject("selector") ?: JSONObject()
                val limits = query.optJSONObject("limits") ?: JSONObject()
                val requested = limits.optInt("limit", 100)
                val screen =
                    if (operation == "wait" && token.node != null) {
                        val current =
                            backend.inspect(token.node) ?: throw StorageFailure("target_stale")
                        backend.activeWindow().copy(elements = listOf(current), visitedCount = 1)
                    } else
                        observe(
                            ObservationQuery(
                                rootReference = token.node,
                                textContains =
                                    selector.optionalString("name_contains")
                                        ?: selector.optionalString("name"),
                                maxNodes = minOf(requested, 128),
                                maxDepth = limits.optInt("max_depth", 6),
                                maxVisited = limits.optInt("max_visited", 2000),
                                timeoutMs = limits.optLong("timeout_ms", 3000),
                                identifier = selector.optionalString("identifier"),
                            )
                        )
                if (token.node == null && token.window != windowKey(screen))
                    throw StorageFailure("target_stale")
                visited = screen.visitedCount
                truncated = screen.truncated
                val matchedNodes = mutableListOf<ScreenElement>()
                for (element in screen.elements) {
                    val value = element(input, token.window, element, false)
                    if (matches(value, selector)) {
                        rows.put(value)
                        matchedNodes.add(element)
                    }
                }
                if (operation == "wait") {
                    val condition = query.getJSONObject("condition")
                    val type = condition.getString("type")
                    if (rows.length() > 1 && type !in listOf("exists", "absent"))
                        throw StorageFailure("ambiguous")
                    val one = rows.optJSONObject(0)
                    val matched =
                        when (type) {
                            "exists" -> rows.length() > 0
                            "absent" -> rows.length() == 0 && !truncated
                            "enabled" ->
                                !truncated &&
                                    one?.optBoolean("enabled") == condition.getBoolean("value")
                            "selected" ->
                                !truncated &&
                                    one?.optBoolean("selected") == condition.getBoolean("value")
                            "checked" ->
                                !truncated &&
                                    one?.optionalString("checked") == condition.getString("value")
                            "value_equals" ->
                                !truncated &&
                                    matchedNodes.singleOrNull()?.let {
                                        !it.protected &&
                                            !it.valueTruncated &&
                                            it.value == condition.getString("value")
                                    } == true
                            else -> throw StorageFailure("android_ui_condition_unavailable")
                        }
                    outcome = if (matched) "matched" else "timed_out"
                }
            }
            "get",
            "action" -> {
                val token = resolve(input, query.getString("element_ref"))
                val raw = token.node ?: throw StorageFailure("target_stale")
                val current = backend.inspect(raw) ?: throw StorageFailure("target_stale")
                if (operation == "get")
                    rows.put(
                        element(
                            input,
                            token.window,
                            current,
                            query.optBoolean("include_value"),
                            query.getString("element_ref"),
                        )
                    )
                else {
                    val action = query.getJSONObject("action")
                    val expected = query.optJSONObject("expected") ?: JSONObject()
                    val result =
                        dispatcher.execute(
                            AutomationCommand.Control(
                                raw,
                                ControlAction(
                                    action.getString("type"),
                                    action.optionalString("value"),
                                    if (action.isNull("checked")) null
                                    else action.getBoolean("checked"),
                                    expected.optionalBoolean("enabled"),
                                    expected.optionalBoolean("read_only"),
                                    expected.optionalString("checked"),
                                    expected.optionalBoolean("selected"),
                                    expected.optionalString("value"),
                                ),
                            )
                        )
                    if (result is AutomationResult.Dispatched) {
                        dispatched = result.actionDispatched
                        verification = if (dispatched) "native_returned" else "matched"
                    } else if (result is AutomationResult.Failed) {
                        error = result.error.name.lowercase()
                        outcome = if (error == "unconfirmed") "unconfirmed" else "rejected"
                        verification = "unavailable"
                    }
                }
            }
            else -> throw StorageFailure("android_ui_operation_unavailable")
        }
        val state =
            obj(
                "elements" to rows,
                "visited_count" to visited,
                "truncated" to truncated,
                "stop_reason" to if (truncated) "android_traversal_limit" else null,
                "outcome" to outcome,
                "action_dispatched" to if (outcome == "unconfirmed") null else dispatched,
                "verification" to verification,
                "error_code" to error,
                "sampled_from_unix_ms" to from,
                "sampled_at_unix_ms" to System.currentTimeMillis(),
            )
        return response(
            obj(
                "type" to "desktop",
                "snapshot" to snapshot().put("ui", state).put("action_started", dispatched),
            ),
            rows.length(),
            truncated = truncated,
        )
    }

    private suspend fun wait(input: JSONObject, query: JSONObject): JSONObject {
        val deadline = SystemClock.elapsedRealtime() + query.optLong("timeout_ms", 5000)
        while (true) {
            val result = gate.withLock {
                ensureAllowed()
                ui(input, query)
            }
            if (
                result
                    .getJSONObject("data")
                    .getJSONObject("snapshot")
                    .getJSONObject("ui")
                    .getString("outcome") == "matched" || SystemClock.elapsedRealtime() >= deadline
            )
                return result
            delay(query.optLong("poll_ms", 250))
        }
    }

    private fun element(
        input: JSONObject,
        window: String,
        node: ScreenElement,
        includeValue: Boolean,
        existing: String? = null,
    ): JSONObject {
        val actions = mutableListOf<String>()
        if (!node.protected) {
            if (node.clickable) actions.add("invoke")
            if (node.editable) actions.add("set_value")
            if (node.checkable && node.clickable) actions.add("set_checked")
            for ((id, name) in
                listOf(
                    AccessibilityNodeInfo.ACTION_FOCUS to "focus",
                    AccessibilityNodeInfo.ACTION_SELECT to "select",
                    AccessibilityNodeInfo.ACTION_EXPAND to "expand",
                    AccessibilityNodeInfo.ACTION_COLLAPSE to "collapse",
                )) if (id in node.actions) actions.add(name)
        }
        return obj(
            "element_ref" to (existing ?: remember(input, window, node.reference)),
            "parent_ref" to null,
            "role" to role(node.role),
            "native_role" to node.role.take(128),
            "name" to node.name,
            "identifier" to node.identifier,
            "value" to if (includeValue && !node.protected) node.value else null,
            "protected" to node.protected,
            "enabled" to node.enabled,
            "visible" to true,
            "offscreen" to false,
            "focused" to node.focused,
            "read_only" to !node.editable,
            "checked" to if (node.checkable) (if (node.checked) "on" else "off") else null,
            "selected" to node.selected,
            "expanded" to null,
            "bounding_rect" to
                obj(
                    "x" to node.bounds.left,
                    "y" to node.bounds.top,
                    "width" to node.bounds.right - node.bounds.left,
                    "height" to node.bounds.bottom - node.bounds.top,
                ),
            "coordinate_space" to "android_screen_pixels",
            "supported_actions" to array(actions),
            "field_errors" to
                obj("parent_ref" to "not_reported", "expanded" to "not_reported").apply {
                    if (node.valueTruncated) put("value", "text_limit")
                    if (node.protected) put("value", "protected")
                },
        )
    }

    private fun role(native: String): String =
        when {
            native.endsWith("EditText") -> "text_field"
            native.endsWith("CheckBox") || native.endsWith("Switch") -> "check_box"
            native.endsWith("RadioButton") -> "radio_button"
            native.endsWith("Button") -> "button"
            native.endsWith("TextView") -> "text"
            native.contains("RecyclerView") || native.endsWith("ListView") -> "list"
            native.endsWith("Layout") || native.endsWith("ScrollView") -> "group"
            else -> "unknown"
        }

    private fun matches(node: JSONObject, selector: JSONObject): Boolean =
        listOf("role", "name", "identifier").all {
            selector.isNull(it) || node.optionalString(it) == selector.getString(it)
        } &&
            (selector.isNull("name_contains") ||
                node.optionalString("name")?.contains(selector.getString("name_contains")) == true)

    private fun remember(input: JSONObject, window: String, node: String?): String {
        if (references.size >= 1024) references.remove(references.keys.first())
        val id = UUID.randomUUID().toString()
        references[id] =
            Reference(
                input.getString("caller"),
                input.getString("connection_scope"),
                window,
                node,
                SystemClock.elapsedRealtime(),
            )
        return id
    }

    private fun resolve(input: JSONObject, id: String): Reference {
        val ref = references[id] ?: throw StorageFailure("target_stale")
        if (
            ref.caller != input.getString("caller") ||
                ref.scope != input.getString("connection_scope") ||
                SystemClock.elapsedRealtime() - ref.created > 600000
        )
            throw StorageFailure("target_stale")
        return ref
    }

    private fun windowKey(screen: ScreenObservation) =
        "${screen.packageName}:${screen.windowId}:${screen.geometry}"

    private suspend fun observe(query: ObservationQuery = ObservationQuery()): ScreenObservation {
        ensureAllowed()
        return when (val result = dispatcher.execute(AutomationCommand.Query(query))) {
            is AutomationResult.Observed -> result.screen
            is AutomationResult.Failed -> throw StorageFailure(result.error.name.lowercase())
            else -> throw StorageFailure("android_ui_query_failed")
        }
    }

    private fun requireAction(result: AutomationResult) {
        if (result is AutomationResult.Failed) throw StorageFailure(result.error.name.lowercase())
        if (result !is AutomationResult.Dispatched)
            throw StorageFailure("android_ui_dispatch_unconfirmed")
    }

    private fun ensureAllowed() {
        if (!allowed() || dispatcher.paused.value)
            throw StorageFailure("control_paused_or_disconnected")
    }

    private fun snapshot() =
        obj(
            "helper_instance" to instance,
            "backend" to "android_accessibility",
            "coordinate_space" to "android_screen_pixels",
            "monitors" to JSONArray(),
            "windows" to JSONArray(),
            "window_ref" to null,
            "action_started" to false,
            "verification" to null,
        )

    private fun JSONObject.optionalBoolean(key: String): Boolean? =
        if (isNull(key)) null else getBoolean(key)
}
