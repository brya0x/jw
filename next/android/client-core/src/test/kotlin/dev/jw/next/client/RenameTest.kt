package dev.jw.next.client

import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.Node
import dev.jw.next.contract.WorkspaceKind
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/** REQ-184 and REQ-185 against the shared fixtures. */
class RenameTest {
    private val session = (ContractJson.decodeFromString(
        DaemonMsg.serializer(),
        File(System.getProperty("contract.dir") ?: error("contract.dir is not set"), "fixtures/daemon/snapshot-session-attention.json").readText(),
    ) as DaemonMsg.Snapshot).node as Node.Session

    @Test
    fun aWorktreeWithItsAutomaticNameShowsItsBranchAndWithAnotherShowsIt() {
        assertEquals("feat/login", shownName("app: feat/login", WorkspaceKind.Worktree, "feat/login"))
        assertEquals("login fix", shownName("login fix", WorkspaceKind.Worktree, "feat/login"))
        assertEquals("feat/login", shownName("feat/login", WorkspaceKind.Worktree, "feat/login"))
        assertEquals("a: b", shownName("a: b", WorkspaceKind.Folder, null), "a folder shows its name")
        assertEquals("app: x", shownName("app: x", WorkspaceKind.Worktree, null), "no branch, nothing to show")
    }

    @Test
    fun theRowsAndTheAvisosUseTheShownName() {
        val row = homeRows(session, Seen()).first { it.name.startsWith("↳") }
        assertEquals("↳ feat/login", row.name)
        assertTrue(row.detail.startsWith("worktree · "), row.detail)
        val folder = homeRows(session, Seen()).first { !it.name.startsWith("↳") }
        assertTrue(folder.detail.startsWith("carpeta · "), folder.detail)
        assertTrue(avisos(session, Seen()).any { it.workspace == "feat/login" })
    }

    @Test
    fun aRenamedWorktreeShowsTheNameChosen() {
        val renamed = session.copy(workspaces = session.workspaces.map { if (it.kind == WorkspaceKind.Worktree) it.copy(name = "login fix") else it })
        assertTrue(homeRows(renamed, Seen()).any { it.name == "↳ login fix" })
    }

    @Test
    fun aSourceWithNoPcAnswersARenameWithItsMessage() {
        val source = object : NodeSource {
            override val epoch = ""
            override fun session() = session
            override fun node(id: String): Node? = null
        }
        var got: Result<Unit>? = null
        source.renameWorkspace("w", "x") { got = it }
        assertEquals("sin conexión a un PC", got!!.exceptionOrNull()!!.message)
        got = null
        source.renamePane("p", "x") { got = it }
        assertEquals("sin conexión a un PC", got!!.exceptionOrNull()!!.message)
    }
}
