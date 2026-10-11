package dev.jw.mobile

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

// The look of docs/specs/mobile-prototype.html: mono type, panel header and status bar with a mode chip,
// 6 dp corners, 1 dp lines, 48 dp touch targets.

private val Corner = RoundedCornerShape(6.dp)

/** `.hdr`: panel background, a line under it, bold title and a dim subtitle; `onBack` adds the 44 dp back button. */
@Composable
fun Header(title: String, subtitle: String? = null, onBack: (() -> Unit)? = null) {
    val j = LocalJw.current
    Row(
        Modifier.fillMaxWidth().background(j.panel).drawBehind {
            drawLine(j.line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx())
        }.padding(horizontal = 12.dp, vertical = 8.dp).heightIn(min = 52.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        if (onBack != null) {
            Box(
                Modifier.size(44.dp).clickable(role = Role.Button, onClickLabel = "Volver", onClick = onBack),
                contentAlignment = Alignment.Center,
            ) { Icon(Ic.Back, j.fg, 22.dp) }
        }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.Center) {
            Text(title, color = j.fg, fontSize = 14.sp, fontWeight = FontWeight.Bold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (subtitle != null) Text(subtitle, color = j.dim, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

/** `.status`: the mode chip and a hint; `msg` is the yellow message on the right. */
@Composable
fun StatusBar(mode: String, chip: Color, hint: String, msg: String? = null) {
    val j = LocalJw.current
    Row(
        Modifier.fillMaxWidth().background(j.panel).drawBehind {
            drawLine(j.line, Offset(0f, 0f), Offset(size.width, 0f), 1.dp.toPx())
        }.padding(horizontal = 12.dp).heightIn(min = 30.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text(
            mode, color = j.bg, fontSize = 11.5.sp, fontWeight = FontWeight.Bold,
            modifier = Modifier.background(chip, RoundedCornerShape(4.dp)).padding(horizontal = 8.dp, vertical = 1.dp),
        )
        Text(hint, color = j.dim, fontSize = 11.5.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
        if (msg != null) Text(msg, color = j.yellow, fontSize = 11.5.sp)
    }
}

/** `.sec`: a dim section label with a rule after it. */
@Composable
fun Section(label: String) {
    val j = LocalJw.current
    Row(Modifier.fillMaxWidth().padding(start = 14.dp, end = 14.dp, top = 14.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(label, color = j.dim, fontSize = 11.sp)
        Box(Modifier.weight(1f).heightIn(min = 1.dp).background(j.line))
    }
}

/** `.row`: key on the left (dim), value in a colour, 50 dp tall. */
@Composable
fun KvRow(key: String, value: String, color: Color = LocalJw.current.fg) {
    val j = LocalJw.current
    Row(Modifier.fillMaxWidth().heightIn(min = 50.dp).padding(horizontal = 14.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(key, color = j.dim, fontSize = 12.sp, modifier = Modifier.padding(end = 12.dp).weight(0.4f))
        Text(value, color = color, fontSize = 13.5.sp, modifier = Modifier.weight(0.6f))
    }
}

/** `.btn.fill` and `.btn.tone`: 48 dp, 6 dp corners; fill is blue, tone has a line. */
@Composable
fun JwButton(label: String, fill: Boolean = false, icon: Ic? = null, onClick: () -> Unit) {
    val j = LocalJw.current
    val base = Modifier.fillMaxWidth().heightIn(min = 48.dp).let {
        if (fill) it.background(j.blue, Corner) else it.border(BorderStroke(1.dp, j.line), Corner)
    }
    Box(base.clickable(role = Role.Button, onClick = onClick).padding(horizontal = 16.dp), contentAlignment = Alignment.Center) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (icon != null) Icon(icon, if (fill) j.bg else j.fg, 18.dp)
            Text(label, color = if (fill) j.bg else j.fg, fontWeight = FontWeight.Bold, fontSize = 13.5.sp)
        }
    }
}

/** A plain paragraph in the screen's body. */
@Composable
fun Body(text: String, color: Color = LocalJw.current.fg, size: Double = 13.5) =
    Text(text, color = color, fontSize = size.sp, lineHeight = (size * 1.45).sp, modifier = Modifier.padding(horizontal = 14.dp))

@Composable
fun Screen(
    title: String,
    subtitle: String?,
    mode: String,
    chip: Color,
    hint: String,
    content: @Composable ColumnScope.() -> Unit,
) {
    val j = LocalJw.current
    Column(Modifier.fillMaxSize().background(j.bg)) {
        Header(title, subtitle)
        Column(Modifier.weight(1f).fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(10.dp), content = {
            Box(Modifier.padding(top = 4.dp))
            content()
        })
        StatusBar(mode, chip, hint)
    }
}
