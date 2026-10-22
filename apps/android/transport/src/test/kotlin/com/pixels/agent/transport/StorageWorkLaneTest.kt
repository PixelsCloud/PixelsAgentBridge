package com.pixels.agent.transport

import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

class StorageWorkLaneTest {
    @Test
    fun blockedJobsRemainBoundedAcrossReconnectAndCannotDeliverToNewGeneration() {
        val lane = StorageWorkLane()
        val first = lane.begin()
        val started = CountDownLatch(2)
        val release = CountDownLatch(1)
        val finished = CountDownLatch(2)
        val stale = java.util.concurrent.atomic.AtomicInteger()
        repeat(2) {
            assertTrue(
                lane.submit(first) { current ->
                    started.countDown()
                    try {
                        check(release.await(5, TimeUnit.SECONDS))
                        if (!current()) stale.incrementAndGet()
                    } finally {
                        finished.countDown()
                    }
                }
            )
        }
        try {
            assertTrue(started.await(5, TimeUnit.SECONDS))
            assertFalse(lane.submit(first) { fail("Third job was admitted") })
            lane.end(first)
            val second = lane.begin()
            assertFalse(lane.current(first))
            assertTrue(lane.current(second))
            assertFalse(lane.submit(second) { fail("Reconnect multiplied blocked jobs") })
            release.countDown()
            assertTrue(finished.await(5, TimeUnit.SECONDS))
            assertEquals(2, stale.get())
            val recovered = CountDownLatch(1)
            val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5)
            while (!lane.submit(second) { recovered.countDown() }) {
                check(System.nanoTime() < deadline)
                Thread.yield()
            }
            assertTrue(recovered.await(5, TimeUnit.SECONDS))
        } finally {
            release.countDown()
        }
    }
}
