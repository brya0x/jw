package dev.jw.next.client

import com.google.zxing.BarcodeFormat
import com.google.zxing.EncodeHintType
import com.google.zxing.qrcode.QRCodeWriter
import com.google.zxing.qrcode.decoder.ErrorCorrectionLevel
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

/** The scanner reads what `jwn pair` prints. */
class QrTest {
    private val link = "jwn://192.168.100.152:49211/298ab0a2575a934bd5ff000db05f0129?fp=" + "a6".repeat(32) + "&t=try-velocity-confidentiality-predicted.trycloudflare.com"

    /** A frame as a camera hands it over: [scale] pixels per module, [pad] bytes of row padding, optionally light on dark. */
    private fun frame(text: String, scale: Int, pad: Int, inverted: Boolean): Triple<ByteArray, Int, Int> {
        val m = QRCodeWriter().encode(text, BarcodeFormat.QR_CODE, 0, 0, mapOf(EncodeHintType.ERROR_CORRECTION to ErrorCorrectionLevel.L, EncodeHintType.MARGIN to 4))
        val width = m.width * scale
        val height = m.height * scale
        val stride = width + pad
        val bytes = ByteArray(stride * height) { 127 }
        for (y in 0 until height) for (x in 0 until width) {
            val dark = m.get(x / scale, y / scale)
            bytes[y * stride + x] = if (dark != inverted) 0 else 255.toByte()
        }
        return Triple(bytes, stride, width)
    }

    @Test
    fun theLinkOfJwnPairIsReadFromAFrame() {
        val (bytes, stride, width) = frame(link, 4, 0, false)
        assertEquals(link, decodeQr(bytes, stride, width, bytes.size / stride))
    }

    @Test
    fun rowPaddingAndALightOnDarkCodeAreRead() {
        val (padded, stride, width) = frame(link, 4, 24, false)
        assertEquals(link, decodeQr(padded, stride, width, padded.size / stride))
        val (inverted, s2, w2) = frame(link, 4, 0, true)
        assertEquals(link, decodeQr(inverted, s2, w2, inverted.size / s2))
    }

    @Test
    fun aFrameWithNoCodeIsNull() {
        val blank = ByteArray(320 * 240) { 128.toByte() }
        assertNull(decodeQr(blank, 320, 320, 240))
    }
}
