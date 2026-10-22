package com.pixels.agent

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.pixels.agent.core.AppearancePreferences
import com.pixels.agent.core.ConnectionRepository
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

@OptIn(ExperimentalCoroutinesApi::class)
internal class ProductViewModel(
    private val store: ProductPreferences,
    reader: ActivityHistoryReader,
    connection: ConnectionRepository,
) : ViewModel() {
    val preferences = store.state
    val preferencesFailed = store.failed
    private val refresh = MutableStateFlow(0)
    val history =
        combine(connection.settings, refresh) { settings, _ -> settings }
            .flatMapLatest { settings ->
                flow {
                        while (currentCoroutineContext().isActive) {
                            val history =
                                try {
                                    reader.read(settings)
                                } catch (_: Exception) {
                                    ActivityHistory(unavailable = true)
                                }
                            emit(history)
                            delay(5000)
                        }
                    }
                    .flowOn(Dispatchers.IO)
            }
            .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5000), ActivityHistory())

    init {
        viewModelScope.launch { store.load() }
    }

    fun savePreferences(value: AppearancePreferences) {
        viewModelScope.launch { store.save(value) }
    }

    fun refreshHistory() {
        refresh.value++
    }
}
