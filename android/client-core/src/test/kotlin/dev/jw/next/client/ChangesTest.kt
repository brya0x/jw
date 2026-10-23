package dev.jw.next.client

import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.DaemonMsg
import dev.jw.next.contract.DiffScope
import dev.jw.next.contract.Id
import dev.jw.next.contract.Node
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertIs
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** REQ-113 to REQ-121: the changes screen's model, against the shared fixtures. */
class ChangesTest {
    private val dir = File(System.getProperty("contract.dir") ?: error("contract.dir is not set"), "fixtures/daemon")

    private fun fixture(name: String): Node =
        (ContractJson.decodeFromString(DaemonMsg.serializer(), File(dir, name).readText()) as DaemonMsg.Snapshot).node

    /** A PC that answers every request at once with a fixture node, and remembers what it was asked. */
    private class Pc(private val dir: (String) -> Node) : NodeSource {
        val asked = mutableListOf<String>()
        val opened = mutableListOf<Id>()
        val closed = mutableListOf<Id>()
        val marks = mutableListOf<Triple<Id, String, Boolean>>()
        val docs = mutableMapOf<Id, Node>()
        var failWith: String? = null
        private val session = dir("snapshot-session.json") as Node.Session
        override val epoch = session.epoch
        override fun session() = session
        override fun node(id: Id) = docs[id]
        override fun open(id: Id) { opened += id }
        override fun close(id: Id) { closed += id }
        override fun setViewed(node: Id, path: String, viewed: Boolean) { marks += Triple(node, path, viewed) }
        override fun refreshDiff(node: Id) { asked += "refresh $node" }

        private fun answer(name: String, label: String, reply: (Result<Id>) -> Unit) {
            asked += label
            failWith?.let { return reply(Result.failure(Exception(it))) }
            val node = dir(name)
            docs[node.id] = node
            reply(Result.success(node.id))
        }

        override fun openDiff(workspace: Id, scope: DiffScope, commit: String?, reply: (Result<Id>) -> Unit) = answer(
            when (scope) {
                DiffScope.Uncommitted -> "snapshot-diff-uncommitted.json"
                DiffScope.General -> "snapshot-diff.json"
                DiffScope.Commit -> "snapshot-diff-commit.json"
            },
            "diff $scope ${commit ?: ""}".trim(),
            reply,
        )

        override fun openCommits(workspace: Id, reply: (Result<Id>) -> Unit) = answer("snapshot-commits.json", "commits", reply)
    }

    private fun pc() = Pc(::fixture)

    @Test
    fun openingAsksForTheUncommittedChangesOnceAndWatchesThem() {
        val pc = pc()
        val m = ChangesModel(pc, "ws")
        assertEquals(listOf("diff Uncommitted"), pc.asked)
        assertEquals(1, pc.opened.size)
        val c = m.content()
        assertIs<Content.Files>(c)
        assertEquals(3, c.diff.files.size)
        m.select(ChangesView.Uncommitted)
        assertEquals(listOf("diff Uncommitted"), pc.asked, "once")
    }

    @Test
    fun theOtherViewsAreAskedWhenFirstShownAndCountsAppearOnceKnown() {
        val pc = pc()
        val m = ChangesModel(pc, "ws")
        assertNull(m.count(ChangesView.General), "not asked yet: nothing to say")
        m.select(ChangesView.General)
        m.select(ChangesView.Commits)
        m.select(ChangesView.Uncommitted)
        m.select(ChangesView.General)
        assertEquals(listOf("diff Uncommitted", "diff General", "commits"), pc.asked)
        assertEquals("3", m.count(ChangesView.Uncommitted))
        assertEquals("4", m.count(ChangesView.General))
        assertEquals("2", m.count(ChangesView.Commits))
    }

    @Test
    fun aCommitOfTheListOpensItsFilesAndStepsBetweenCommits() {
        val pc = pc()
        val m = ChangesModel(pc, "ws")
        m.select(ChangesView.Commits)
        assertIs<Content.CommitList>(m.content())
        val sha = "89abcde0123456789abcdef0123456789abcdef0"
        m.openCommit(sha)
        assertEquals("diff Commit $sha", pc.asked.last())
        assertIs<Content.Files>(m.content())
        assertTrue(m.hasCommit(-1))
        assertFalse(m.hasCommit(1))
        m.stepCommit(-1)
        assertEquals("0123456789abcdef0123456789abcdef01234567", m.commit)
        assertTrue(m.back(), "back goes to the list")
        assertNull(m.commit)
        assertFalse(m.back(), "then it leaves the screen")
    }

    @Test
    fun backClosesTheFileBeforeAnythingElse() {
        val m = ChangesModel(pc(), "ws")
        m.openFile("notes/todo.md")
        assertEquals("notes/todo.md", m.file)
        assertTrue(m.back())
        assertNull(m.file)
        assertFalse(m.back())
    }

