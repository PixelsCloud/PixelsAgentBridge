package com.pixels.agent

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.pixels.agent.core.AutomationCommand
import com.pixels.agent.core.AutomationResult
import com.pixels.agent.core.CaptureResult
import com.pixels.agent.core.CommandDispatcher
import com.pixels.agent.core.ConnectionRepository
import com.pixels.agent.core.ConnectionSettings
import com.pixels.agent.core.ControlAvailability
import com.pixels.agent.core.ControlIntentRepository
import com.pixels.agent.core.DeviceEnvironment
import com.pixels.agent.core.ScreenCaptureProvider
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

data class DeviceUiState(
    val serviceReady: Boolean = false,
    val paused: Boolean = true,
    val nativeLoaded: Boolean = false,
    val nativeChecked: Boolean = false,
    val settingsLoaded: Boolean = false,
    val settingsFailed: Boolean = false,
    val captureActive: Boolean = false,
    val availability: ControlAvailability = ControlAvailability.PREPARING,
    val elementCount: Int? = null,
    val error: String? = null,
    val captureSize: Pair<Int, Int>? = null,
    val passwordRotating: Boolean = false,
)

class DeviceViewModel(
    serviceReady: StateFlow<Boolean>,
    private val dispatcher: CommandDispatcher,
    private val probe: () -> String,
    private val screenCapture: ScreenCaptureProvider,
    private val connection: ConnectionRepository,
    private val controlIntent: ControlIntentRepository,
    environment: StateFlow<DeviceEnvironment>,
    captureActive: StateFlow<Boolean>,
) : ViewModel() {
    val connectionSettings = connection.settings
    val connectionStatus = connection.status
    val accessPassword = connection.accessPassword
    private val details = MutableStateFlow(DeviceUiState())
    private val localState =
        combine(serviceReady, dispatcher.paused, details) { available, paused, details ->
            details.copy(serviceReady = available, paused = paused)
        }
    val state: StateFlow<DeviceUiState> =
        combine(localState, environment, connection.status, captureActive, controlIntent.state) {
                local,
                environment,
                connection,
                capturing,
                intent ->
                local.copy(
                    captureActive = capturing,
                    settingsLoaded = local.settingsLoaded && intent.loaded,
                    settingsFailed = local.settingsFailed || intent.error != null,
                    error = local.error ?: intent.error,
                    availability =
                        ControlAvailability.resolve(
                            nativeChecked = local.nativeChecked,
                            nativeLoaded = local.nativeLoaded,
                            settingsLoaded = local.settingsLoaded && intent.loaded,
                            settingsFailed = local.settingsFailed || intent.error != null,
                            serviceReady = local.serviceReady,
                            paused = local.paused,
                            environment = environment,
                            connection = connection,
                        ),
                )
            }
            .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5000), DeviceUiState())

    init {
        reloadSettings()
        viewModelScope.launch {
            val loaded =
                withContext(Dispatchers.IO) {
                    try {
                        org.json.JSONObject(probe()).optString("status") == "loaded"
                    } catch (_: UnsatisfiedLinkError) {
                        false
                    } catch (_: org.json.JSONException) {
                        false
                    }
                }
            details.value = details.value.copy(nativeLoaded = loaded, nativeChecked = true)
        }
    }

    fun reloadSettings() {
        viewModelScope.launch {
            try {
                connection.loadSettings()
                controlIntent.load()
                details.value =
                    details.value.copy(settingsLoaded = true, settingsFailed = false, error = null)
            } catch (cancelled: kotlinx.coroutines.CancellationException) {
                throw cancelled
            } catch (_: Exception) {
                details.value =
                    details.value.copy(
                        settingsLoaded = false,
                        settingsFailed = true,
                        error = "settings_unavailable",
                    )
            }
        }
    }

    fun saveConnection(settings: ConnectionSettings, onSaved: () -> Unit) {
        details.value = details.value.copy(error = null)
        viewModelScope.launch {
            try {
                connection.saveSettings(settings)
                details.value = details.value.copy(error = null)
                onSaved()
            } catch (cancelled: kotlinx.coroutines.CancellationException) {
                throw cancelled
            } catch (error: Exception) {
                details.value =
                    details.value.copy(
                        error =
                            when (error.message) {
                                "device_rename_rate_limited" -> "device_rename_rate_limited"
                                "device_rename_failed" -> "device_rename_failed"
                                else -> "settings_invalid_or_unavailable"
                            }
                    )
            }
        }
    }

    fun rotateAccessPassword() {
        if (details.value.passwordRotating) return
        details.value = details.value.copy(passwordRotating = true)
        viewModelScope.launch {
            try {
                connection.rotateAccessPassword()
                details.value = details.value.copy(error = null)
            } catch (cancelled: kotlinx.coroutines.CancellationException) {
                throw cancelled
            } catch (_: Exception) {
                details.value = details.value.copy(error = "password_rotation_failed")
            } finally {
                details.value = details.value.copy(passwordRotating = false)
            }
        }
    }

    fun observe() {
        viewModelScope.launch {
            when (val result = dispatcher.execute(AutomationCommand.Observe)) {
                is AutomationResult.Observed ->
                    details.value =
                        details.value.copy(
                            elementCount = result.screen.elements.size,
                            error = null,
                        )
                is AutomationResult.Failed ->
                    details.value = details.value.copy(error = result.error.name)
                is AutomationResult.Dispatched -> Unit
            }
        }
    }

    fun capture() {
        if (dispatcher.paused.value) return
        viewModelScope.launch {
            when (val result = screenCapture.capture()) {
                is CaptureResult.Frame ->
                    details.value =
                        details.value.copy(
                            captureSize = result.width to result.height,
                            error = null,
                        )
                is CaptureResult.Failed ->
                    details.value = details.value.copy(error = result.reason.name)
            }
        }
    }
}
