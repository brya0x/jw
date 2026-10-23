package dev.jw.next.client

import dev.jw.next.contract.ArtifactDoc
import dev.jw.next.contract.ArtifactFormat
import dev.jw.next.contract.Attention
import dev.jw.next.contract.AttentionKind
import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.Delivered
import dev.jw.next.contract.Node
import dev.jw.next.contract.Verdict
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertIs
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** REQ-776, 778, 779, 781, 782, 783, 784 and 791 against the shared fixtures: the list, the bar, the page that holds an artifact, the copies, and what is seen. */
class ArtifactsTest {
    private val dir = File(System.getProperty("contract.dir") ?: error("contract.dir is not set"), "fixtures/daemon")

    private fun msg(name: String): DaemonMsg = ContractJson.decodeFromString(DaemonMsg.serializer(), File(dir, name).readText())

    private fun doc(name: String): ArtifactDoc =
        ContractJson.decodeFromJsonElement(ArtifactDoc.serializer(), (msg(name) as DaemonMsg.Reply).result!!)

    private val node = (msg("snapshot-artifacts.json") as DaemonMsg.Snapshot).node as Node.Artifacts
    private val ajustes = node.items.first { it.name == "ajustes" }
    private val spec = node.items.first { it.name == "n171" }

    @Test
    fun theTwoRepliesOfArtifactReadDecodeAsHtmlOrBlocks() {
        val html = doc("reply-ok-artifact-read.json")
        assertEquals(ArtifactFormat.Html, html.format)
        assertEquals("<h1>Ajustes v2</h1>", html.html)
        assertNull(html.doc)
        val md = doc("reply-ok-artifact-read-md.json")
        assertNull(md.html)
        assertEquals("N171", md.doc?.title)
    }

    @Test
    fun theListHasWhatWaitsFirstAndTheDecidedApartAndStatusIsTheLatestVerdict() {
        assertEquals(ArtifactStatus.Pending, ajustes.status(), "its latest version has no verdict, its first one has")
        assertEquals(ArtifactStatus.Approved, spec.status())
        assertEquals(listOf(ArtifactStatus.Changes, ArtifactStatus.Pending), ajustes.versions.map { it.status() }, "each version has its own")
        val list = artifactList(node.items)
        assertEquals(listOf("ajustes"), list.pending.map { it.name })
        assertEquals(listOf("n171"), list.decided.map { it.name })
        assertEquals(1, pendingArtifacts(node.items))
        val newer = ajustes.copy(versions = ajustes.versions.map { it.copy(at = it.at + 1000) }, name = "newer", id = "x")
        assertEquals(listOf("newer", "ajustes"), artifactList(listOf(ajustes, newer)).pending.map { it.name }, "the newest first")
    }

    @Test
    fun theBarSaysWhatToShowForTheVersionSeenAndTheLink() {
        assertEquals(ArtifactBar.Newer(2), artifactBar(ajustes, viewing = 1, linkUp = true))
        assertEquals(ArtifactBar.Ready, artifactBar(ajustes, viewing = 2, linkUp = true))
        assertEquals(ArtifactBar.Offline, artifactBar(ajustes, viewing = 2, linkUp = false))
        assertEquals(ArtifactBar.Decided(Verdict.Approved, null, Delivered.Nobody), artifactBar(spec, viewing = 1, linkUp = true))
        assertEquals(ArtifactBar.Decided(Verdict.Approved, null, Delivered.Nobody), artifactBar(spec, viewing = 1, linkUp = false), "a verdict is shown whatever the link")
        assertEquals(2, ajustes.version(2).n)
        assertEquals(2, ajustes.version(9).n, "no such version: the latest")
        assertTrue(canAskChanges("más grande"))
        assertFalse(canAskChanges("  \n"))
    }

    @Test
    fun aRefusalOfAVerdictIsSaidInWordsAndAnyOtherIsPassedOn() {
        assertTrue(verdictRefusal("stale: ajustes is at version 3 now").contains("versión nueva"))
        assertTrue(verdictRefusal("decided: that version already has a verdict").contains("ya tiene"))
        assertEquals("se perdió la conexión", verdictRefusal("se perdió la conexión"))
    }

    /** The text of the artifact in the shell, as the iframe reads it back. */
    private fun srcdocOf(shell: String): String =
        Regex("srcdoc=\"([^\"]*)\"></iframe>$").find(shell)!!.groupValues[1].replace("&quot;", "\"").replace("&amp;", "&")

    @Test
    fun theShellPutsThePolicyFirstWhateverTheArtifactSaysAndGivesItBackUnchanged() {
        val cases = listOf(
            "<!-- <head> --><script>fetch('https://example.com')</script>",
            "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src *\"><p>x",
            "\"quoted\" & &amp; &quot; <b>\"</b>",
            "",
        )
        for (artifact in cases) {
            val shell = artifactShell(artifact)
            assertTrue(shell.indexOf("Content-Security-Policy") in 0 until shell.indexOf("<iframe"), "the policy comes first: $artifact")
            assertEquals(1, Regex("<iframe").findAll(shell).count(), "one iframe, whatever the artifact has: $artifact")
            assertEquals(artifact, srcdocOf(shell), "unchanged: $artifact")
        }
        val shell = artifactShell("x")
        assertTrue(shell.contains("<iframe sandbox=\"allow-scripts\" srcdoc="), "scripts in a sandbox, no more")
        assertTrue(ARTIFACT_CSP.contains("connect-src 'none'") && ARTIFACT_CSP.contains("img-src data:"))
        assertTrue(shell.contains("content=\"$ARTIFACT_CSP\""))
    }

