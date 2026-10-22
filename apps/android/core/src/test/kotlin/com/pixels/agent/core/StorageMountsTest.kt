package com.pixels.agent.core

import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class StorageMountsTest {
    @Test
    fun refusesTraversalAndUnselectedMountsBeforeCallingProvider() {
        val mounts = StorageMounts { emptyMap() }
        for (path in
            listOf(
                "relative",
                "/workspace/../credentials",
                "/workspace/./a",
                "/workspace/a\\b",
                "/workspace//a",
                "/workspace/\u0000",
            )) {
            val failure = assertThrows(StorageFailure::class.java) { mounts.resolve(path) }
            assertTrue(path, failure.code == "invalid_path")
        }
        val missing =
            assertThrows(StorageFailure::class.java) { mounts.resolve("/credentials/seed") }
        assertTrue(missing.code == "mount_not_found")
    }

    @Test
    fun enforcesTheProtocolNameBudgetInUtf8Bytes() {
        assertTrue(StorageMounts.validName("中文文件.txt"))
        assertTrue(StorageMounts.validName("a".repeat(255)))
        assertFalse(StorageMounts.validName("a".repeat(256)))
        assertFalse(StorageMounts.validName("中".repeat(86)))
    }
}
