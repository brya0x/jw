package dev.jw.next.client

import dev.jw.next.contract.Accent
import dev.jw.next.contract.Artifact
import dev.jw.next.contract.AskKind
import dev.jw.next.contract.CommandSource
import dev.jw.next.contract.ConvCommand
import dev.jw.next.contract.ConvItem
import dev.jw.next.contract.ConvMode
import dev.jw.next.contract.ConvStatus
import dev.jw.next.contract.Image
import dev.jw.next.contract.Node
import dev.jw.next.contract.SavedConversation
import dev.jw.next.contract.TaskState

/*
 * What the phone knows how to do with a `conversation` node (spec REQ-440 to REQ-452), apart from
 * drawing it: the words, the colours it takes from the theme, what a line typed does, the `/`
 * list and the lists of jwn's. Pure functions of the node, so they are tested without a screen.
 */

/** The theme's colour for a conversation's accent: always one of its six (REQ-452). */
fun Theme.accentOf(a: Accent): Long = when (a) {
    Accent.Blue -> blue
    Accent.Green -> green
    Accent.Yellow -> yellow
    Accent.Red -> red
    Accent.Magenta -> magenta
    Accent.Cyan -> cyan
}

/** The six accents with the word each is called by. */
val ACCENTS: List<Pair<Accent, String>> = listOf(
    Accent.Blue to "azul",
    Accent.Green to "verde",
    Accent.Yellow to "amarillo",
    Accent.Red to "rojo",
    Accent.Magenta to "magenta",
    Accent.Cyan to "cian",
)

/** The mode as the phone writes it. */
fun ConvMode.label(): String = if (this == ConvMode.AcceptEdits) "accept edits" else flag

/** The models and efforts the settings offer; the daemon takes these names. */
val MODELS = listOf("fable", "opus", "sonnet", "haiku")
val EFFORTS = listOf("low", "medium", "high", "xhigh", "max")

fun convSecs(ms: Long): String {
    val s = ms / 1000
    return if (s < 60) "$s s" else "${s / 60} min ${s % 60} s"
}

/** «1 herramienta», «3 herramientas». */
fun convTools(n: Int): String = "$n ${if (n == 1) "herramienta" else "herramientas"}"

fun convKilo(n: Long): String = if (n >= 1000) "%.1f k".format(java.util.Locale.ROOT, n / 1000.0) else "$n"

fun convUsd(micro: Long): String = "\$${micro / 1_000_000},%02d".format(java.util.Locale.ROOT, (micro % 1_000_000) / 10_000)

/** Whether Claude is busy for the screen: a dead engine counts only once a message was sent to it. */
fun Node.Conversation.working(retaking: Boolean): Boolean =
    status != ConvStatus.Idle && !(status == ConvStatus.Dead && !retaking)

/** What Claude is doing and for how long, for the line over the field (REQ-400, REQ-413); null when nothing runs. */
fun Node.Conversation.workLine(retaking: Boolean, nowMs: Long): String? {
    if (!working(retaking)) return null
    val what = when (status) {
        ConvStatus.Resuming, ConvStatus.Dead -> return "retomando conversación…"
        ConvStatus.Thinking -> "Pensando…"
        ConvStatus.Streaming -> "Escribiendo…"
        ConvStatus.Tool -> "Trabajando…"
        ConvStatus.Asking -> "Espera tu respuesta"
        ConvStatus.Compacting -> "Compactando…"
        ConvStatus.Idle -> return null
    }
    val elapsed = since?.let { "${convSecs(maxOf(0, nowMs - it))} · " }.orEmpty()
    return "$what (${elapsed}■ interrumpe)"
}

/** The word on the sheet that asks (REQ-441). */
fun AskKind.tag(): String = when (this) {
    AskKind.Permission -> "pide permiso"
    AskKind.Question -> "pregunta"
    AskKind.Plan -> "plan listo"
}

// ---- what is copied (REQ-449) ----

/** An item as the text that is copied from it. */
fun itemText(item: ConvItem): String = when (item) {
    is ConvItem.User -> item.text
    is ConvItem.Text -> item.text
    is ConvItem.Note -> item.text
    is ConvItem.Tool -> (listOf("${item.name}(${item.arg})") + listOfNotNull(item.summary) +
        item.diff.map { "${it.sign.char()} ${it.text}" } + item.output).joinToString("\n")
    is ConvItem.Agent -> (listOf("Agent(${item.name}) ${item.description}") + item.steps).joinToString("\n")
    is ConvItem.Shell -> (listOf("! ${item.command}") + item.output).joinToString("\n")
    is ConvItem.Out -> item.lines.joinToString("\n")
    is ConvItem.Result -> "Listo en ${convSecs(item.ms)} · ${convTools(item.tools)} · ${convUsd(item.cost)}"
    is ConvItem.Divider -> item.text
}

