package dev.jw.next.app

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.border
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.jw.next.client.NodeSource
import dev.jw.next.contract.Id

private val SheetGutter = 16.dp

/** A bottom sheet over a dimmed screen: a tap outside it, or Back, dismisses it. */
@Composable
internal fun BoxScope.Sheet(dismiss: () -> Unit, content: @Composable ColumnScope.() -> Unit) {
    BackHandler(onBack = dismiss)
    Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.55f)).clickable(onClick = dismiss))
    Column(
        Modifier.align(Alignment.BottomCenter).fillMaxWidth().background(Palette.panel, RoundedCornerShape(topStart = 14.dp, topEnd = 14.dp))
            .navigationBarsPadding().imePadding().padding(SheetGutter),
        verticalArrangement = Arrangement.spacedBy(12.dp),
        content = content,
    )
}

/** One choice of a sheet: a glyph in a key and what it does. */
@Composable
internal fun SheetAction(glyph: String, text: String, color: Color = Palette.fg, enabled: Boolean = true, click: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().defaultMinSize(minHeight = 48.dp).clickable(enabled = enabled, onClick = click),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Mono(glyph, color, 14, bold = true, modifier = Modifier.width(24.dp))
        Mono(text, color, 14, bold = color != Palette.fg)
    }
}

/** What a sheet gives a name to: a workspace or a pane, with the name as it is shown. */
internal sealed interface Naming {
    val shown: String

    data class Workspace(val id: Id, override val shown: String) : Naming
    data class Pane(val id: Id, override val shown: String) : Naming
}

/**
 * The field of a name (REQ-184): the name as shown, `guardar` sends it (empty: the automatic
 * one), and what the PC refuses with stays on the sheet.
 */
@Composable
internal fun BoxScope.RenameSheet(source: NodeSource, naming: Naming, dismiss: () -> Unit) {
    var text by remember(naming) { mutableStateOf(TextFieldValue(naming.shown, TextRange(naming.shown.length))) }
    var error by remember(naming) { mutableStateOf<String?>(null) }
    var busy by remember(naming) { mutableStateOf(false) }
    val focus = remember { FocusRequester() }
    LaunchedEffect(naming) { focus.requestFocus() }
    val save = {
        if (!busy) {
            busy = true
            error = null
            val done: (Result<Unit>) -> Unit = { result ->
                busy = false
                result.onSuccess { dismiss() }.onFailure { error = it.message ?: "el PC no pudo guardar el nombre" }
            }
            when (naming) {
                is Naming.Workspace -> source.renameWorkspace(naming.id, text.text, done)
                is Naming.Pane -> source.renamePane(naming.id, text.text, done)
            }
        }
    }
    Sheet(dismiss) {
        val workspace = naming is Naming.Workspace
        Mono(if (workspace) "renombrar el workspace" else "nombrar el panel", Palette.fg, 14, bold = true)
        BasicTextField(
            value = text,
            onValueChange = { text = it },
            singleLine = true,
            textStyle = TextStyle(color = Palette.fg, fontFamily = MonoFont, fontSize = 14.sp),
            cursorBrush = SolidColor(Palette.blue),
            keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false, imeAction = ImeAction.Done),
            keyboardActions = KeyboardActions(onDone = { save() }),
            modifier = Modifier.fillMaxWidth().focusRequester(focus),
            decorationBox = { inner ->
                Box(Modifier.fillMaxWidth().defaultMinSize(minHeight = 44.dp).border(1.dp, Palette.blue, RoundedCornerShape(6.dp)).padding(horizontal = 12.dp), contentAlignment = Alignment.CenterStart) {
                    if (text.text.isEmpty()) Mono(if (workspace) "nombre del workspace" else "nombre del panel", Palette.dim, 14)
                    inner()
                }
            },
        )
        Mono(
            (if (workspace) "Cambia el nombre que se ve, aquí y en el PC. La carpeta y la rama no se tocan. " else "El panel conserva este nombre aunque cambie lo que corre en él. ") +
                "Vacío: vuelve el nombre automático.",
            Palette.dim, 12, lines = 4,
        )
        if (error != null) Mono(error.orEmpty(), Palette.red, 12, lines = 4)
        SheetAction("↵", if (busy) "guardando…" else "guardar", Palette.blue, enabled = !busy, click = save)
    }
}
