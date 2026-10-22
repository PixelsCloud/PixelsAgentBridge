package com.pixels.agent.platform.storage

import com.pixels.agent.core.AtomicFilePublisher
import com.pixels.agent.core.StorageFailure
import com.pixels.agent.core.StorageMounts
import java.io.ByteArrayInputStream
import java.nio.file.Files
import java.security.MessageDigest
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class WorkspaceEntryTest {
    @get:Rule val temporary = TemporaryFolder()

    private fun workspace(root: java.io.File): WorkspaceEntry =
        WorkspaceEntry(
            root,
            publisher =
                AtomicFilePublisher { source, destination, replace ->
                    if (replace) {
                        Files.move(
                            source.toPath(),
                            destination.toPath(),
                            java.nio.file.StandardCopyOption.ATOMIC_MOVE,
                            java.nio.file.StandardCopyOption.REPLACE_EXISTING,
                        )
                    } else {
                        Files.createLink(destination.toPath(), source.toPath())
                    }
                },
        )

    @Test
    fun createsChineseFileAndPreservesItOnOverwriteOrHashConflict() {
        val root = temporary.newFolder("workspace")
        val mounts = StorageMounts { mapOf("workspace" to workspace(root)) }
        val entry = mounts.resolve("/workspace/中文.txt")
        val original = "第一行\n第二行".toByteArray()
        entry.publish({ ByteArrayInputStream(original) }, false, null)
        assertArrayEquals(original, entry.openRead().use { it.readBytes() })
        assertThrows(Exception::class.java) {
            entry.publish({ ByteArrayInputStream(byteArrayOf(1, 2, 3)) }, false, null)
        }
        val conflict =
            assertThrows(StorageFailure::class.java) {
                entry.publish({ ByteArrayInputStream(byteArrayOf(4)) }, true, "0".repeat(64))
            }
        assertEquals("hash_conflict", conflict.code)
        assertArrayEquals(original, entry.openRead().use { it.readBytes() })
        val hash =
            MessageDigest.getInstance("SHA-256").digest(original).joinToString("") {
                "%02x".format(it.toInt() and 255)
            }
        entry.publish({ ByteArrayInputStream(byteArrayOf(5)) }, true, hash)
        assertArrayEquals(byteArrayOf(5), entry.openRead().use { it.readBytes() })
        assertEquals(listOf("中文.txt"), root.listFiles()!!.map { it.name })
    }

    @Test
    fun failedSourceDoesNotPublishAnIncompleteFileOrDamageExistingBytes() {
        val root = temporary.newFolder("workspace")
        val entry = workspace(root).child("file.bin")
        entry.publish({ ByteArrayInputStream(byteArrayOf(9)) }, false, null)
        assertThrows(IllegalStateException::class.java) {
            entry.publish({ throw IllegalStateException("test source failure") }, true, null)
        }
        assertArrayEquals(byteArrayOf(9), entry.openRead().use { it.readBytes() })
        assertEquals(listOf("file.bin"), root.listFiles()!!.map { it.name })
    }

    @Test
    fun lostAuthorizationAtPublicationPreservesOriginalAndReleasesStage() {
        val root = temporary.newFolder("workspace")
        val entry = workspace(root).child("file.txt")
        entry.publish({ ByteArrayInputStream("original".toByteArray()) }, false, null)
        val denied = { throw StorageFailure("connection_changed") }
        assertEquals(
            "connection_changed",
            assertThrows(StorageFailure::class.java) {
                    entry.publish(
                        { ByteArrayInputStream("late bytes".toByteArray()) },
                        true,
                        null,
                        denied,
                    )
                }
                .code,
        )
        assertArrayEquals("original".toByteArray(), entry.openRead().use { it.readBytes() })
        assertEquals(0, root.parentFile.resolve("file-staging").listFiles()!!.size)
        assertThrows(StorageFailure::class.java) { entry.delete(denied) }
        assertArrayEquals("original".toByteArray(), entry.openRead().use { it.readBytes() })
        assertThrows(StorageFailure::class.java) {
            workspace(root).child("late-directory").mkdir(false, false, denied)
        }
        assertEquals(listOf("file.txt"), root.listFiles()!!.map { it.name })
    }

    @Test
    fun cannotDeleteMountRoot() {
        val root = workspace(temporary.newFolder("workspace"))
        assertEquals(
            "mount_root_protected",
            assertThrows(StorageFailure::class.java) { root.delete() }.code,
        )
    }

    @Test
    fun rejectsSymlinkInsteadOfReadingOutsideWorkspace() {
        val root = temporary.newFolder("workspace")
        val outside = temporary.newFile("private-key").apply { writeText("test-only") }
        try {
            Files.createSymbolicLink(root.resolve("alias").toPath(), outside.toPath())
        } catch (error: Exception) {
            org.junit.Assume.assumeNoException(
                "Windows needs symlink permission; also verify on Android",
                error,
            )
        }
        val alias = workspace(root).child("alias")
        assertEquals(
            "symlink_not_allowed",
            assertThrows(StorageFailure::class.java) { alias.openRead() }.code,
        )
    }
}