fun dev.jw.next.contract.DiffSign.char(): Char = when (this) {
    dev.jw.next.contract.DiffSign.Add -> '+'
    dev.jw.next.contract.DiffSign.Del -> '-'
    dev.jw.next.contract.DiffSign.Same -> ' '
}

/** The whole turn the item at [index] belongs to: from the person's message to the next one (dividers left out). */
fun turnText(items: List<ConvItem>, index: Int): String {
    if (index !in items.indices) return ""
    val start = (index downTo 0).firstOrNull { items[it] is ConvItem.User } ?: 0
    val end = (index + 1 until items.size).firstOrNull { items[it] is ConvItem.User } ?: items.size
    return items.subList(start, end).filter { it !is ConvItem.Divider }.joinToString("\n\n") { itemText(it) }
}

// ---- the `/` list (REQ-446) ----

private fun CommandSource.rank(): Int = when (this) {
    CommandSource.Jwn -> 0
    CommandSource.Claude -> 1
    CommandSource.Project -> 2
    CommandSource.Skill -> 3
    CommandSource.Plugin -> 4
}

/** The word that says where a command is defined. */
fun CommandSource.word(): String = when (this) {
    CommandSource.Jwn -> "jwn"
    CommandSource.Claude -> "claude"
    CommandSource.Project -> "proyecto"
    CommandSource.Skill -> "skill"
    CommandSource.Plugin -> "plugin"
}

/** The commands whose names hold the letters after the `/` the line is, the ones that begin with them first. */
fun slashMenu(commands: List<ConvCommand>, line: String): List<ConvCommand> {
    val typed = line.removePrefix("/").takeIf { line.startsWith("/") && it.none(Char::isWhitespace) }?.lowercase() ?: return emptyList()
    return commands
        .filter { it.name.removePrefix("/").lowercase().contains(typed) }
        .sortedWith(compareBy({ !it.name.removePrefix("/").lowercase().startsWith(typed) }, { it.source.rank() }, { it.name }))
}

/** What the person typed, and so what the phone does with it when they send it. */
sealed interface Submit {
    data object Nothing : Submit

    /** Say it to Claude. */
    data class Say(val text: String, val images: List<Image>) : Submit

    /** A shell line (`!command`). */
    data class Shell(val command: String) : Submit

    /** One of jwn's lists (`/resume`…); [arg] is what followed the command. */
    data class Panel(val name: String, val arg: String) : Submit

    /** A command whose value is chosen in the settings sheet when it has none: `model`, `effort`, `mode`, `color` or `rename`. */
    data class Setting(val which: String) : Submit
}

private val SETTINGS = setOf("model", "effort", "mode", "color", "rename")

/** What sending [line] does; only the pictures whose mark is still in it go (REQ-447). */
fun submitOf(line: String, pictures: List<Pair<Int, Image>>): Submit {
    val text = line.trim()
    if (text.isEmpty()) return Submit.Nothing
    if (text.startsWith("/")) {
        val word = text.removePrefix("/").substringBefore(' ').substringBefore('\n')
        val arg = text.removePrefix("/").removePrefix(word).trim()
        if (isPanel(word)) return Submit.Panel(word, arg)
        if (word in SETTINGS && arg.isEmpty()) return Submit.Setting(word)
    }
    text.removePrefix("!").takeIf { text.startsWith("!") && it.isNotBlank() }?.let { return Submit.Shell(it.trim()) }
    return Submit.Say(text, pictures.filter { (n, _) -> text.contains("[Imagen#$n]") }.map { it.second })
}

// ---- the lists of jwn's (REQ-450) ----

/** What choosing a row of a list does. */
sealed interface RowAct {
    data object Nothing : RowAct
    data class Insert(val text: String) : RowAct
    data class Resume(val id: String) : RowAct
    data class Copy(val text: String) : RowAct
    data class Open(val url: String) : RowAct
}

data class PanelRow(
    val label: String,
    val detail: String = "",
    val tag: String? = null,
    val tone: Tone? = null,
    /** When it happened, in milliseconds since the epoch: written as how long ago. */
    val at: Long? = null,
    val act: RowAct = RowAct.Nothing,
)

data class PanelView(val title: String, val rows: List<PanelRow>, val empty: String)

