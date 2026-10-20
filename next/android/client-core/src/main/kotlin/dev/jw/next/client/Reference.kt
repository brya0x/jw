package dev.jw.next.client

/*
 * The `@` of a conversation is written from the folder its engine runs in, which resolves it against that
 * folder: `../` where a file is above it, and any folder, the workspace's or not (REQ-631). The terminal
 * client has the same table (`tui/shell.rs`, `from_folder`).
 */

private fun parts(path: String) = path.split('/').filter { it.isNotEmpty() && it != "." }

/** The path of [rel], a file of the workspace at [workspace], as written from the folder [cwd]. */
fun fromFolder(workspace: String, cwd: String, rel: String): String {
    val from = parts(cwd)
    val to = parts(workspace) + parts(rel)
    val same = from.zip(to).takeWhile { (a, b) -> a == b }.size
    val path = List(from.size - same) { ".." } + to.drop(same)
    return if (path.isEmpty()) "." else path.joinToString("/")
}

/** The folder [cwd] as a path relative to the [workspace], for the list to start in; "" (its root) when it is the workspace, is outside it or is not known. */
fun folderInWorkspace(workspace: String?, cwd: String?): String {
    if (workspace == null || cwd == null) return ""
    val root = parts(workspace)
    val folder = parts(cwd)
    if (folder.size <= root.size || folder.take(root.size) != root) return ""
    return folder.drop(root.size).joinToString("/")
}
