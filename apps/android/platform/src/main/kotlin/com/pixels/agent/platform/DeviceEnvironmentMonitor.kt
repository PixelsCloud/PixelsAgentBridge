package com.pixels.agent.platform

import android.app.KeyguardManager
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import com.pixels.agent.core.DeviceEnvironment
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow

/** Observes system state while a home screen is visible; it never owns the connection. */
class DeviceEnvironmentMonitor(private val context: Context) {
    private val connectivity = context.getSystemService(ConnectivityManager::class.java)
    private val handler = Handler(Looper.getMainLooper())
    private val current = MutableStateFlow(DeviceEnvironment())
    val state = current.asStateFlow()
    private var observers = 0
    private var defaultNetwork: Network? = null
    private val screenReceiver =
        object : BroadcastReceiver() {
            override fun onReceive(context: Context?, intent: Intent?) = refreshScreen()
        }
    private val networkCallback =
        object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                defaultNetwork = network
            }

            override fun onCapabilitiesChanged(
                network: Network,
                capabilities: NetworkCapabilities,
            ) {
                if (network == defaultNetwork) {
                    // A local server is usable without Android validating public Internet access.
                    current.value =
                        current.value.copy(
                            networkAvailable =
                                capabilities.hasCapability(
                                    NetworkCapabilities.NET_CAPABILITY_INTERNET
                                )
                        )
                }
            }

            override fun onLost(network: Network) {
                if (network == defaultNetwork) {
                    defaultNetwork = null
                    current.value = current.value.copy(networkAvailable = false)
                }
            }
        }

    fun start() {
        if (observers++ != 0) return
        defaultNetwork = connectivity.activeNetwork
        current.value =
            current.value.copy(
                networkAvailable =
                    connectivity
                        .getNetworkCapabilities(defaultNetwork)
                        ?.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) == true
            )
        refreshScreen()
        val filter =
            IntentFilter().apply {
                addAction(Intent.ACTION_SCREEN_ON)
                addAction(Intent.ACTION_SCREEN_OFF)
                addAction(Intent.ACTION_USER_PRESENT)
            }
        if (Build.VERSION.SDK_INT >= 33) {
            context.registerReceiver(screenReceiver, filter, Context.RECEIVER_NOT_EXPORTED)
        } else context.registerReceiver(screenReceiver, filter)
        connectivity.registerDefaultNetworkCallback(networkCallback, handler)
    }

    fun refreshScreen() {
        val locked = context.getSystemService(KeyguardManager::class.java).isKeyguardLocked
        val interactive = context.getSystemService(PowerManager::class.java).isInteractive
        current.value = current.value.copy(screenLocked = locked || !interactive)
    }

    fun stop() {
        check(observers > 0)
        if (--observers != 0) return
        connectivity.unregisterNetworkCallback(networkCallback)
        context.unregisterReceiver(screenReceiver)
    }
}
