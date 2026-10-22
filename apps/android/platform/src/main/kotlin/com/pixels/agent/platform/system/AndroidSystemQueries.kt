package com.pixels.agent.platform.system

import android.app.ActivityManager
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.BatteryManager
import android.os.Build
import android.os.Debug
import android.os.Process
import android.os.StatFs
import android.os.SystemClock
import android.system.Os
import com.pixels.agent.core.StorageFailure
import java.io.File
import java.net.Inet4Address
import java.net.Inet6Address
import java.net.InetAddress
import java.util.concurrent.SynchronousQueue
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.delay
import org.json.JSONObject

/** Public SDK observations; unavailable global information is never inferred from app data. */
class AndroidSystemQueries(
    private val context: Context,
    fileExporter: (String) -> Pair<android.net.Uri, String>,
    packageSnapshot: (String) -> File,
    backend: com.pixels.agent.platform.AndroidAutomationBackend,
    dispatcher: com.pixels.agent.core.CommandDispatcher,
    canDispatch: () -> Boolean,
) {
    private val apps = AndroidApplications(context, canDispatch, fileExporter)
    private val packages = AndroidPackageChanges(context, canDispatch, packageSnapshot)
    private val logs = AndroidAppLogs(context)
    private val notifications =
        com.pixels.agent.platform.notifications.AndroidNotifications(context, canDispatch)
    private val desktop = AndroidDesktopQueries(backend, dispatcher, canDispatch)
    // A timed-out native resolver can keep running. Two occupied slots reject new work.
    private val dns = ThreadPoolExecutor(2, 2, 0, TimeUnit.SECONDS, SynchronousQueue<Runnable>())
    private val connectivity
        get() = context.getSystemService(ConnectivityManager::class.java)

    suspend fun execute(input: JSONObject): JSONObject {
        val query = input.getJSONObject("query")
        return when (query.getString("action")) {
            "notifications" -> notifications.execute(input)
            "desktop" -> desktop.execute(input)
            "info" -> info(query)
            "disks" -> disks(query)
            "networks" -> networks(query)
            "processes",
            "process" -> processes(query)
            "dns" -> resolve(query)
            "execution_contexts" -> apps.contexts(query)
            "applications" -> {
                val appQuery = query.getJSONObject("query")
                if (appQuery.getString("action") == "package") {
                    if (!apps.available())
                        throw StorageFailure("android_user_locked_or_display_not_interactive")
                    val result =
                        try {
                            packages.execute(
                                appQuery.getJSONObject("request"),
                                input.getString("caller"),
                                input.getString("request_id"),
                            )
                        } catch (failure: kotlinx.coroutines.CancellationException) {
                            throw failure
                        } catch (failure: StorageFailure) {
                            throw failure
                        } catch (_: Exception) {
                            return obj(
                                "error" to "package_action_outcome_unconfirmed",
                                "action_started" to
                                    (appQuery.getJSONObject("request").getString("operation") in
                                        listOf("install", "uninstall", "resume")),
                            )
                        }
                    result.put("execution_identity", apps.identity(true))
                    response(
                        obj(
                            "type" to "applications",
                            "snapshot" to obj("type" to "package", "result" to result),
                        )
                    )
                } else apps.execute(appQuery)
            }
            "logs" ->
                logs.execute(
                    query.getJSONObject("query"),
                    input.getString("caller"),
                    input.getJSONObject("device_ref"),
                )
            else -> throw StorageFailure("android_system_action_unavailable")
        }
    }

    private fun activeTransport(): String? {
        val caps = connectivity.getNetworkCapabilities(connectivity.activeNetwork) ?: return null
        return when {
            caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN) -> "vpn"
            caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> "wifi"
            caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> "cellular"
            caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> "ethernet"
            caps.hasTransport(NetworkCapabilities.TRANSPORT_BLUETOOTH) -> "bluetooth"
            else -> "other"
        }
    }

    private suspend fun ownProcess(sample: Boolean): JSONObject {
        val before = Process.getElapsedCpuTime()
        val start = SystemClock.elapsedRealtime()
        if (sample) delay(200)
        val elapsed = (SystemClock.elapsedRealtime() - start).coerceAtLeast(1)
        return obj(
            "termination_identity" to null,
            "pid" to Process.myPid(),
            "parent_pid" to Os.getppid(),
            "name" to context.packageName,
            "executable" to null,
            "user_id" to "uid:${Process.myUid()}",
            "user_name" to context.packageName,
            "status" to "running",
            "start_time_unix_seconds" to
                ((System.currentTimeMillis() - SystemClock.elapsedRealtime() +
                    Process.getStartElapsedRealtime()) / 1000),
            "memory_bytes" to Debug.getPss().toLong() * 1024,
            "cpu_usage_basis_points" to
                if (sample) ((Process.getElapsedCpuTime() - before) * 10000 / elapsed) else null,
        )
    }

    private suspend fun info(query: JSONObject): JSONObject {
        val memory = ActivityManager.MemoryInfo()
        context.getSystemService(ActivityManager::class.java).getMemoryInfo(memory)
        val battery = context.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
        val level = battery?.getIntExtra(BatteryManager.EXTRA_LEVEL, -1) ?: -1
        val scale = battery?.getIntExtra(BatteryManager.EXTRA_SCALE, -1) ?: -1
        val status = battery?.getIntExtra(BatteryManager.EXTRA_STATUS, -1) ?: -1
        val properties = connectivity.getLinkProperties(connectivity.activeNetwork)
        val caps = connectivity.getNetworkCapabilities(connectivity.activeNetwork)
        val mem =
            File("/proc/meminfo").useLines { lines ->
                lines
                    .take(128)
                    .mapNotNull { line ->
                        val fields = line.split(Regex("\\s+"))
                        fields.getOrNull(1)?.toLongOrNull()?.let {
                            fields[0].removeSuffix(":") to it * 1024
                        }
                    }
                    .toMap()
            }
        val swap = mem["SwapTotal"] ?: throw StorageFailure("memory_observation_unavailable")
        val swapFree = mem["SwapFree"] ?: throw StorageFailure("memory_observation_unavailable")
        val info =
            obj(
                "host_name" to null,
                "os_name" to "Android",
                "os_version" to Build.VERSION.RELEASE,
                "kernel_version" to Os.uname().release,
                "architecture" to Build.SUPPORTED_ABIS.firstOrNull(),
                "uptime_seconds" to SystemClock.elapsedRealtime() / 1000,
                "boot_time_unix_seconds" to
                    (System.currentTimeMillis() - SystemClock.elapsedRealtime()) / 1000,
                "cpu_brand" to Build.HARDWARE,
                "cpu_frequency_mhz" to null,
                "physical_cores" to null,
                "logical_cores" to Runtime.getRuntime().availableProcessors(),
                "cpu_usage_basis_points" to null,
                "memory_total_bytes" to memory.totalMem,
                "memory_available_bytes" to memory.availMem,
                "memory_used_bytes" to (memory.totalMem - memory.availMem).coerceAtLeast(0),
                "swap_total_bytes" to swap,
                "swap_used_bytes" to (swap - swapFree).coerceAtLeast(0),
                "executor" to ownProcess(query.optBoolean("sample_cpu")),
                "gpu" to
                    obj(
                        "backend" to "android_public_api",
                        "status" to "unavailable",
                        "driver_version" to null,
                        "entries" to array(emptyList<Any>()),
                        "errors" to array(listOf("GPU telemetry is not exposed by this adapter")),
                        "truncated" to false,
                    ),
                "mobile" to
                    obj(
                        "manufacturer" to Build.MANUFACTURER.take(256),
                        "model" to Build.MODEL.take(256),
                        "sdk" to Build.VERSION.SDK_INT,
                        "battery_percent" to
                            if (level >= 0 && scale > 0) (level * 100 / scale).coerceIn(0, 100)
                            else null,
                        "charging" to
                            if (status < 0) null
                            else
                                status == BatteryManager.BATTERY_STATUS_CHARGING ||
                                    status == BatteryManager.BATTERY_STATUS_FULL,
                        "active_transport" to activeTransport(),
                        "internet_validated" to
                            caps?.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED),
                        "dns_servers" to
                            array(
                                properties?.dnsServers?.mapNotNull { it.hostAddress }
                                    ?: emptyList<String>()
                            ),
                    ),
            )
        return response(
            obj("type" to "info", "info" to info),
            warnings =
                listOf(
                    "Memory is device-wide; executor memory/CPU are this app only. CPU brand is Android hardware identifier; logical cores are available processors.",
                    "Global CPU, physical cores, frequency and GPU telemetry are unavailable; null fields are not zero measurements.",
                ),
        )
    }

    private fun disks(query: JSONObject): JSONObject {
        val entries =
            listOf(File(context.noBackupFilesDir, "workspace") to "/workspace")
                .map { (file, mount) ->
                    val stat = StatFs(file.absolutePath)
                    obj(
                        "name" to "Pixels workspace",
                        "mount_point" to mount,
                        "filesystem" to "unavailable",
                        "kind" to "app_workspace",
                        "total_bytes" to stat.totalBytes,
                        "available_bytes" to stat.availableBytes,
                        "removable" to false,
                        "readonly" to !file.canWrite(),
                    )
                }
                .take(query.getInt("limit"))
        return response(
            obj("type" to "disks", "entries" to array(entries)),
            entries.size,
            listOf(
                "Capacity of the filesystem containing /workspace; not a physical disk inventory. Filesystem type is unavailable; SAF providers need not expose capacity."
            ),
        )
    }

    private fun networks(query: JSONObject): JSONObject {
        val wanted = query.optionalString("name")
        val seen = HashSet<String>()
        val entries =
            connectivity.allNetworks
                .mapNotNull { network ->
                    val props = connectivity.getLinkProperties(network) ?: return@mapNotNull null
                    val name = props.interfaceName ?: return@mapNotNull null
                    if (
                        !seen.add(name) ||
                            (wanted != null && !name.contains(wanted, ignoreCase = true))
                    )
                        return@mapNotNull null
                    fun counter(which: String): Long? =
                        try {
                            File("/sys/class/net/$name/statistics/$which")
                                .readText()
                                .trim()
                                .toLongOrNull()
                                ?.takeIf { it >= 0 }
                        } catch (_: Exception) {
                            null
                        }
                    obj(
                        "name" to name,
                        "addresses" to
                            array(
                                props.linkAddresses.take(32).map {
                                    "${it.address.hostAddress}/${it.prefixLength}"
                                }
                            ),
                        "mac" to null,
                        "mtu" to
                            if (Build.VERSION.SDK_INT >= 29) props.mtu.takeIf { it > 0 } else null,
                        "state" to
                            if (network == connectivity.activeNetwork) "active" else "available",
                        "received_bytes" to counter("rx_bytes"),
                        "transmitted_bytes" to counter("tx_bytes"),
                    )
                }
                .sortedBy { it.getString("name") }
        val limit = query.getInt("limit")
        val selected = mutableListOf<JSONObject>()
        var bytes = 0
        for (entry in entries) {
            val size = entry.toString().toByteArray(Charsets.UTF_8).size
            if (selected.size >= limit || bytes + size > 22000) break
            selected.add(entry)
            bytes += size
        }
        return response(
            obj("type" to "networks", "entries" to array(selected)),
            selected.size,
            listOf(
                "ConnectivityManager-visible networks only; MAC is unavailable. Interface counters are null when Android denies access."
            ),
            entries.size > selected.size,
        )
    }

    private suspend fun processes(query: JSONObject): JSONObject {
        val single = query.getString("action") == "process"
        val own = ownProcess(query.optBoolean("sample_cpu"))
        val pid = if (query.isNull("pid")) null else query.getInt("pid")
        if (single && pid != Process.myPid()) throw StorageFailure("process_not_visible_to_app")
        val name = query.optionalString("name")
        val user = query.optionalString("user")
        val selected =
            (pid == null || pid == Process.myPid()) &&
                (name == null || context.packageName.contains(name, true)) &&
                (user == null || user == "uid:${Process.myUid()}" || user == context.packageName)
        val entries = if (selected) listOf(own) else emptyList()
        return response(
            if (single) obj("type" to "process", "process" to own)
            else obj("type" to "processes", "entries" to array(entries)),
            if (single) 1 else entries.size,
            listOf(
                "Only this application's current process is observable. This is not the device-wide process inventory; termination is unavailable."
            ),
        )
    }

    private fun resolve(query: JSONObject): JSONObject {
        val type = query.getString("record_type")
        if (type != "A" && type != "AAAA")
            throw StorageFailure("android_dns_record_type_unavailable")
        val name = query.getString("name")
        val task =
            try {
                dns.submit<Array<InetAddress>> { InetAddress.getAllByName(name) }
            } catch (_: java.util.concurrent.RejectedExecutionException) {
                throw StorageFailure("dns_busy")
            }
        val addresses =
            try {
                task.get(query.getLong("timeout_ms"), TimeUnit.MILLISECONDS).toList()
            } catch (_: java.util.concurrent.TimeoutException) {
                task.cancel(true)
                throw StorageFailure("dns_timeout")
            } catch (_: java.util.concurrent.ExecutionException) {
                throw StorageFailure("dns_lookup_failed")
            }
        val selected =
            addresses
                .filter { if (type == "A") it is Inet4Address else it is Inet6Address }
                .mapNotNull { it.hostAddress }
                .distinct()
                .sorted()
        val limit = query.getInt("limit")
        val records =
            selected.take(limit).map {
                obj(
                    "name" to name,
                    "record_type" to type,
                    "ttl_seconds" to null,
                    "value" to it,
                    "value_truncated" to false,
                )
            }
        return response(
            obj(
                "type" to "dns",
                "result" to
                    obj(
                        "requested_name" to name,
                        "query_name" to name,
                        "record_type" to type,
                        "resolver" to "android_native_getaddrinfo",
                        "hosts_file_consulted" to true,
                        "records" to array(records),
                    ),
            ),
            records.size,
            listOf(
                "Uses Android native address resolution (hosts/cache/VPN policy may apply). A/AAAA only; TTL is unavailable."
            ),
            selected.size > limit,
        )
    }
}
