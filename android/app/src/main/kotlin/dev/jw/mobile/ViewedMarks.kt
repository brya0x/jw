package dev.jw.mobile

import android.content.Context
import dev.jw.proto.DiffFile
import dev.jw.proto.DiffScope
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue

/** Where the marks live: preferences in the app, a set in tests. */
interface MarkStore {
    fun load(): Set<String>
    fun save(marks: Set<String>)
}

/** Not secret (a path and a hash of what was shown): plain preferences, like [AnswerPrefs]. */
class PrefsMarkStore(context: Context) : MarkStore {
    private val prefs = context.getSharedPreferences("viewed", Context.MODE_PRIVATE)
    override fun load(): Set<String> = prefs.getStringSet("marks", emptySet()).orEmpty().toSet()
    override fun save(marks: Set<String>) { prefs.edit().putStringSet("marks", marks).apply() }
}

/**
 * The files the person marked "viewed" (REQ-74), per workspace and view. A mark is of what was shown
 * ([DiffFile.key]), so it stops applying when the file changes; [prune] drops the ones for files a view no longer
 * has, so the preferences do not grow for ever.
 */
class ViewedMarks(private val store: MarkStore) {
    /** Snapshot state, replaced whole on every change: whatever reads [isViewed] in a composition redraws when it does. */
    private var marks by mutableStateOf<Set<String>>(LinkedHashSet(store.load()))

    private fun group(ws: String, scope: DiffScope) = "$ws\u0001${scope.word}\u0001"
    private fun entry(ws: String, scope: DiffScope, f: DiffFile) = group(ws, scope) + f.path + "\u0001" + f.key

    fun isViewed(ws: String, scope: DiffScope, f: DiffFile) = entry(ws, scope, f) in marks

    fun toggle(ws: String, scope: DiffScope, f: DiffFile) {
        val e = entry(ws, scope, f)
        commit(LinkedHashSet(marks).also { if (!it.remove(e)) it.add(e) })
    }

    /** How many of `files` are marked. */
    fun count(ws: String, scope: DiffScope, files: List<DiffFile>) = files.count { isViewed(ws, scope, it) }

    /** Drops the marks of `ws`'s `scope` for anything that is not in `files` as it is now. */
    fun prune(ws: String, scope: DiffScope, files: List<DiffFile>) {
        val g = group(ws, scope)
        val keep = files.mapTo(HashSet()) { entry(ws, scope, it) }
        val next = LinkedHashSet(marks)
        if (next.removeAll { it.startsWith(g) && it !in keep }) commit(next)
    }

    private fun commit(next: LinkedHashSet<String>) {
        // Marks of workspaces and commits long gone are never pruned by a view; the oldest go past the cap.
        while (next.size > CAP) next.remove(next.first())
        store.save(LinkedHashSet(next))
        marks = next
    }

    private companion object { const val CAP = 4000 }
}
