package dev.jw.mobile

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/** What identifies a question, so one the person closed with Esc does not open again while it stays on the screen. */
internal fun AskText.key(): String = question + "\u0000" + options.joinToString("\u0001") { it.label }

/**
 * An agent's question over the whole pane (REQ-95): no keyboard, no composer, no keys and no approve or reject. One choice of
 * several (a tap sends it), several choices (a tap marks one, `Enviar` sends them), or a plan (its text, then the decisions
 * under it). [plan] is the plan's text when the agent asks to go ahead with one. [onPick] takes an option's number, [onSend]
 * confirms the marked ones, [onOther] sends a written answer through the option that takes one, and [onEsc] cancels.
 */
@Composable
fun AskView(
    ask: AskText, plan: String?, who: String, onPick: (Int) -> Unit, onSend: () -> Unit, onOther: (String) -> Unit, onEsc: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val j = LocalJw.current
    var writing by remember(ask.key()) { mutableStateOf(false) }
    var typed by remember(ask.key()) { mutableStateOf("") }
    val kicker = when {
        plan != null -> "Plan"
        ask.multi -> "Elige varias"
        else -> "Pregunta"
    }
    Column(modifier.background(j.bg)) {
        Row(
            Modifier.fillMaxWidth().background(j.sel).heightIn(min = 48.dp).padding(start = 16.dp, end = 4.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Box(Modifier.size(8.dp).background(if (plan != null) j.magenta else j.yellow, CircleShape))
            Text(if (plan != null) "$who propone un plan" else if (ask.multi) "$who te pregunta · elige varias" else "$who te pregunta", color = j.fg, fontSize = 12.sp, modifier = Modifier.weight(1f))
            Text(
                "✕ esc", color = j.dim, fontSize = 12.sp,
                modifier = Modifier.heightIn(min = 48.dp).clickable(role = Role.Button, onClickLabel = "Cancelar con Escape", onClick = onEsc).padding(horizontal = 14.dp, vertical = 15.dp),
            )
        }
        Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(kicker.uppercase(), color = j.dim, fontSize = 11.sp, letterSpacing = 0.5.sp)
            if (plan != null) {
                Text(plan, color = j.fg, fontSize = 12.5.sp, lineHeight = 20.sp)
            } else {
                Text(ask.question, color = j.fg, fontSize = 17.sp, lineHeight = 24.sp, fontWeight = FontWeight.Bold)
                for (o in ask.options) {
                    if (o === ask.other && writing) continue
                    Choice(o, ask.multi) { if (o === ask.other) writing = true else onPick(o.number) }
                }
                if (writing) {
                    BasicTextField(
                        value = typed, onValueChange = { typed = it }, singleLine = true,
                        textStyle = TextStyle(color = j.fg, fontSize = 14.sp, fontFamily = JwMono), cursorBrush = SolidColor(j.fg),
                        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send), keyboardActions = KeyboardActions(onSend = { if (typed.isNotBlank()) onOther(typed) }),
                        modifier = Modifier.fillMaxWidth(),
                        decorationBox = { inner ->
                            Box(Modifier.fillMaxWidth().heightIn(min = 52.dp).border(BorderStroke(2.dp, j.blue), RoundedCornerShape(12.dp)).padding(horizontal = 14.dp), contentAlignment = Alignment.CenterStart) {
                                if (typed.isEmpty()) Text("Otra respuesta…", color = j.dim, fontSize = 14.sp)
                                inner()
                            }
                        },
                    )
                }
            }
        }
        Column(Modifier.fillMaxWidth().background(j.panel).padding(horizontal = 16.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            if (plan != null) {
                Text("¿Seguimos con este plan?", color = j.dim, fontSize = 11.sp)
                for (o in ask.options) Choice(o, false) { onPick(o.number) }
            } else if (ask.multi) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    Text("${ask.options.count { it.checked == true }} elegidas", color = j.dim, fontSize = 12.sp, modifier = Modifier.weight(1f))
                    Text(
                        "Enviar", color = j.bg, fontSize = 14.sp, fontWeight = FontWeight.Bold,
                        modifier = Modifier.heightIn(min = 52.dp).clickable(role = Role.Button, onClick = onSend)
                            .background(j.blue, RoundedCornerShape(12.dp)).padding(horizontal = 28.dp, vertical = 15.dp),
                    )
                }
            } else if (writing) {
                Text(
                    "Enviar", color = j.bg, fontSize = 14.sp, fontWeight = FontWeight.Bold,
                    modifier = Modifier.fillMaxWidth().heightIn(min = 52.dp).clickable(enabled = typed.isNotBlank(), role = Role.Button) { onOther(typed) }
                        .background(if (typed.isBlank()) j.dim else j.blue, RoundedCornerShape(12.dp)).padding(vertical = 15.dp),
                    textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                )
            } else {
                Text("Toca una opción: se envía al instante. No hay aprobar ni rechazar.", color = j.dim, fontSize = 11.sp, lineHeight = 16.sp)
            }
        }
    }
}

/** One option as a row of at least 64 dp: its number or check box, its label and its description. */
@Composable
private fun Choice(o: AskOption, multi: Boolean, onTap: () -> Unit) {
    val j = LocalJw.current
    val on = if (multi) o.checked == true else o.selected
    val shape = RoundedCornerShape(12.dp)
    Row(
        Modifier.fillMaxWidth().heightIn(min = 64.dp).background(if (on) j.sel else j.panel, shape)
            .border(BorderStroke(if (on) 2.dp else 1.dp, if (on) j.blue else j.line), shape)
            .clickable(role = Role.Button, onClick = onTap).padding(horizontal = 14.dp, vertical = 12.dp),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        val mark: Color = if (on) j.blue else j.dim
        Box(
            Modifier.size(26.dp).then(
                if (multi) Modifier.background(if (on) j.blue else Color.Transparent, RoundedCornerShape(7.dp)).border(BorderStroke(1.5.dp, mark), RoundedCornerShape(7.dp))
                else Modifier.border(BorderStroke(1.5.dp, mark), CircleShape),
            ),
            contentAlignment = Alignment.Center,
        ) {
            if (multi) { if (on) Text("✓", color = j.bg, fontSize = 15.sp, fontWeight = FontWeight.Bold) }
            else Text(o.number.toString(), color = mark, fontSize = 12.sp)
        }
        Column(Modifier.weight(1f)) {
            Text(o.label, color = j.fg, fontSize = 13.5.sp, fontWeight = FontWeight.Bold)
            if (o.description.isNotEmpty()) Text(o.description, color = j.dim, fontSize = 11.5.sp, lineHeight = 16.sp, modifier = Modifier.padding(top = 3.dp))
        }
    }
}
