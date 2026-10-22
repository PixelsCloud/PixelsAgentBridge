package com.pixels.agent.transport

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import com.pixels.agent.core.DeviceAccessPassword
import java.io.File
import java.nio.ByteBuffer
import java.security.KeyStore
import java.security.MessageDigest
import java.security.SecureRandom
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import org.json.JSONObject

/** The Ed25519 seed is wrapped by Keystore AES-GCM, never written in plaintext. */
internal class EndpointCredentialStore(private val directory: File) {
    fun access(scope: String, native: NativeBridge): JSONObject {
        val stored = read(scope, "access")
        if (stored != null) {
            val access =
                try {
                    JSONObject(stored.toString(Charsets.UTF_8))
                } finally {
                    stored.fill(0)
                }
            val length = access.getString("password").length
            if (length == DeviceAccessPassword.LENGTH) return access
            // Upgrade only the known legacy format; never reset the device identity.
            check(length == 16) { "invalid_access_password" }
            return createAccess(scope, native, nextVersion(access))
        }
        return createAccess(scope, native, 1)
    }

    fun rotateAccess(scope: String, native: NativeBridge): JSONObject {
        val previous = access(scope, native)
        val next = createAccess(scope, native, nextVersion(previous))
        try {
            nativeValue(
                native.updateAccessPassword(next.getString("hash"), next.getLong("version"))
            )
        } catch (error: Exception) {
            val bytes = previous.toString().toByteArray(Charsets.UTF_8)
            try {
                write(scope, "access", bytes)
            } finally {
                bytes.fill(0)
            }
            throw error
        }
        return next
    }

    private fun nextVersion(access: JSONObject): Long {
        val version = access.getLong("version")
        check(version in 1 until Long.MAX_VALUE) { "invalid_password_version" }
        return version + 1
    }

    private fun createAccess(scope: String, native: NativeBridge, version: Long): JSONObject {
        val password = DeviceAccessPassword.generate()
        val access =
            JSONObject()
                .put("password", password)
                .put("hash", nativeValue(native.passwordHash(password)).getString("hash"))
                .put("version", version)
        val bytes = access.toString().toByteArray(Charsets.UTF_8)
        try {
            write(scope, "access", bytes)
        } finally {
            bytes.fill(0)
        }
        return access
    }

    fun seed(scope: String, allowCreate: Boolean): ByteArray {
        val stored = read(scope, "seed")
        if (stored != null) {
            check(stored.size == 32) { "invalid_identity" }
            return stored
        }
        check(allowCreate) { "identity_missing" }
        return ByteArray(32).also {
            SecureRandom().nextBytes(it)
            try {
                write(scope, "seed", it)
            } catch (error: Exception) {
                it.fill(0)
                throw error
            }
        }
    }

    fun registration(scope: String): String? =
        read(scope, "registration")?.let { bytes ->
            try {
                bytes.toString(Charsets.UTF_8)
            } finally {
                bytes.fill(0)
            }
        }

    fun saveRegistration(scope: String, registration: String) {
        val bytes = registration.toByteArray(Charsets.UTF_8)
        try {
            write(scope, "registration", bytes)
        } finally {
            bytes.fill(0)
        }
    }

    private fun file(scope: String, purpose: String): AtomicFile {
        val digest = MessageDigest.getInstance("SHA-256").digest(scope.toByteArray(Charsets.UTF_8))
        val name = digest.joinToString("") { "%02x".format(it.toInt() and 0xff) }
        return AtomicFile(File(directory, "$name-$purpose.bin"))
    }

    private fun read(scope: String, purpose: String): ByteArray? {
        val envelope =
            try {
                file(scope, purpose).openRead().use { it.readBounded(8192) }
            } catch (_: java.io.FileNotFoundException) {
                return null
            }
        check(envelope.size in 30..8192 && envelope[0] == 1.toByte()) { "invalid_credential" }
        // A missing wrapping key with existing ciphertext is an error, never an identity reset.
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(
            Cipher.DECRYPT_MODE,
            wrappingKey(create = false),
            GCMParameterSpec(128, envelope.copyOfRange(1, 13)),
        )
        cipher.updateAAD("pixels-v1:$scope:$purpose".toByteArray(Charsets.UTF_8))
        return cipher.doFinal(envelope, 13, envelope.size - 13)
    }

    private fun write(scope: String, purpose: String, plaintext: ByteArray) {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, wrappingKey(create = true))
        check(cipher.iv.size == 12)
        cipher.updateAAD("pixels-v1:$scope:$purpose".toByteArray(Charsets.UTF_8))
        val ciphertext = cipher.doFinal(plaintext)
        val envelope =
            ByteBuffer.allocate(1 + 12 + ciphertext.size)
                .put(1.toByte())
                .put(cipher.iv)
                .put(ciphertext)
                .array()
        directory.mkdirs()
        val target = file(scope, purpose)
        val stream = target.startWrite()
        try {
            stream.write(envelope)
            target.finishWrite(stream)
        } catch (error: Exception) {
            target.failWrite(stream)
            throw error
        }
    }

    private fun wrappingKey(create: Boolean): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        val existing = store.getKey(KEY_ALIAS, null)
        if (existing != null) return existing as SecretKey
        check(create) { "credential_key_missing" }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").run {
            init(
                KeyGenParameterSpec.Builder(
                        KEY_ALIAS,
                        KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
                    )
                    .setKeySize(256)
                    .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                    .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                    .build()
            )
            generateKey()
        }
    }

    private companion object {
        const val KEY_ALIAS = "pixels.endpoint.wrap.v1"
    }
}
