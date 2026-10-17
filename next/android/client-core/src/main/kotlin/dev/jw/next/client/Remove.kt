package dev.jw.next.client

import dev.jw.next.contract.Id
import dev.jw.next.contract.Node
import dev.jw.next.contract.WorkspaceSummary

/** The workspace a long press is on, with what the sheet needs to know about it (REQ-183, REQ-227). */
data class WorkspaceTarget(val id: Id, val shown: String, val kind: HomeKind, val panes: Int, val repo: String?)

fun Node.Session.target(w: WorkspaceSummary) = WorkspaceTarget(w.id, w.shown(), homeKind(w), w.paneCount, w.project)

fun Node.Workspace.target() = WorkspaceTarget(
    id, shownName(name, kind, branch), homeKind(kind, project, namedAsRepo = false), panes.size, project,
)

/** What a choice of the sheet does. */
enum class WsAction { NewWorktree, ExistingWorktrees, Project, Rename, RemoveWorkspace, CloseWorkspace, RemoveWorktree }

data class MenuItem(val action: WsAction, val glyph: String, val label: String, val note: String?, val danger: Boolean)

/** The sheet of a workspace is one for each kind: a repository's root, a folder that is not one, a worktree (REQ-227). Only a worktree is renamed: a folder is a directory on disk. */
fun workspaceMenu(kind: HomeKind, repo: String?): List<MenuItem> = when (kind) {
    HomeKind.Root -> listOfNotNull(
        repo?.let { MenuItem(WsAction.NewWorktree, "w", "worktree nuevo", "de «${repoName(it)}»", false) },
        repo?.let { MenuItem(WsAction.ExistingWorktrees, "e", "worktrees existentes", "de «${repoName(it)}»", false) },
        repo?.let { MenuItem(WsAction.Project, "p", "configuración del proyecto", "«${repoName(it)}»", false) },
        MenuItem(WsAction.RemoveWorkspace, "×", "quitar el workspace", "la carpeta no se toca", true),
    )
    HomeKind.Folder -> listOf(
        MenuItem(WsAction.RemoveWorkspace, "×", "quitar la carpeta", "la carpeta no se toca", true),
    )
    HomeKind.Worktree -> listOfNotNull(
        MenuItem(WsAction.Rename, "✎", "renombrar el worktree", null, false),
        repo?.let { MenuItem(WsAction.ExistingWorktrees, "e", "worktrees existentes", "de «${repoName(it)}»", false) },
        repo?.let { MenuItem(WsAction.Project, "p", "configuración del proyecto", "«${repoName(it)}»", false) },
        MenuItem(WsAction.CloseWorkspace, "×", "cerrar el workspace", "deja la carpeta y la rama", true),
        MenuItem(WsAction.RemoveWorktree, "×", "quitar el worktree", "borra su carpeta", true),
    )
}

/** What the sheet calls each kind: the root of a repository is a plain «workspace». */
private val HomeKind.word get() = when (this) { HomeKind.Worktree -> "worktree"; HomeKind.Folder -> "carpeta"; HomeKind.Root -> "workspace" }

fun menuTitle(t: WorkspaceTarget): String = "${t.kind.word} «${t.shown}»"

/** What `remove_workspace` is asked for: the workspace only, or a worktree's folder with it. */
enum class RemoveMode(val removesWorktree: Boolean) { Workspace(false), KeepWorktree(false), Worktree(true) }

/** The sheet that asks before a workspace is let go of: what it says and what its button does (REQ-228). */
data class RemoveSheet(val workspace: Id, val name: String, val kind: HomeKind, val mode: RemoveMode, val panes: Int) {
    val action: String
        get() = when (mode) {
            RemoveMode.Workspace -> if (kind == HomeKind.Folder) "quitar la carpeta" else "quitar el workspace"
            RemoveMode.KeepWorktree -> "cerrar el workspace"
            RemoveMode.Worktree -> "quitar el worktree"
        }

    val heading: String get() = "$action «$name»"

    val body: String
        get() {
            val closes = when (panes) {
                0 -> "No tiene paneles abiertos."
                1 -> "Se cierra su panel y lo que corra en él."
                else -> "Se cierran sus $panes paneles y lo que corra en ellos."
            }
            val rest = when (mode) {
                RemoveMode.Workspace ->
                    if (kind == HomeKind.Folder) "La carpeta del PC no se toca."
                    else "La carpeta y sus worktrees no se tocan: los worktrees siguen abiertos aparte."
                RemoveMode.KeepWorktree -> "El worktree, su carpeta y su rama se quedan en el PC."
                RemoveMode.Worktree ->
                    "Git borra la carpeta del worktree (git worktree remove, sin forzar). La rama no se borra. " +
                        "Si tiene cambios sin commitear, git lo rechaza y no se quita nada."
            }
            return "$closes $rest"
        }
}

/** The sheet for the choice [action] of the menu of [t]; null for the choices that are not a removal. */
fun removeSheet(t: WorkspaceTarget, action: WsAction): RemoveSheet? {
    val mode = when (action) {
        WsAction.RemoveWorkspace -> RemoveMode.Workspace
        WsAction.CloseWorkspace -> RemoveMode.KeepWorktree
        WsAction.RemoveWorktree -> RemoveMode.Worktree
        else -> return null
    }
    return RemoveSheet(t.id, t.shown, t.kind, mode, t.panes)
}
