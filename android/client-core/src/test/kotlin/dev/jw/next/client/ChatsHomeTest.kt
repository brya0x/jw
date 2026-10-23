package dev.jw.next.client

import dev.jw.next.contract.ChatInfo
import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.Group
import dev.jw.next.contract.Id
import dev.jw.next.contract.Node
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** REQ-813, REQ-815, REQ-817, REQ-818: the conversations of their own on the phone. */
class ChatsHomeTest {
    private val session = (ContractJson.decodeFromString(DaemonMsg.serializer(), File(System.getProperty("contract.dir"), "fixtures/daemon/snapshot-session-chats.json").readText()) as DaemonMsg.Snapshot).node as Node.Session
    private val app = session.workspaces[0]
    private val live = session.workspaces[1]
    private val shelved = session.workspaces[2]

    private fun shape(s: Node.Session) = homeItems(s, Seen()).map {
        when (it) {
            is HomeItem.Heading -> "# ${it.text}"
            is HomeItem.Row -> "row:${it.row.id.takeLast(2)}"
            is HomeItem.Group -> "@${it.name}"
            is HomeItem.Empty -> "vacío"
            HomeItem.Ungrouped -> "sin grupo"
            is HomeItem.Chats -> "chats ${it.count}"
            is HomeItem.Chat -> "chat:${it.row.id.takeLast(2)}"
            HomeItem.NoChats -> "ninguna"
            is HomeItem.ArchivedLink -> "archivadas ${it.count}"
        }
    }

    @Test
    fun theChatsAreASectionAfterTheWorkspacesAndTheArchivedAreALink() {
        assertEquals(listOf("row:b1", "chats 1", "chat:b4", "archivadas 1"), shape(session))
        // the archived one has no row of its own, and no number is a chat's
        assertEquals(listOf("row:b1"), homeItems(session, Seen()).filterIsInstance<HomeItem.Row>().map { "row:${it.row.id.takeLast(2)}" })
    }

    @Test
    fun theChatsLeaveCarpetasAndTheGroupsAndComeAfterSinGrupo() {
        val groups = session.copy(groups = listOf(Group("g1", "producto")), workspaces = listOf(app.copy(group = "g1"), live, shelved, app.copy(id = "loose", path = "/home/u/loose", project = null)))
        assertEquals(listOf("@producto", "row:b1", "sin grupo", "row:se", "chats 1", "chat:b4", "archivadas 1"), shape(groups).map { it.replace("row:ose", "row:se") })
        val folders = session.copy(workspaces = listOf(app.copy(project = null), live))
        assertEquals(listOf("row:b1", "chats 1", "chat:b4"), shape(folders), "a chat is no folder: no «carpetas»")
    }

    @Test
    fun withNoLiveChatTheSectionSaysSoAndWithNothingItIsAllThereIs() {
        assertEquals(listOf("row:b1", "chats 0", "ninguna", "archivadas 1"), shape(session.copy(workspaces = listOf(app, shelved))))
        assertEquals(listOf("chats 0", "ninguna"), shape(session.copy(workspaces = emptyList())))
    }

    @Test
    fun aChatRowSaysItsLastLineAndWhenInsteadOfTheFolder() {
        val row = homeItems(session, Seen()).filterIsInstance<HomeItem.Chat>().single().row
        assertEquals("Un índice parcial solo cubre las filas que cumplen una condición.", row.chatLast)
        assertEquals(1761212000L, row.chatActive)
        assertEquals("ahora", chatAgo(1761212000L, 1761212000L))
        assertTrue(chatAgo(1761212000L + 7200, 1761212000L).contains("2"), chatAgo(1761212000L + 7200, 1761212000L))
    }

    @Test
    fun theSheetOfAChatIsItsOwnAndNeverTheWorkspacesWithNothingToTakeWhenItIsEmpty() {
        assertEquals(listOf(ChatAction.Rename, ChatAction.Promote, ChatAction.Archive), chatMenu(hasMessages = true).map { it.action })
        assertEquals(listOf(ChatAction.Rename, ChatAction.Archive), chatMenu(hasMessages = false).map { it.action })
        assertTrue(chatMenu(true).none { it.label.contains("grupo") || it.label.contains("cerrar") })
        assertEquals("borrar «x»", DeleteChatSheet("w", "x").heading)
        assertTrue(DeleteChatSheet("w", "x").body.contains("~/.claude/projects/"))
        // the workspaces a chat can go to: no chat, and none being removed
        assertEquals(listOf(app.id), session.chatTargets().map { it.id })
        assertEquals(listOf(shelved.id), session.archivedChats().map { it.id })
    }

    private fun source(s: Node.Session) = object : NodeSource {
        override val epoch = "e"
        override fun session() = s
        override fun node(id: Id): Node? = null
    }

    @Test
    fun anArchivedChatIsNotOpenedUntilItIsLive() {
        val nav = Navigator(source(session))
        assertNull(nav.openWhenShown(ClientState(), shelved.id), "listed but archived: no panel to open")
        assertNull(nav.openChat(ClientState(), shelved.id, "p1", "texto"))
        assertEquals(Screen.Workspace(live.id, null), nav.openWhenShown(ClientState(), live.id)?.screen)
        // the session says it is live now
        val back = session.copy(workspaces = listOf(app, live, shelved.copy(chat = ChatInfo(archived = false, active = 5))))
        assertNotNull(Navigator(source(back)).openWhenShown(ClientState(), shelved.id))
    }

    @Test
    fun theFieldWaitsForTheOpenedChatAndDoesNotOutliveItsScreen() {
        val nav = Navigator(source(session))
        val opened = assertNotNull(nav.openChat(ClientState(), live.id, "p1", "resumen"))
        assertEquals(ChatOpen(live.id, "p1", "resumen"), opened.chatOpen)
        assertEquals(Screen.Workspace(live.id, null), opened.screen)
        assertNull(nav.openChat(ClientState(), live.id)!!.chatOpen?.text, "a chat made from nothing has no text")
        assertNull(nav.spendChatOpen(opened).chatOpen)
        // leaving the screen drops it, so it cannot surface in that pane days later
        assertNull(nav.back(opened).chatOpen)
        assertNull(nav.show(opened, Screen.Archived).chatOpen)
        assertNotNull(nav.show(opened, Screen.Workspace(live.id, "p1")).chatOpen)
        assertNull(nav.openWorkspace(opened, app.id).chatOpen)
        assertFalse(nav.openChat(ClientState(), live.id, text = "")!!.chatOpen!!.text != null)
    }
}
