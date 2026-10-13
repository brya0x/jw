package dev.jw.mobile

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.jw.proto.CommandInfo

/** Whether what is typed is a command being named: it starts with `/` and has no space yet (REQ-102). */
internal fun isSlashCommand(text: String): Boolean = text.length >= 1 && text[0] == '/' && ' ' !in text && '\n' !in text

/** The commands that match what is typed after the slash, those that start with it first, each group by name. */
internal fun slashMatches(all: List<CommandInfo>, text: String): List<CommandInfo> {
    if (!isSlashCommand(text)) return emptyList()
    val q = text.drop(1).lowercase()
    return all.filter { q in it.name.drop(1).lowercase() }.sortedWith(compareBy({ !it.name.drop(1).lowercase().startsWith(q) }, { it.name }))
}

/** What Claude's own commands do, in the phone's words: the daemon sends no description for them. */
private val BUILTIN_WORDS = mapOf(
    "help" to "Ayuda y comandos disponibles.", "clear" to "Borra la conversación y empieza de nuevo.",
    "compact" to "Resume la conversación para liberar contexto.", "config" to "Abre los ajustes de claude.",
    "context" to "Muestra cuánto contexto se usa.", "cost" to "Muestra el costo y la duración de la sesión.",
    "doctor" to "Revisa la instalación de claude.", "init" to "Crea un CLAUDE.md con la documentación del código.",
    "model" to "Cambia el modelo.", "memory" to "Edita los archivos de memoria.", "permissions" to "Ve y cambia los permisos.",
    "agents" to "Gestiona los subagentes.", "mcp" to "Gestiona los servidores MCP.", "hooks" to "Configura los hooks.",
    "resume" to "Retoma una conversación anterior.", "rewind" to "Vuelve a un punto anterior de la conversación.",
    "status" to "Muestra el estado de claude.", "theme" to "Cambia el tema de color.", "usage" to "Muestra el uso y los límites.",
    "vim" to "Activa o desactiva el modo vim.", "export" to "Exporta la conversación.", "add-dir" to "Añade una carpeta de trabajo.",
    "login" to "Inicia sesión.", "logout" to "Cierra la sesión.", "plan" to "Entra en modo plan.", "review" to "Revisa un pull request.",
    "release-notes" to "Muestra las novedades de la versión.",
)

private fun CommandInfo.words(): String = desc.ifEmpty { BUILTIN_WORDS[name.drop(1)] ?: "" }

private fun CommandInfo.origin(): String = when (kind) {
    "claude" -> "claude"
    "skill" -> "skill"
    else -> "comando"
}

/** The list of what Claude offers after `/`, over the input (REQ-102): a tap puts `/name ` in the field. */
@Composable
fun SlashList(items: List<CommandInfo>, total: Int, onPick: (CommandInfo) -> Unit, modifier: Modifier = Modifier) {
    val j = LocalJw.current
    Column(modifier.fillMaxWidth().heightIn(max = 280.dp).background(j.panel).verticalScroll(rememberScrollState())) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp)) {
            Text("COMANDOS", color = j.dim, fontSize = 10.5.sp, letterSpacing = 0.5.sp, modifier = Modifier.weight(1f))
            Text("${items.size} de $total", color = j.dim, fontSize = 10.5.sp)
        }
        if (items.isEmpty()) {
            Text("Ningún comando empieza así. Se envía tal cual.", color = j.dim, fontSize = 12.sp, modifier = Modifier.padding(12.dp))
        }
        for (c in items) {
            Row(
                Modifier.fillMaxWidth().heightIn(min = 52.dp).clickable(role = Role.Button, onClickLabel = c.name) { onPick(c) }
                    .padding(horizontal = 12.dp, vertical = 8.dp),
                horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.Top,
            ) {
                Text(c.name, color = j.blue, fontSize = 12.5.sp, fontFamily = JwMono, fontWeight = FontWeight.Medium, modifier = Modifier.widthIn(min = 80.dp, max = 140.dp))
                Text(c.words(), color = j.dim, fontSize = 11.5.sp, lineHeight = 16.sp, maxLines = 2, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                Text(
                    c.origin(), color = if (c.kind == "claude") j.dim else j.magenta, fontSize = 10.sp,
                    modifier = Modifier.border(BorderStroke(1.dp, j.line), RoundedCornerShape(8.dp)).padding(horizontal = 6.dp, vertical = 1.dp),
                )
            }
        }
    }
}