    private val session = (msg("snapshot-session-attention.json") as DaemonMsg.Snapshot).node as Node.Session
    private val ws = session.workspaces[0]
    private val waiting = Attention(AttentionKind.Artifact, ws.id, rev = 9)
    private val withArtifact = session.copy(workspaces = listOf(ws.copy(attention = ws.attention + waiting)) + session.workspaces.drop(1))

    private class Fixed(private val session: Node.Session) : NodeSource {
        /** What this phone told the daemon it had seen: ending an attention for every client is not to be done by a tap that does not look. */
        val told = mutableListOf<Pair<String, Long>>()
        override val epoch = session.epoch
        override fun session() = session
        override fun node(id: String): Node? = null
        override fun markSeen(id: String, rev: Long) { told += id to rev }
    }

    @Test
    fun anArtifactIsNotSeenByLookingAtItsWorkspaceButAnyOtherAttentionIs() {
        val seen = Seen().mark(ws.id, 100, session.epoch)
        assertFalse(seen.isSeen(waiting), "opening the workspace does not hide what waits for a verdict")
        assertTrue(seen.isSeen(Attention(AttentionKind.Question, ws.id, rev = 9)))
        val s = Navigator(Fixed(withArtifact)).openWorkspace(ClientState(), ws.id)
        assertTrue(avisos(withArtifact, s.seen).any { it.attention == waiting }, "still in Avisos after opening it")
        assertTrue(homeRows(withArtifact, s.seen).first { it.id == ws.id }.unseen > 0)
    }

    @Test
    fun aTapOnTheAttentionOpensTheListAndBackWalksFromTheArtifactToTheListToTheWorkspace() {
        val source = Fixed(withArtifact)
        val nav = Navigator(source)
        val list = nav.go(ClientState(), waiting)
        assertEquals(Screen.Artifacts(ws.id), list.screen)
        assertFalse(list.seen.isSeen(waiting), "and it is still there after the tap")
        assertEquals(emptyList(), source.told, "the tap did not look at the workspace, so it ends nothing for any client")
        assertEquals(ws.id, list.last)
        val one = nav.artifact(list, ws.id, "a1")
        assertEquals(Screen.Artifact(ws.id, "a1"), one.screen)
        assertEquals(Screen.Artifacts(ws.id), nav.leaveArtifacts(one).screen)
        assertIs<Screen.Workspace>(nav.leaveArtifacts(nav.leaveArtifacts(one)).screen)
        assertTrue(source.told.isNotEmpty(), "arriving at the workspace is looking at it")
        assertEquals(Screen.Home, nav.leaveArtifacts(ClientState()).screen, "anywhere else it is back")
    }

    @Test
    fun theCopiesKeepWhatWasOpenedAndOnlyTheNewestVersionsAndComeBackFromAFile() {
        val html = doc("reply-ok-artifact-read.json")
        var copies = ArtifactCopies()
        assertTrue(copies.isEmpty() && copies.workspaces().isEmpty())
        copies = copies.keep("w1", "app", ajustes, html)
        assertEquals(listOf("w1" to "app"), copies.workspaces())
        assertEquals(listOf(ajustes), copies.items("w1"))
        assertEquals(emptyList(), copies.items("w2"))
        assertEquals(html, copies.docOf(ajustes.id, html.n))
        assertNull(copies.docOf(ajustes.id, 1), "version 1 was never opened")
        // Opened again, it is one copy, and only the newest three versions' texts are kept.
        for (n in 3..6) copies = copies.keep("w1", "app", ajustes, html.copy(n = n))
        assertEquals(1, copies.copies.size)
        assertEquals(setOf(4, 5, 6), copies.copies[0].docs.keys)
        // A file it can read gives them back; one it cannot gives none.
        assertEquals(copies, ArtifactCopies.fromJson(copies.toJson()))
        assertTrue(ArtifactCopies.fromJson("not json").isEmpty())
        assertTrue(ArtifactCopies.fromJson(null).isEmpty())
    }

    @Test
    fun theListSaysWhatEachRowIs() {
        val now = ajustes.latest().at + 180
        assertEquals("v2 · prototipo · 1 KiB · hace 3 min", artifactSub(ajustes, now))
        assertEquals("1 KiB", artifactSize(1))
        assertEquals("38 KiB", artifactSize(38 * 1024L))
        assertEquals("3.0 MiB", artifactSize(3 * 1_048_576L))
        assertEquals(listOf("por revisar", "aprobado", "cambios pedidos"), ArtifactStatus.entries.map { it.label() })
        assertEquals("a3f5c1", shortHash("a3f5c1e07b99"))
        assertEquals(listOf("cambiar workspace", "archivos", "cambios", "artefactos"), WorkspaceMenuRow.entries.map { it.label() })
    }
}
