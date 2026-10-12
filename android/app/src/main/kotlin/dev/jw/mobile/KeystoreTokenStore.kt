package dev.jw.mobile

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.io.File
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * The device token, encrypted with an AES-GCM key that never leaves the Android Keystore.
 * The file holds `iv(12) + ciphertext`; backups are off, because the key is not backed up
 * and a restored file could not be read. A file that cannot be decrypted counts as "no device".
 */
class KeystoreTokenStore(context: Context) : TokenStore {
    private val file = File(context.filesDir, "device.bin")

    override fun load(): StoredDevice? {
        val blob = try {
            file.readBytes()
        } catch (_: Exception) {
            return null
        }
        return try {
            if (blob.size <= IV_LEN) error("short")
            val cipher = Cipher.getInstance(TRANSFORM)
            cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, blob, 0, IV_LEN))
            val text = String(cipher.doFinal(blob, IV_LEN, blob.size - IV_LEN), Charsets.UTF_8)
            decode(text)
        } catch (_: Exception) {
            clear()
            null
        }
    }

    override fun save(device: StoredDevice) {
        val cipher = Cipher.getInstance(TRANSFORM)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val ct = cipher.doFinal(encode(device).toByteArray(Charsets.UTF_8))
        file.writeBytes(cipher.iv + ct)
    }

    override fun clear() {
        file.delete()
        try {
            KeyStore.getInstance(PROVIDER).apply { load(null) }.deleteEntry(ALIAS)
        } catch (_: Exception) {
        }
    }

    private fun key(): SecretKey {
        val ks = KeyStore.getInstance(PROVIDER).apply { load(null) }
        (ks.getKey(ALIAS, null) as? SecretKey)?.let { return it }
        val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, PROVIDER)
        gen.init(
            KeyGenParameterSpec.Builder(
                ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build(),
        )
        return gen.generateKey()
    }

    internal companion object {
        /**
         * `v2\n<device>\n<tunnel host>\n<lan host:port>\n<lan fp>`; the first form, `<host>\n<device>`, is still read.
         * Neither holds anything the reader does not check again (REQ-62).
         */
        fun encode(d: StoredDevice): String =
            "v2\n${d.device}\n${d.host}\n${d.lan?.hostPort.orEmpty()}\n${d.lan?.fp.orEmpty()}"

        fun decode(text: String): StoredDevice {
            val lines = text.split('\n')
            if (lines[0] != "v2") {
                val (host, device) = text.split('\n', limit = 2)
                return StoredDevice(host, device)
            }
            val lan = lines.getOrNull(3)?.takeIf { it.isNotEmpty() }?.let { hp ->
                val i = hp.lastIndexOf(':')
                dev.jw.proto.Hosts.lan(hp.substring(0, i), hp.substring(i + 1).toInt(), lines.getOrElse(4) { "" })
            }
            val host = lines.getOrElse(2) { "" }.takeIf { it.isEmpty() || dev.jw.proto.Hosts.isTunnel(it) } ?: error("bad host")
            return StoredDevice(host, lines[1], lan)
        }

        private const val PROVIDER = "AndroidKeyStore"
        private const val ALIAS = "jw_device"
        private const val TRANSFORM = "AES/GCM/NoPadding"
        private const val IV_LEN = 12
    }
}
