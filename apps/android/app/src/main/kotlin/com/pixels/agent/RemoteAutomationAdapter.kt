package com.pixels.agent

import android.app.KeyguardManager
import android.os.PowerManager
import com.pixels.agent.core.AutomationCommand
import com.pixels.agent.core.AutomationResult
import com.pixels.agent.core.CaptureFailure
import com.pixels.agent.core.CaptureResult
import com.pixels.agent.core.CommandDispatcher
import com.pixels.agent.core.NavigationAction
import com.pixels.agent.core.ObservationQuery
import com.pixels.agent.core.RemoteCommandHandler
import com.pixels.agent.core.RemotePlatformResult
import com.pixels.agent.core.ScreenCaptureProvider
import com.pixels.agent.core.ScrollDirection
import com.pixels.agent.core.TouchGesture
import java.util.UUID
import kotlinx.coroutines.CancellationException
import org.json.JSONArray
import org.json.JSONObject

/** Translates the private platform mailbox; authentication and persistence stay in Rust. */
internal class RemoteAutomationAdapter(
    private val dispatcher: CommandDispatcher,
    private val capture: ScreenCaptureProvider,
    private val keyguard: KeyguardManager,
    private val power: PowerManager,
    private val connectionAllowed: () -> Boolean = { true },
) : RemoteCommandHandler {
    override suspend fun execute(command: String): RemotePlatformResult {
        try {
            val input = JSONObject(command)
            if (input.getString("action") == "capabilities") {
                return RemotePlatformResult(
                    JSONObject()
                        .put(
                            "capabilities",
                            JSONObject()
                                .put("schema_version", 2)
                                .put(
                                    "actions",
                                    JSONArray(
                                        listOf(
                                            "observe",
                                            "query",
                                            "click",
                                            "long_click",
                                            "set_text",
                                            "scroll",
                                            "gesture",
                                            "navigate",
                                            "launch_app",
                                            "wait_for",
                                            "screenshot",
                                            "capabilities",
                                        )
                                    ),
                                )
                                .put("max_elements", 128)
                                .put("max_text_bytes", 4096)
                                .put(
                                    "screenshot_requires_consent",
                                    android.os.Build.VERSION.SDK_INT < 30,
                                ),
                        )
                        .toString()
                )
            }
            if (!connectionAllowed()) return failure("control_disconnected")
            if (dispatcher.paused.value) return failure("control_paused")
            if (locked()) return failure("screen_locked")
            if (input.getString("action") == "screenshot") {
                return when (val frame = capture.capture()) {
                    is CaptureResult.Failed ->
                        failure(
                            if (frame.reason == CaptureFailure.PROTECTED_OR_REJECTED)
                                "protected_content"
                            else frame.reason.name.lowercase()
                        )
                    is CaptureResult.Frame ->
                        if (!connectionAllowed()) failure("control_disconnected")
                        else if (dispatcher.paused.value) failure("control_paused")
                        else if (locked()) failure("screen_locked")
                        else
                            RemotePlatformResult(
                                JSONObject()
                                    .put("width", frame.width)
                                    .put("height", frame.height)
                                    .toString(),
                                frame.jpeg,
                            )
                }
            }
            val request =
                when (input.getString("action")) {
                    "observe" -> AutomationCommand.Observe
                    "query" ->
                        AutomationCommand.Query(
                            ObservationQuery(
                                input.optionalString("root_reference"),
                                input.optionalString("text_contains"),
                                input.optionalString("role"),
                                input.getInt("max_nodes"),
                                input.getInt("max_depth"),
                            )
                        )
                    "click" -> AutomationCommand.Click(input.getString("reference"))
                    "long_click" -> AutomationCommand.LongClick(input.getString("reference"))
                    "scroll" ->
                        AutomationCommand.Scroll(
                            input.getString("reference"),
                            ScrollDirection.valueOf(input.getString("direction").uppercase()),
                        )
                    "launch_app" -> AutomationCommand.LaunchApp(input.getString("package_name"))
                    "wait_for" ->
                        AutomationCommand.WaitFor(
                            input.getString("text"),
                            input.getBoolean("present"),
                            input.getLong("timeout_ms"),
                            input.getLong("interval_ms"),
                        )
                    "set_text" ->
                        AutomationCommand.SetText(
                            input.getString("reference"),
                            input.getString("text"),
                        )
                    "navigate" ->
                        AutomationCommand.Navigate(
                            NavigationAction.valueOf(input.getString("destination").uppercase())
                        )
                    "gesture" ->
                        AutomationCommand.Touch(
                            TouchGesture(
                                input.getLong("revision"),
                                input.getInt("from_x"),
                                input.getInt("from_y"),
                                input.getInt("to_x"),
                                input.getInt("to_y"),
                                input.getLong("duration_ms"),
                            )
                        )
                    else -> return failure("unsupported_action")
                }
            return when (val result = dispatcher.execute(request)) {
                is AutomationResult.Failed -> failure(result.error.name.lowercase())
                is AutomationResult.Dispatched ->
                    RemotePlatformResult(
                        JSONObject()
                            .put("acknowledged", result.platformAcknowledged)
                            .put("warnings", JSONArray(result.warnings))
                            .toString()
                    )
                is AutomationResult.Observed -> {
                    if (!connectionAllowed()) return failure("control_disconnected")
                    if (dispatcher.paused.value) return failure("control_paused")
                    if (locked()) return failure("screen_locked")
                    val screen = result.screen
                    val elements = JSONArray()
                    var truncated = screen.truncated
                    for (element in screen.elements) {
                        val left = element.bounds.left.coerceIn(0, screen.geometry.width)
                        val top = element.bounds.top.coerceIn(0, screen.geometry.height)
                        val item =
                            JSONObject()
                                .put("element_ref", element.reference)
                                .put("role", element.role.take(128))
                                .put(
                                    "name",
                                    if (element.protected) JSONObject.NULL
                                    else element.name?.take(256) ?: JSONObject.NULL,
                                )
                                .put(
                                    "bounds",
                                    JSONObject()
                                        .put("left", left)
                                        .put("top", top)
                                        .put(
                                            "right",
                                            element.bounds.right.coerceIn(
                                                left,
                                                screen.geometry.width,
                                            ),
                                        )
                                        .put(
                                            "bottom",
                                            element.bounds.bottom.coerceIn(
                                                top,
                                                screen.geometry.height,
                                            ),
                                        ),
                                )
                                .put("enabled", element.enabled)
                                .put("clickable", element.clickable)
                                .put("editable", element.editable)
                                .put("protected", element.protected)
                        if (
                            elements.length() >= 128 ||
                                elements.toString().toByteArray().size +
                                    item.toString().toByteArray().size > 30000
                        ) {
                            truncated = true
                            break
                        }
                        elements.put(item)
                    }
                    val observation =
                        JSONObject()
                            .put("observation_ref", UUID.randomUUID().toString())
                            .put("revision", screen.revision)
                            .put("width", screen.geometry.width)
                            .put("height", screen.geometry.height)
                            .put("rotation", screen.geometry.rotation)
                            .put("elements", elements)
                            .put("truncated", truncated)
                            .put("package_name", screen.packageName ?: JSONObject.NULL)
                    RemotePlatformResult(JSONObject().put("observation", observation).toString())
                }
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Exception) {
            return failure("platform_rejected")
        }
    }

    private fun locked() = keyguard.isKeyguardLocked || !power.isInteractive

    private fun JSONObject.optionalString(name: String): String? =
        if (isNull(name)) null else getString(name)

    private fun failure(code: String) =
        RemotePlatformResult(JSONObject().put("error", code).toString())
}
