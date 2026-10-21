package dev.jw.next.client

/*
 * The `@` of a conversation is written from the folder its engine runs in, which resolves it against that
 * folder: `../` where a file is above it, and any folder, the workspace's or not (REQ-631). The terminal
 * client has the same table (`tui/shell.rs`, `from_folder`).
 */

/** The names of a path, `..` climbing lexically and staying at the root, as the shell does. */
private fun parts(path: String): List<String> {
    val out = ArrayList<String>()
    for (c in path.split('/')) when (c) {
        "", "." -> {}
        ".." -> if (out.isNotEmpty()) out.removeAt(out.lastIndex)
        else -> out.add(c)
    }
    return out
}

/** The path of [rel], a file of the workspace at [workspace], as written from the folder [cwd]. */
fun fromFolder(workspace: String, cwd: String, rel: String): String {
    val from = parts(cwd)
    val to = parts(workspace) + parts(rel)
    val same = from.zip(to).takeWhile { (a, b) -> a == b }.size
    val path = List(from.size - same) { ".." } + to.drop(same)
    return if (path.isEmpty()) "." else path.joinToString("/")
}

/**
 * The reference of lines [from] to [to] (from 1, both included) of the file [rel] of the workspace: `@path#L10-14 `, one
 * line as `#L10`, the path written as the `@` picker writes it from the folder [cwd] the engine runs in when both the
 * workspace's folder and it are known, and as it is when not (REQ-639).
 */
fun lineReference(workspace: String?, cwd: String?, rel: String, from: Int, to: Int): String {
    val path = if (workspace != null && cwd != null) fromFolder(workspace, cwd, rel) else rel
    return "@$path#L$from" + (if (to > from) "-$to" else "") + " "
}

/** The folder [cwd] as a path relative to the [workspace], for the list to start in; "" (its root) when it is the workspace, is outside it or is not known. */
fun folderInWorkspace(workspace: String?, cwd: String?): String {
    if (workspace == null || cwd == null) return ""
    val root = parts(workspace)
    val folder = parts(cwd)
    if (folder.size <= root.size || folder.take(root.size) != root) return ""
    return folder.drop(root.size).joinToString("/")
}
