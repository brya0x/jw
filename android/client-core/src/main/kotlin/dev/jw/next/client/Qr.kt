package dev.jw.next.client

import com.google.zxing.BarcodeFormat
import com.google.zxing.BinaryBitmap
import com.google.zxing.DecodeHintType
import com.google.zxing.NotFoundException
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeReader

private val hints = mapOf(
    DecodeHintType.POSSIBLE_FORMATS to listOf(BarcodeFormat.QR_CODE),
    DecodeHintType.TRY_HARDER to true,
)

/**
 * The text of the QR code in a camera frame, or null when there is none. [luma] is the Y plane
 * of the frame: [rowStride] bytes per row, of which the first [width] are pixels, [height] rows.
 */
fun decodeQr(luma: ByteArray, rowStride: Int, width: Int, height: Int): String? {
    val source = PlanarYUVLuminanceSource(luma, rowStride, height, 0, 0, width, height, false)
    // A terminal draws light modules on a dark background, so the inverted frame is tried too.
    return read(BinaryBitmap(HybridBinarizer(source))) ?: read(BinaryBitmap(HybridBinarizer(source.invert())))
}

private fun read(bitmap: BinaryBitmap): String? = try {
    QRCodeReader().decode(bitmap, hints).text
} catch (_: NotFoundException) {
    null
} catch (_: com.google.zxing.ChecksumException) {
    null
} catch (_: com.google.zxing.FormatException) {
    null
}
