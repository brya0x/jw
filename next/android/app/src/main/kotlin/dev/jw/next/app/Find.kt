package dev.jw.next.app

import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
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
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.jw.next.client.ClientState
import dev.jw.next.client.Navigator
import dev.jw.next.client.findWorkspaces
import dev.jw.next.client.shortPath

/**
 * The search of workspaces (REQ-231 to REQ-233), as `docs/prototypes/mobile.html` draws it: a
 * field over every workspace, the ones opened lately first; a tap opens one.
 */
@Composable
internal fun BoxScope.FindSheet(source: AppSource, state: ClientState, nav: Navigator, dismiss: () -> Unit, open: (ClientState) -> Unit) {
    val version = source.tick.intValue
    val session = remember(version) { source.session() }
    var text by remember { mutableStateOf("") }
    val found = remember(version, text, state.recent, state.seen) { findWorkspaces(session, state.seen, state.recent, text) }
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { focus.requestFocus() }
    Sheet(dismiss) {
        Mono("workspaces por último uso", Palette.fg, 14, bold = true)
        BasicTextField(
            value = text,
            onValueChange = { text = it },
            singleLine = true,
            textStyle = TextStyle(color = Palette.fg, fontFamily = MonoFont, fontSize = 14.sp),
            cursorBrush = SolidColor(Palette.blue),
            keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false, imeAction = ImeAction.Search),
            modifier = Modifier.fillMaxWidth().focusRequester(focus),
            decorationBox = { inner ->
                Box(Modifier.fillMaxWidth().defaultMinSize(minHeight = 44.dp).border(1.dp, Palette.blue, RoundedCornerShape(6.dp)).padding(horizontal = 12.dp), contentAlignment = Alignment.CenterStart) {
                    if (text.isEmpty()) Mono("buscar…", Palette.dim, 14)
                    inner()
                }
            },
        )
        if (found.isEmpty()) Mono("sin resultados", Palette.dim, 13, modifier = Modifier.padding(vertical = 12.dp))
        LazyColumn(Modifier.heightIn(max = 340.dp)) {
            items(found, key = { it.row.id }) { f ->
                val row = f.row
                Row(
                    Modifier.fillMaxWidth().defaultMinSize(minHeight = 52.dp).clickable { open(nav.openWorkspace(state, row.id)) }.padding(vertical = 4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Mono("●", if (row.paneCount > 0) Palette.green else Palette.dim, 14, modifier = Modifier.width(12.dp))
                    Column(Modifier.weight(1f)) {
                        Mono(row.name, bold = true, size = 14)
                        Mono(shortPath(f.path), Palette.dim, 11)
                    }
                    Tag(row.kind.label, Palette.dim)
                    row.mark?.let { m -> Mono(m.glyph, m.tone.color(), 14, bold = true) }
                }
            }
        }
    }
}
