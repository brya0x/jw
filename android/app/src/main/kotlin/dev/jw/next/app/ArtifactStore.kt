package dev.jw.next.app

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import dev.jw.next.client.ArtifactCopies
import dev.jw.next.contract.ArtifactDoc
import dev.jw.next.contract.ArtifactItem
import dev.jw.next.contract.Id
import java.io.File

/**
 * What the phone keeps of the artifacts it opened, in the app's own private folder, one file per PC: they
 * are shown read only when the link is off (REQ-783) and deleted when the PC is revoked or forgotten (REQ-784).
 */
class ArtifactStore(private val context: Context) {
    private fun file(pc: String) = File(context.filesDir, "artifacts-${pc.filter { it.isLetterOrDigit() || it == '-' }}.json")

    fun load(pc: String): ArtifactCopies = ArtifactCopies.fromJson(file(pc).takeIf { it.exists() }?.readText())

    fun save(pc: String, copies: ArtifactCopies) {
        val target = file(pc)
        val tmp = File(target.path + ".tmp")
        tmp.writeText(copies.toJson())
        tmp.renameTo(target)
    }

    fun wipe(pc: String) {
        file(pc).delete()
        File(file(pc).path + ".tmp").delete()
    }
}

/** The copies of the PC in use, watched by the screens and written through to the file as an artifact is opened. */
class ArtifactCopyBox(private val store: ArtifactStore, private val pc: String) {
    var copies by mutableStateOf(store.load(pc))
        private set

    fun keep(workspace: Id, workspaceName: String, item: ArtifactItem, doc: ArtifactDoc) {
        copies = copies.keep(workspace, workspaceName, item, doc)
        store.save(pc, copies)
    }

    fun wipe() {
        copies = ArtifactCopies()
        store.wipe(pc)
    }
}
