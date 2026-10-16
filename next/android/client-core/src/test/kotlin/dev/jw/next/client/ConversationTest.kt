package dev.jw.next.client

import dev.jw.next.contract.Accent
import dev.jw.next.contract.AskKind
import dev.jw.next.contract.CommandKind
import dev.jw.next.contract.CommandSource
import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.ConvCommand
import dev.jw.next.contract.ConvItem
import dev.jw.next.contract.ConvMode
import dev.jw.next.contract.ConvStatus
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.Image
import dev.jw.next.contract.Node
import dev.jw.next.contract.SavedConversation
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertIs
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** REQ-440 to REQ-452: what the phone does with a conversation node, against the shared fixture. */
class ConversationTest {
    private val dir = File(System.getProperty("contract.dir") ?: error("contract.dir is not set"), "fixtures/daemon")

    private fun conversation(): Node.Conversation =
        (ContractJson.decodeFromString(DaemonMsg.serializer(), File(dir, "snapshot-conversation.json").readText()) as DaemonMsg.Snapshot).node as Node.Conversation

    private val pic = Image("image/jpeg", "AAAA")

    @Test
    fun anAccentIsAlwaysOneOfTheThemesSixColours() {
        val theme = Themes.get(null, dark = true)
        val six = setOf(theme.blue, theme.green, theme.yellow, theme.red, theme.magenta, theme.cyan)
        assertEquals(six, ACCENTS.map { theme.accentOf(it.first) }.toSet())
        assertEquals(theme.magenta, theme.accentOf(Accent.Magenta))
        assertEquals(listOf("azul", "verde", "amarillo", "rojo", "magenta", "cian"), ACCENTS.map { it.second })
    }

    @Test
    fun theWordsAreTheTerminalClientsAndTheModeIsWrittenWithASpace() {
        assertEquals("accept edits", ConvMode.AcceptEdits.label())
        assertEquals("manual", ConvMode.Manual.label())
        assertEquals("11 s", convSecs(11_000))
        assertEquals("1 min 5 s", convSecs(65_000))
        assertEquals("22.0 k", convKilo(22_024))
        assertEquals("1 herramienta", convTools(1))
        assertEquals("0 herramientas", convTools(0))
        assertEquals("3 herramientas", convTools(3))
        assertEquals("$0,06", convUsd(60_000))
        assertEquals("pide permiso", AskKind.Permission.tag())
        assertEquals("pregunta", AskKind.Question.tag())
        assertEquals("plan listo", AskKind.Plan.tag())
    }

    @Test
    fun theWorkingLineSaysWhatClaudeDoesForHowLongAndHowToStopIt() {
        val c = conversation()
        assertNull(c.copy(status = ConvStatus.Idle).workLine(false, 1_792_110_012_000))
        assertEquals("Pensando… (12 s · ■ interrumpe)", c.copy(status = ConvStatus.Thinking).workLine(false, 1_792_110_012_000))
        assertEquals("Escribiendo… (■ interrumpe)", c.copy(status = ConvStatus.Streaming, since = null).workLine(false, 5))
        // a dead engine is told only once a message was sent to it (REQ-413)
        assertNull(c.copy(status = ConvStatus.Dead).workLine(false, 0))
        assertEquals("retomando conversación…", c.copy(status = ConvStatus.Dead).workLine(true, 0))
        assertEquals("retomando conversación…", c.copy(status = ConvStatus.Resuming).workLine(false, 0))
        assertTrue(c.copy(status = ConvStatus.Tool).working(false))
    }

    @Test
    fun anItemIsCopiedAsItsTextAndATurnAsEverythingFromTheMessageToTheNext() {
        val c = conversation()
        val read = c.items.first { it is ConvItem.Tool } as ConvItem.Tool
        assertEquals("Read(src/auth.rs)\nLeídas 142 líneas\npub fn verify(token: &str) -> bool {\n    …", itemText(read))
        val at = c.items.indexOfFirst { it is ConvItem.Text }
        val turn = turnText(c.items, at)
        assertTrue(turn.startsWith("arregla el test que falla en auth::expired"), turn)
        assertTrue(turn.contains("Edit(src/auth.rs)") && turn.contains("! git status"), turn)
        assertFalse(turn.contains("el daemon se reinició"), "a divider is not the turn")
        assertEquals("", turnText(c.items, 99))
        assertTrue(itemText(ConvItem.Result(11_000, 3, 60_000)).startsWith("Listo en 11 s"))
    }

    @Test
    fun theSlashListHoldsTheCommandsWithTheLettersTheOnesThatBeginWithThemFirst() {
        val c = conversation()
        val names = { line: String -> slashMenu(c.commands, line).map { it.name } }
        assertEquals(listOf("/exit", "/tasks", "/context", "/doctor", "/deploy", "/jwn", "/vercel:deploy"), names("/"))
        assertEquals(listOf("/deploy", "/vercel:deploy"), names("/de"))
        assertEquals(emptyList(), names("/dex"))
        assertEquals(emptyList(), names("/model "), "a space ends the word")
        assertEquals(emptyList(), names("hola"))
        assertEquals("jwn", CommandSource.Jwn.word())
        assertEquals("proyecto", CommandSource.Project.word())
    }

