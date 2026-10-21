package dev.jw.next.app

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Matrix
import android.media.ExifInterface
import android.net.Uri
import android.util.Base64
import dev.jw.next.client.IMAGE_MAX_BYTES
import dev.jw.next.client.IMAGE_MAX_SIDE
import dev.jw.next.client.fitSize
import dev.jw.next.client.pictureLabel
import dev.jw.next.contract.Image
import java.io.ByteArrayOutputStream

/** A picture attached to the message being written: made small, as the daemon takes it, with the mark in the line (REQ-447). */
internal data class Picture(val n: Int, val image: Image, val width: Int, val height: Int, val bytes: Int) {
    val label: String get() = pictureLabel(n, width, height, bytes)
}

/**
 * The picture at [uri] as a JPEG of at most [IMAGE_MAX_SIDE] pixels on its long side and [IMAGE_MAX_BYTES] bytes. Reading it
 * is work for a background thread. The photo is turned upright by what its metadata says, as it would be on a screen.
 */
internal fun readPicture(context: Context, uri: Uri, n: Int): Result<Picture> = runCatching {
    val resolver = context.contentResolver
    val orientation = resolver.openInputStream(uri)?.use { ExifInterface(it).getAttributeInt(ExifInterface.TAG_ORIENTATION, ExifInterface.ORIENTATION_NORMAL) }
        ?: ExifInterface.ORIENTATION_NORMAL
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    resolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, bounds) }
    // Decode at half, a quarter… of the size while it stays above the limit: a 12-megapixel photo is not held whole.
    var sample = 1
    while (maxOf(bounds.outWidth, bounds.outHeight) / (sample * 2) >= IMAGE_MAX_SIDE) sample *= 2
    val decoded = resolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, BitmapFactory.Options().apply { inSampleSize = sample }) }
        ?: error("no se pudo leer la imagen")
    val (w, h) = fitSize(decoded.width, decoded.height)
    val scaled = if (w == decoded.width && h == decoded.height) decoded else Bitmap.createScaledBitmap(decoded, w, h, true)
    val turn = when (orientation) {
        ExifInterface.ORIENTATION_ROTATE_90 -> 90f
        ExifInterface.ORIENTATION_ROTATE_180 -> 180f
        ExifInterface.ORIENTATION_ROTATE_270 -> 270f
        else -> 0f
    }
    val upright = if (turn == 0f) scaled else Bitmap.createBitmap(scaled, 0, 0, scaled.width, scaled.height, Matrix().apply { postRotate(turn) }, true)
    var quality = 85
    var bytes: ByteArray
    do {
        bytes = ByteArrayOutputStream().also { upright.compress(Bitmap.CompressFormat.JPEG, quality, it) }.toByteArray()
        quality -= 15
    } while (bytes.size > IMAGE_MAX_BYTES && quality > 10)
    if (bytes.size > IMAGE_MAX_BYTES) error("la imagen pesa demasiado")
    Picture(n, Image("image/jpeg", Base64.encodeToString(bytes, Base64.NO_WRAP)), upright.width, upright.height, bytes.size)
}
