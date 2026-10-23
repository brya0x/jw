package dev.jw.next.client

import dev.jw.next.contract.Id
import dev.jw.next.contract.Node
import dev.jw.next.contract.WorkspaceSummary

/** What a choice of a conversation's sheet does (REQ-817, REQ-818). */
enum class ChatAction { Rename, Promote, Archive }

data class ChatMenuItem(val action: ChatAction, val glyph: String, val label: String, val note: String?)

/**
 * The sheet of a conversation of its own, from a long press on its row or from `⋯` in its header: it never
 * goes through the workspace's, which offers «mover a un grupo…» and «cerrar» (REQ-818). With nothing said there is
 * nothing to take to a project.
 */
fun chatMenu(hasMessages: Boolean): List<ChatMenuItem> = listOfNotNull(
    ChatMenuItem(ChatAction.Rename, "✎", "renombrar", null),
    if (hasMessages) ChatMenuItem(ChatAction.Promote, "→", "llevar a un proyecto…", "una conversación nueva, con un resumen de esta") else null,
    ChatMenuItem(ChatAction.Archive, "▾", "archivar", if (hasMessages) "se reanuda desde «archivadas»" else "está vacía: se descarta"),
)

/** The sheet that asks before an archived conversation is deleted (REQ-817). */
data class DeleteChatSheet(val workspace: Id, val name: String) {
    val heading: String get() = "borrar «$name»"
    val body: String get() = "Se borra su carpeta en el PC. El transcript de Claude queda en ~/.claude/projects/."
    val action: String get() = "borrar la conversación"
}

/** The workspaces a conversation can be taken to: the ones that are no conversation of their own and are not being removed (REQ-817). */
fun Node.Session.chatTargets(): List<WorkspaceSummary> = workspaces.filter { it.chat == null && !it.removing }

/** The conversations that are archived, the most recent first as the daemon sends them (REQ-817). */
fun Node.Session.archivedChats(): List<WorkspaceSummary> = workspaces.filter { it.chat?.archived == true }

/** What a conversation of its own that the phone just made, resumed or took a summary to waits for: once the panel has its conversation, its field takes the focus, with [text] in it (REQ-815, REQ-817). */
data class ChatOpen(val workspace: Id, val pane: Id?, val text: String?)

/** What `promote_chat` answers: the new conversation's panel, and the summary for its field. */
data class Promoted(val pane: Id, val text: String)

/** How long ago a conversation said its last line, from its `active` seconds. */
fun chatAgo(nowSecs: Long, active: Long): String = ago(nowSecs, active)
