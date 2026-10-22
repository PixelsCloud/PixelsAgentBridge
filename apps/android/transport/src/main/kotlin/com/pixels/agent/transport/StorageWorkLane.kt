package com.pixels.agent.transport

import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Semaphore

/** App-lifetime bounded IO: a provider ignoring cancellation cannot multiply on reconnect. */
internal class StorageWorkLane {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val slots = Semaphore(2)
    private val generation = AtomicLong()

    fun begin(): Long = generation.incrementAndGet()

    fun end(epoch: Long) {
        generation.compareAndSet(epoch, epoch + 1)
    }

    fun current(epoch: Long): Boolean = generation.get() == epoch

    fun submit(epoch: Long, work: suspend (() -> Boolean) -> Unit): Boolean {
        if (!current(epoch) || !slots.tryAcquire()) return false
        scope.launch {
            try {
                if (current(epoch)) work { current(epoch) }
            } finally {
                slots.release()
            }
        }
        return true
    }
}