    @Test
    fun whatIsSentDependsOnHowTheLineStarts() {
        val c = conversation()
        assertEquals(Submit.Nothing, submitOf("   ", emptyList()))
        assertEquals(Submit.Say("hola", emptyList()), submitOf("  hola ", emptyList()))
        assertEquals(Submit.Shell("git status"), submitOf("!git status", emptyList()))
        assertEquals(Submit.Say("!", emptyList()), submitOf("!", emptyList()), "a bare bang is a message")
        assertEquals(Submit.Panel("resume", ""), submitOf("/resume", emptyList()))
        assertEquals(Submit.Panel("resume", "abc"), submitOf("/resume abc", emptyList()))
        assertEquals(Submit.Panel("help", ""), submitOf("/help", emptyList()))
        for (w in listOf("model", "effort", "mode", "color", "rename")) assertEquals(Submit.Setting(w), submitOf("/$w", emptyList()), w)
        assertEquals(Submit.Say("/model opus", emptyList()), submitOf("/model opus", emptyList()), "with a value it goes to the daemon")
        assertEquals(Submit.Say("/deploy ahora", emptyList()), submitOf("/deploy ahora", emptyList()))
        assertTrue(c.commands.isNotEmpty())
    }

    @Test
    fun aPictureGoesWithTheMessageOnlyWhileItsMarkIsInTheLine() {
        val a = Image("image/jpeg", "A")
        val b = Image("image/jpeg", "B")
        val sent = submitOf("mira [Imagen#2] aquí", listOf(1 to a, 2 to b))
        assertEquals(Submit.Say("mira [Imagen#2] aquí", listOf(b)), sent)
        assertEquals(Submit.Say("sin ellas", emptyList()), submitOf("sin ellas", listOf(1 to a)))
        assertIs<Submit.Say>(submitOf("[Imagen#1]", listOf(1 to pic)))
    }

    @Test
    fun aPictureIsMadeNoLargerThan1568PixelsOnItsLongSideKeepingItsShape() {
        assertEquals(1000 to 800, fitSize(1000, 800))
        assertEquals(1568 to 1176, fitSize(4032, 3024))
        assertEquals(882 to 1568, fitSize(1764, 3136))
        assertEquals(1568 to 1, fitSize(100_000, 40))
        assertEquals("Imagen#2 · 1568 px · 240 KB", pictureLabel(2, 1568, 1176, 240 * 1024))
        assertEquals(4, IMAGES_MAX)
        assertEquals(5 * 1024 * 1024, IMAGE_MAX_BYTES)
    }

    @Test
    fun theListsOfJwnComeFromTheNode() {
        val c = conversation()
        val help = helpPanel(c)
        assertTrue(help.rows.any { it.label == "+" } && help.rows.any { it.label == "/exit" })
        assertFalse(help.rows.any { it.label == "/context" }, "Claude's are not jwn's")
        val status = statusPanel(c, "/work/app")
        fun at(k: String) = status.rows.first { it.label == k }.detail
        assertEquals("sonnet-5.5", at("modelo"))
        assertEquals("manual", at("modo"))
        assertEquals("/work/app", at("carpeta"))
        assertEquals("$0,06", at("costo"))
        assertEquals("62 %", at("límite de 5 horas"))
        assertEquals("31 %", at("límite de 7 días"))
        val skills = skillsPanel(c)
        assertEquals(listOf("/deploy", "/vercel:deploy", "/jwn"), skills.rows.map { it.label })
        assertEquals(RowAct.Insert("/deploy "), skills.rows[0].act)
        val tasks = tasksPanel(c)
        assertEquals(listOf("en curso", "detenido"), tasks.rows.map { it.tag })
        assertEquals(Tone.Warn, tasks.rows[0].tone)
        assertTrue(tasksPanel(c.copy(tasks = emptyList())).rows.isEmpty())
        val pages = artifactsPanel(c.artifacts)
        assertEquals(RowAct.Open("https://claude.ai/artifact/6HbvGgivzfNgDnkzpDjf4Y"), pages.rows[0].act)
        for (n in listOf("help", "status", "resume", "continue", "skills", "tasks", "artifacts", "permissions", "memory")) assertTrue(isPanel(n), n)
        assertFalse(isPanel("model"))
    }

    @Test
    fun theSavedConversationsResumeByTheirIdAndSayHowLongAgo() {
        val list = listOf(SavedConversation("2cab1f5a-3d20-4cd2-9d13-485b39b26147", "arregla el login", 1_792_110_000_000))
        val p = savedPanel(list)
        assertEquals(RowAct.Resume("2cab1f5a-3d20-4cd2-9d13-485b39b26147"), p.rows[0].act)
        assertEquals("2cab1f5a", p.rows[0].tag)
        assertEquals("ahora", listAgo(10_000, 5_000))
        assertEquals("hace 5 min", listAgo(1_000_000_000, 1_000_000_000 - 5 * 60_000))
        assertEquals("hace 3 h", listAgo(1_000_000_000, 1_000_000_000 - 3 * 3_600_000))
        assertEquals("hace 2 d", listAgo(1_000_000_000, 1_000_000_000 - 2 * 86_400_000))
        assertTrue(savedPanel(emptyList()).rows.isEmpty())
    }

    @Test
    fun aCommandThatNeedsClaudesScreenIsAScreenCommandInTheList() {
        val c = ConvCommand("/rewind", CommandSource.Claude, CommandKind.Screen, "volver a un punto anterior")
        assertEquals(CommandKind.Screen, c.kind)
        assertEquals(listOf("/rewind"), slashMenu(listOf(c), "/re").map { it.name })
    }
}