    @Test
    fun aViewThatFailedSaysWhyAndRetryAsksAgain() {
        val pc = pc().apply { failWith = "not inside a git repository" }
        val m = ChangesModel(pc, "ws")
        assertEquals(Content.Failed("not inside a git repository"), m.content())
        assertEquals("!", m.count(ChangesView.Uncommitted))
        m.select(ChangesView.General)
        assertEquals(Content.Failed("not inside a git repository"), m.content(), "each view fails on its own")
        pc.failWith = null
        m.retry()
        assertIs<Content.Files>(m.content())
        assertEquals(3, pc.asked.count { it.startsWith("diff") }, "uncommitted, general, general again")
    }

    @Test
    fun markingAFileAsksThePcToKeepTheMarkAndMarkingASeenOneForgetsIt() {
        val pc = pc()
        val m = ChangesModel(pc, "ws")
        val node = (m.content() as Content.Files).diff
        // notes/todo.md is already seen in the fixture: marking it forgets it.
        m.toggleViewed("notes/todo.md")
        assertEquals(Triple(node.id, "notes/todo.md", false), pc.marks.last())
        m.toggleViewed("old/gone.txt")
        assertEquals(Triple(node.id, "old/gone.txt", true), pc.marks.last())
    }

    @Test
    fun foldingIsRememberedPerView() {
        val m = ChangesModel(pc(), "ws")
        m.toggleFolder("notes/")
        assertTrue(m.isShut("notes/"))
        m.select(ChangesView.General)
        assertFalse(m.isShut("notes/"))
        m.select(ChangesView.Uncommitted)
        assertTrue(m.isShut("notes/"))
    }

    @Test
    fun leavingTheScreenStopsWatchingEveryNodeItOpened() {
        val pc = pc()
        val m = ChangesModel(pc, "ws")
        m.select(ChangesView.General)
        m.close()
        assertEquals(pc.opened.toSet(), pc.closed.toSet())
        assertEquals(2, pc.closed.size)
    }

    @Test
    fun aPatchFromElsewhereSaysUpdatedForAMomentAndOneOfItsOwnDoesNot() {
        var now = 1_000L
        val pc = pc()
        val m = ChangesModel(pc, "ws", clock = { now })
        m.onNodes()
        assertEquals(Live.Following, m.live())
        val id = pc.docs.keys.first()
        val doc = pc.docs[id] as Node.Diff
        pc.docs[id] = doc.copy(rev = doc.rev + 1)
        m.onNodes()
        assertEquals(Live.Updated, m.live())
        now += ChangesModel.UPDATED_MS + 1
        assertEquals(Live.Following, m.live())

        m.toggleViewed("old/gone.txt")
        pc.docs[id] = doc.copy(rev = doc.rev + 2)
        m.onNodes()
        assertEquals(Live.Following, m.live(), "the echo of the person's own mark is not news")
    }

    @Test
    fun theFilesAreATreeWithFolderChainsTotalsAndFolding() {
        val files = (fixture("snapshot-diff.json") as Node.Diff).files
        val rows = treeRows(files) { false }
        val dirs = rows.filterIsInstance<TreeRow.Dir>()
        assertEquals(listOf("assets", "crates/web/src"), dirs.map { it.label })
        assertEquals(15, dirs.last().added)
        assertEquals(7, dirs.last().deleted)
        val shut = treeRows(files) { it == "crates/web/src/" }
        assertTrue(shut.filterIsInstance<TreeRow.File>().none { it.file.path.startsWith("crates") })
        assertEquals(listOf("a/a.rs", "a/b.rs", "m/n/o.rs", "b.rs"), listOf("b.rs", "a/b.rs", "m/n/o.rs", "a/a.rs").sortedWith(::treeCompare))
    }

    @Test
    fun hunkLinesAreNumberedFromTheHunksStarts() {
        val file = (fixture("snapshot-diff.json") as Node.Diff).files.first { it.path == "crates/web/src/auth.rs" }
        val rows = hunkRows(file)
        assertEquals("@@ -41,9 +41,10 @@ impl Auth {", (rows[0] as HunkRow.Head).text)
        val del = rows.filterIsInstance<HunkRow.Line>().first { it.kind == LineKind.Del }
        assertEquals(42 to null, del.old to del.new)
        val add = rows.filterIsInstance<HunkRow.Line>().first { it.kind == LineKind.Add }
        assertEquals(null to 42, add.old to add.new)
    }

    @Test
    fun aCommitIsDatedInWords() {
        assertEquals("ahora", ago(1000, 990))
        assertEquals("hace 2 h", ago(10_000, 10_000 - 7200))
        assertEquals("hace 1 d", ago(1_000_000, 1_000_000 - 90_000))
    }
}
