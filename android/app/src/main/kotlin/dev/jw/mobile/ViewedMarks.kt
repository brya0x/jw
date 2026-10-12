package dev.jw.mobile

import android.content.Context
import dev.jw.proto.DiffFile
import dev.jw.proto.DiffScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

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
    private var marks = LinkedHashSet(store.load())
    private val _revision = MutableStateFlow(0)

    /** Moves with every change, so what shows the marks redraws. */
    val revision: StateFlow<Int> = _revision

    private fun group(ws: String, scope: DiffScope) = "$ws\u0001${scope.word}\u0001"
    private fun entry(ws: String, scope: DiffScope, f: DiffFile) = group(ws, scope) + f.path + "\u0001" + f.key

    fun isViewed(ws: String, scope: DiffScope, f: DiffFile) = entry(ws, scope, f) in marks

    fun toggle(ws: String, scope: DiffScope, f: DiffFile) {
        val e = entry(ws, scope, f)
        if (!marks.remove(e)) marks.add(e)
        commit()
    }

    /** How many of `files` are marked. */
    fun count(ws: String, scope: DiffScope, files: List<DiffFile>) = files.count { isViewed(ws, scope, it) }

    /** Drops the marks of `ws`'s `scope` for anything that is not in `files` as it is now. */
    fun prune(ws: String, scope: DiffScope, files: List<DiffFile>) {
        val g = group(ws, scope)
        val keep = files.mapTo(HashSet()) { entry(ws, scope, it) }
        if (marks.removeAll { it.startsWith(g) && it !in keep }) commit()
    }

    private fun commit() {
        // Marks of workspaces and commits long gone are never pruned by a view; the oldest go past the cap.
        while (marks.size > CAP) marks.remove(marks.first())
        store.save(LinkedHashSet(marks))
        _revision.value++
    }

    private companion object { const val CAP = 4000 }
}