/** The lists the phone draws as sheets; `/permissions` and `/memory` read files of the PC and stay in the terminal. */
fun isPanel(name: String) = name in setOf("help", "status", "resume", "continue", "skills", "tasks", "artifacts", "permissions", "memory")

private val TOUCHES = listOf(
    "tocar un mensaje" to "lo selecciona: copiar, copiar el turno",
    "+" to "adjuntar un archivo, una foto o una imagen",
    "/" to "los comandos",
    "@" to "un archivo del workspace",
    "!" to "una línea de shell",
    "■" to "interrumpe lo que Claude hace",
)

fun helpPanel(conv: Node.Conversation) = PanelView(
    "ayuda",
    TOUCHES.map { (k, d) -> PanelRow(k, d, "toque", Tone.Dim) } +
        conv.commands.filter { it.source == CommandSource.Jwn }
            .map { PanelRow(it.name, it.description.orEmpty(), "jwn", Tone.Dim, act = RowAct.Insert("${it.name} ")) },
    "",
)

fun statusPanel(conv: Node.Conversation, cwd: String): PanelView {
    val rows = mutableListOf(
        PanelRow("modelo", conv.model),
        PanelRow("esfuerzo", conv.effort),
        PanelRow("modo", conv.mode.label()),
        PanelRow("sesión", conv.session.orEmpty()),
        PanelRow("carpeta", cwd),
        PanelRow("costo", convUsd(conv.cost)),
    )
    conv.limits?.let { l ->
        rows += PanelRow("límite de 5 horas", l.fiveHour?.let { "$it %" } ?: "—")
        rows += PanelRow("límite de 7 días", l.sevenDay?.let { "$it %" } ?: "—")
    }
    return PanelView("estado", rows, "")
}

fun skillsPanel(conv: Node.Conversation) = PanelView(
    "skills y comandos",
    conv.commands.filter { it.source == CommandSource.Skill || it.source == CommandSource.Project || it.source == CommandSource.Plugin }
        .map { PanelRow(it.name, it.description.orEmpty(), it.source.word(), Tone.Dim, act = RowAct.Insert("${it.name} ")) },
    "Nada de tu proyecto, tus skills o tus plugins.",
)

fun tasksPanel(conv: Node.Conversation) = PanelView(
    "segundo plano",
    conv.tasks.map {
        val (word, tone) = when (it.state) {
            TaskState.Run -> "en curso" to Tone.Warn
            TaskState.Done -> "terminó" to Tone.Good
            TaskState.Stopped -> "detenido" to Tone.Dim
            TaskState.Failed -> "falló" to Tone.Danger
        }
        PanelRow(it.label, it.info.orEmpty(), word, tone)
    },
    "Nada corre en segundo plano.",
)

fun artifactsPanel(artifacts: List<Artifact>) = PanelView(
    "páginas publicadas",
    artifacts.map { PanelRow(it.title, it.url, act = RowAct.Open(it.url)) },
    "Claude no ha publicado ninguna página en esta conversación.",
)

fun savedPanel(list: List<SavedConversation>) = PanelView(
    "retomar una conversación",
    list.map { PanelRow(it.name, tag = it.id.take(8), at = it.at, act = RowAct.Resume(it.id)) },
    "No hay conversaciones guardadas de esta carpeta.",
)

/** How long ago [at] was, as the lists say it. */
fun listAgo(nowMs: Long, at: Long): String {
    val s = maxOf(0, nowMs - at) / 1000
    return when {
        s < 60 -> "ahora"
        s < 3600 -> "hace ${s / 60} min"
        s < 86_400 -> "hace ${s / 3600} h"
        else -> "hace ${s / 86_400} d"
    }
}

// ---- pictures (REQ-447) ----

const val IMAGE_MAX_SIDE = 1568
const val IMAGES_MAX = 4
const val IMAGE_MAX_BYTES = 5 * 1024 * 1024

/** The size a picture of [w] × [h] is made: no more than [IMAGE_MAX_SIDE] on its long side, the same shape. */
fun fitSize(w: Int, h: Int): Pair<Int, Int> {
    val long = maxOf(w, h)
    if (long <= IMAGE_MAX_SIDE) return w to h
    val k = IMAGE_MAX_SIDE.toDouble() / long
    return maxOf(1, (w * k).toInt()) to maxOf(1, (h * k).toInt())
}

/** What the pill of an attached picture says: its mark, its long side and its weight. */
fun pictureLabel(n: Int, w: Int, h: Int, bytes: Int) = "Imagen#$n · ${maxOf(w, h)} px · ${formatSize(bytes.toLong())}"
