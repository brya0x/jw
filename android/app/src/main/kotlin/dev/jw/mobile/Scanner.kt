package dev.jw.mobile

import android.view.ViewGroup
import androidx.camera.core.Camera
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.ImageProxy
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import com.google.zxing.BarcodeFormat
import com.google.zxing.BinaryBitmap
import com.google.zxing.DecodeHintType
import com.google.zxing.MultiFormatReader
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.common.HybridBinarizer
import java.util.concurrent.Executors

/** Decodes QR codes from camera frames with ZXing (pure Java: no Play services, no native code). */
private class QrAnalyzer(private val onText: (String) -> Unit) : ImageAnalysis.Analyzer {
    private val reader = MultiFormatReader().apply {
        setHints(mapOf(DecodeHintType.POSSIBLE_FORMATS to listOf(BarcodeFormat.QR_CODE), DecodeHintType.TRY_HARDER to true))
    }

    override fun analyze(image: ImageProxy) {
        try {
            val plane = image.planes[0]
            val w = image.width
            val h = image.height
            val buf = plane.buffer
            val stride = plane.rowStride
            // The Y plane's rows can be wider than the image: pack them before decoding.
            val y = ByteArray(w * h)
            for (row in 0 until h) {
                buf.position(row * stride)
                buf.get(y, row * w, w)
            }
            val src = PlanarYUVLuminanceSource(y, w, h, 0, 0, w, h, false)
            val result = reader.decodeWithState(BinaryBitmap(HybridBinarizer(src)))
            onText(result.text)
        } catch (_: Exception) {
            // no code in this frame
        } finally {
            reader.reset()
            image.close()
        }
    }
}

@Composable
fun QrScanner(
    modifier: Modifier = Modifier,
    torch: Boolean = false,
    onTorchAvailable: (Boolean) -> Unit = {},
    onText: (String) -> Unit,
) {
    val context = LocalContext.current
    val owner = LocalLifecycleOwner.current
    val latest = rememberUpdatedState(onText)
    val availability = rememberUpdatedState(onTorchAvailable)
    val camera = remember { mutableStateOf<Camera?>(null) }
    // The flash is switched on only while the scan screen shows the camera; unbinding turns it off.
    LaunchedEffect(torch, camera.value) { camera.value?.cameraControl?.enableTorch(torch) }
    val executor = remember { Executors.newSingleThreadExecutor() }
    val main = remember { ContextCompat.getMainExecutor(context) }
    DisposableEffect(Unit) {
        onDispose {
            executor.shutdown()
            // The camera is bound to the activity: release it when the scan screen leaves.
            ProcessCameraProvider.getInstance(context).let { f -> f.addListener({ f.get().unbindAll() }, main) }
        }
    }
    AndroidView(
        modifier = modifier,
        factory = { ctx ->
            val view = PreviewView(ctx).apply {
                layoutParams = ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT)
            }
            val future = ProcessCameraProvider.getInstance(ctx)
            future.addListener({
                val provider = future.get()
                val preview = Preview.Builder().build().also { it.surfaceProvider = view.surfaceProvider }
                val analysis = ImageAnalysis.Builder()
                    .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                    .build()
                    .also { it.setAnalyzer(executor, QrAnalyzer { t -> main.execute { latest.value(t) } }) }
                provider.unbindAll()
                val cam = provider.bindToLifecycle(owner, CameraSelector.DEFAULT_BACK_CAMERA, preview, analysis)
                camera.value = cam
                availability.value(cam.cameraInfo.hasFlashUnit())
            }, main)
            view
        },
    )
}
