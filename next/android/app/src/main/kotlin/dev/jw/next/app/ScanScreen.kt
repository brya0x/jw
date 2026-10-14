package dev.jw.next.app

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.util.Size
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.Preview
import androidx.camera.core.resolutionselector.ResolutionSelector
import androidx.camera.core.resolutionselector.ResolutionStrategy
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import dev.jw.next.client.decodeQr
import dev.jw.next.client.parsePairLink
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Scan the QR that `jwn pair` shows: ask for the camera (and say so when it is blocked), then
 * read frames until one is a pairing link. [link] gets the text of that QR.
 */
@Composable
fun ScanFlow(link: (String) -> Unit, paste: () -> Unit, back: () -> Unit) {
    val context = LocalContext.current
    var granted by remember { mutableStateOf(ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) }
    var blocked by remember { mutableStateOf(false) }
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok ->
        granted = ok
        blocked = !ok
    }
    when {
        granted -> ScanScreen(link, paste, back)
        blocked -> PermissionScreen(
            "la cámara está bloqueada",
            "Actívala en los ajustes del sistema, o pega el enlace que muestra jwn pair.",
            "abrir ajustes del sistema",
            { context.startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", context.packageName, null))) },
            paste,
            back,
        )
        else -> PermissionScreen(
            "necesito la cámara",
            "Solo para leer el QR que muestra jwn pair en tu PC. No guarda fotos ni vídeo.",
            "permitir cámara",
            { ask.launch(Manifest.permission.CAMERA) },
            paste,
            back,
        )
    }
}

@Composable
private fun PermissionScreen(title: String, body: String, action: String, act: () -> Unit, paste: () -> Unit, back: () -> Unit) {
    Column(Modifier.fillMaxSize()) {
        Header("escanear código", "jwn pair", onBack = back)
        Column(Modifier.padding(horizontal = 24.dp, vertical = 32.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Mono(title, Palette.fg, 20, bold = true, lines = 3)
            Mono(body, Palette.dim, 13, lines = 8)
            Button(action, true, act)
            Button("pegar enlace", false, paste)
        }
    }
}

@Composable
private fun ScanScreen(link: (String) -> Unit, paste: () -> Unit, back: () -> Unit) {
    var notOurs by remember { mutableStateOf(false) }
    val busy = remember { AtomicBoolean(false) }
    val main = remember { Handler(Looper.getMainLooper()) }
    Column(Modifier.fillMaxSize().navigationBarsPadding()) {
        Header("escanear código", "jwn pair", onBack = back)
        Box(Modifier.weight(1f).fillMaxWidth().background(Palette.panel)) {
            CameraView(
                Modifier.fillMaxSize(),
            ) { text ->
                // Frames keep coming while a code is handled: one at a time.
                if (busy.compareAndSet(false, true)) {
                    main.post {
                        if (parsePairLink(text, BuildConfig.LINK_SCHEME) != null) {
                            link(text)
                        } else {
                            notOurs = true
                            main.postDelayed({ notOurs = false; busy.set(false) }, 2000)
                        }
                    }
                }
            }
            Box(
                Modifier.align(Alignment.Center).size(260.dp).border(2.dp, if (notOurs) Palette.red else Palette.blue, RoundedCornerShape(16.dp)),
            )
            Mono(
                if (notOurs) "ese QR no es de jwn" else "apunta al QR que muestra jwn pair",
                if (notOurs) Palette.red else Palette.fg,
                13,
                modifier = Modifier.align(Alignment.BottomCenter).padding(bottom = 20.dp).background(Palette.panel, RoundedCornerShape(8.dp)).padding(horizontal = 12.dp, vertical = 6.dp),
            )
        }
        Column(Modifier.padding(16.dp)) { Button("pegar enlace", false, paste) }
    }
}

/** The back camera, with every frame offered to [onText] when it holds a QR code. */
@Composable
private fun CameraView(modifier: Modifier, onText: (String) -> Unit) {
    val owner = LocalLifecycleOwner.current
    val executor = remember { Executors.newSingleThreadExecutor() }
    var provider by remember { mutableStateOf<ProcessCameraProvider?>(null) }
    DisposableEffect(Unit) {
        onDispose {
            provider?.unbindAll()
            executor.shutdown()
        }
    }
    AndroidView(modifier = modifier, factory = { ctx ->
        val preview = PreviewView(ctx).apply { scaleType = PreviewView.ScaleType.FILL_CENTER }
        val future = ProcessCameraProvider.getInstance(ctx)
        future.addListener({
            val camera = future.get()
            provider = camera
            val view = Preview.Builder().build().also { it.surfaceProvider = preview.surfaceProvider }
            // A code printed in a terminal is dense: a higher resolution than the default reads it from farther away.
            val analysis = ImageAnalysis.Builder()
                .setResolutionSelector(ResolutionSelector.Builder().setResolutionStrategy(ResolutionStrategy(Size(1280, 720), ResolutionStrategy.FALLBACK_RULE_CLOSEST_HIGHER_THEN_LOWER)).build())
                .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                .build()
            analysis.setAnalyzer(executor) { image ->
                try {
                    val plane = image.planes[0]
                    val bytes = ByteArray(plane.buffer.remaining()).also { plane.buffer.get(it) }
                    decodeQr(bytes, plane.rowStride, image.width, image.height)?.let(onText)
                } finally {
                    image.close()
                }
            }
            camera.unbindAll()
            camera.bindToLifecycle(owner, CameraSelector.DEFAULT_BACK_CAMERA, view, analysis)
        }, ContextCompat.getMainExecutor(ctx))
        preview
    })
}
