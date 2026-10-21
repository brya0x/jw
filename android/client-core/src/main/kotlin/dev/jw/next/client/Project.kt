package dev.jw.next.client

import dev.jw.next.contract.ConfigFrom
import dev.jw.next.contract.OpenKind
import dev.jw.next.contract.ProjectEdit
import dev.jw.next.contract.ProjectInfo

/** The fields of the project screen, in the order they are shown (REQ-520). */
enum class ProjectField(val label: String) {
    Root("carpeta de worktrees"),
    Setup("setup"),
    Copy("copiar"),
    Ports("puertos"),
    Open("al abrir"),
}

/** What the screen shows and edits: [rootSet] is false while the root is the default one, as the PC says it. */
data class ProjectValues(
    val root: String,
    val rootSet: Boolean,
    val setup: List<String>,
    val copy: List<String>,
    val ports: List<Pair<String, Int>>,
    val open: OpenKind,
)

fun ProjectInfo.values() = ProjectValues(rootText ?: root, rootText != null, setup, copy, ports.toList(), open)

/** The ports as they are typed on the screen: `name=offset`, one to a line (REQ-523). */
fun portLines(ports: List<Pair<String, Int>>): List<String> = ports.map { (name, offset) -> "$name=$offset" }

/** What a list of lines typed as ports means, or the first line that cannot be one (REQ-523). */
fun parsePorts(lines: List<String>): Result<List<Pair<String, Int>>> {
    val ports = mutableListOf<Pair<String, Int>>()
    for (line in lines.map { it.trim() }.filter { it.isNotEmpty() }) {
        val name = line.substringBefore('=').trim()
        val offset = line.substringAfter('=', "").trim().toIntOrNull()
        val nameOk = name.isNotEmpty() && name.all { it in 'a'..'z' || it in '0'..'9' || it == '_' }
        if (!nameOk || name == "base" || offset == null || offset !in 0..99) {
            return Result.failure(Exception("«$line»: un nombre en minúsculas, números y _ (no «base»), «=» y un desplazamiento de 0 a 99"))
        }
        if (ports.any { it.first == name }) return Result.failure(Exception("el puerto $name ya está"))
        ports += name to offset
    }
    return Result.success(ports)
}

/**
 * The settings of a repository as the project screen holds them (REQ-520 to REQ-526): [orig] is what the PC
 * said, [cur] what the person has changed, and [to] the file saving writes to.
 */
data class ProjectDraft(val info: ProjectInfo, val orig: ProjectValues, val cur: ProjectValues, val to: ConfigFrom) {
    val dirty get() = cur != orig

    /** Saving would write nothing: no change, and the file is the one the settings came from. */
    val nothingToSave get() = !dirty && info.from == to

    fun warning(): String = when {
        to == ConfigFrom.Personal && info.from == ConfigFrom.Repo -> "quedaría tapada por el .jw.toml del repositorio"
        to == ConfigFrom.Repo && info.from == ConfigFrom.Repo -> "cambia el .jw.toml que ve el equipo"
        to == ConfigFrom.Repo -> "crea un .jw.toml en el repositorio"
        else -> ""
    }

    fun target(repo: String): String = if (to == ConfigFrom.Repo) "${repo.trimEnd('/')}/.jw.toml (lo ve el equipo)" else info.personal

    fun isDefault(field: ProjectField) = when (field) {
        ProjectField.Root -> !cur.rootSet
        ProjectField.Setup -> cur.setup.isEmpty()
        ProjectField.Copy -> cur.copy.isEmpty()
        ProjectField.Ports -> cur.ports.isEmpty()
        ProjectField.Open -> cur.open == OpenKind.Shell
    }

    /** An empty root is the default one again. */
    fun withRoot(text: String): ProjectDraft {
        val t = text.trim()
        return copy(cur = cur.copy(root = t.ifEmpty { info.root }, rootSet = t.isNotEmpty()))
    }

    /** The lines of a list of the settings, with the blank ones taken away. */
    fun withList(field: ProjectField, lines: List<String>): ProjectDraft {
        val clean = lines.map { it.trim() }.filter { it.isNotEmpty() }
        return when (field) {
            ProjectField.Setup -> copy(cur = cur.copy(setup = clean))
            ProjectField.Copy -> copy(cur = cur.copy(copy = clean))
            else -> this
        }
    }

    fun withPorts(ports: List<Pair<String, Int>>) = copy(cur = cur.copy(ports = ports))
    fun withOpen(open: OpenKind) = copy(cur = cur.copy(open = open))
    fun withTarget(to: ConfigFrom) = copy(to = to)

    /**
     * What to send: the fields that changed, and all of them when the file saved to is not the one the
     * settings came from, so that it says what the screen shows (REQ-524).
     */
    fun edit(): ProjectEdit {
        val all = info.from != to
        val root = when {
            cur.rootSet && (all || cur.root != orig.root) -> cur.root
            !cur.rootSet && orig.rootSet -> ""
            else -> null
        }
        return ProjectEdit(
            root = root,
            setup = cur.setup.takeIf { all || it != orig.setup },
            copy = cur.copy.takeIf { all || it != orig.copy },
            ports = cur.ports.toMap().takeIf { all || cur.ports != orig.ports },
            open = cur.open.takeIf { all || it != orig.open },
        )
    }
}

fun ProjectInfo.draft(): ProjectDraft {
    val v = values()
    return ProjectDraft(this, v, v, if (from == ConfigFrom.Repo) ConfigFrom.Repo else ConfigFrom.Personal)
}

/** What the screen knows of the settings: the PC's answer to `project_config`. */
sealed interface ProjectState {
    data object Reading : ProjectState
    data class Ready(val draft: ProjectDraft) : ProjectState
    data class Refused(val why: String) : ProjectState
}

/** What saving said: where the settings went and whether the repository's own file hides them, or why not (REQ-525). */
fun savedNote(result: Result<Pair<String, Boolean>>): String = result.fold(
    { (path, shadowed) -> "guardado en $path" + if (shadowed) " · queda tapado por el .jw.toml del repositorio" else "" },
    { "no se pudo guardar: ${it.message ?: "el PC no contestó"}" },
)
